import AppKit
import CMuxterm
import MuxtermChrome

/// 主窗口：持有 CoreBridge + Timer 轮询 `muxterm_poll_events`，分发到 UI。
final class MainWindowController: NSWindowController, NSWindowDelegate {
    private var bridge: CoreBridge
    private let terminalManager: TerminalManager
    private let content: ContentView
    private let discovery = ConnectionDiscovery()
    private var commandPalette: CommandPaletteController!
    private var pollTimer: Timer?
    private var lastSnapshot = FrameSnapshot()
    private var needsLayoutReload = true
    /// tmux tab 切换命令异步完成前，禁止用旧 active tab 快照重建布局。
    private var pendingActiveTab: UInt32?
    private var isClosing = false
    private var languageObserver: NSObjectProtocol?

    init(bridge: CoreBridge) {
        self.bridge = bridge
        self.terminalManager = TerminalManager(bridge: bridge)
        self.content = ContentView(terminalManager: terminalManager)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Muxterm"
        window.titleVisibility = .visible
        window.minSize = NSSize(width: 480, height: 320)
        window.center()
        window.contentView = content
        window.setAccessibilityIdentifier("muxterm.mainWindow")

        super.init(window: window)
        window.delegate = self

        commandPalette = CommandPaletteController(ownerWindow: window)
        commandPalette.onSelect = { [weak self] item in
            self?.handlePaletteSelection(item)
        }
        languageObserver = NotificationCenter.default.addObserver(
            forName: .muxtermLanguageChanged,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.refreshLocalizedUI()
        }

        content.tabBar.onSelectTab = { [weak self] tabId in
            self?.requestSwitchTab(tabId)
        }
        content.tabBar.onNewTab = { [weak self] in
            self?.newTab()
        }
        content.paneLayout.onActivatePane = { [weak self] paneId in
            guard let self else { return }
            if self.bridge.execute(task: MuxTask.switchPane(paneId)) != 0 {
                self.reportStatusError(
                    MuxtermI18n.shared.tr(.errorSwitchPane, arguments: ["id": "\(paneId)"])
                )
            }
        }
        content.paneLayout.onResizeDivider = { [weak self] paneId, horizontal, size in
            guard let self, self.terminalManager.usesClientResize else { return }
            _ = self.terminalManager.resizePaneAxis(
                paneId: paneId,
                horizontal: horizontal,
                size: size
            )
        }
        terminalManager.onOutputSnippetChanged = { [weak self] snippet in
            self?.content.statusBar.updateOutputSnippet(snippet)
        }
        terminalManager.onError = { [weak self] message in
            self?.reportStatusError(message)
        }

        installKeyEquivalents()
        startPolling()
        DispatchQueue.main.async { [weak self] in
            self?.refreshUI()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    deinit {
        pollTimer?.invalidate()
        if let languageObserver {
            NotificationCenter.default.removeObserver(languageObserver)
        }
        if !isClosing {
            bridge.shutdown()
        }
    }

    // MARK: - 公开动作（菜单 / 快捷键）

    @objc func newTab() {
        guard bridge.execute(task: MuxTask.newTab()) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorNewTab))
            return
        }
        needsLayoutReload = true
    }

    @objc func closeActivePane() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id ?? lastSnapshot.panes.first?.id else {
            return
        }
        // 唯一 pane 时关 pane 会触发后端关 window；UI 侧随后收到 Exited 再关窗口。
        guard bridge.execute(task: MuxTask.closePane(pane)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorClosePane, arguments: ["id": "\(pane)"]))
            return
        }
        needsLayoutReload = true
    }

    @objc func closeActiveWindow() {
        closeSessionWindow()
    }

    /// 菜单 Cmd+1..9：tag 为 1-based 序号。
    @objc func switchTabByNumber(_ sender: Any?) {
        let n: Int
        if let item = sender as? NSMenuItem {
            n = item.tag
        } else if let num = sender as? NSNumber {
            n = num.intValue
        } else {
            return
        }
        switchToTabIndex(n)
    }

    /// 1-based 序号切换 tab。
    func switchToTabIndex(_ oneBased: Int) {
        guard oneBased >= 1, oneBased <= lastSnapshot.tabs.count else { return }
        let tabId = lastSnapshot.tabs[oneBased - 1].id
        requestSwitchTab(tabId)
    }

    @objc func splitHorizontal() {
        splitActivePane(horizontal: true)
    }

    @objc func splitVertical() {
        splitActivePane(horizontal: false)
    }

    @objc func setTabBarTop(_ sender: Any?) {
        TabBarPosition.set(.top)
        content.applyTabBarPosition(.top)
    }

    @objc func setTabBarBottom(_ sender: Any?) {
        TabBarPosition.set(.bottom)
        content.applyTabBarPosition(.bottom)
    }

    @objc func nextPane() {
        movePane(offset: 1)
    }

    @objc func prevPane() {
        movePane(offset: -1)
    }

    @objc func openCommandPalette() {
        guard let commandPalette else { return }
        if commandPalette.window?.isKeyWindow == true {
            commandPalette.dismiss()
        } else {
            commandPalette.present(items: rootPaletteItems())
        }
    }

    private func focusActiveTerminal() {
        let snap = bridge.snapshot()
        guard let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id else {
            return
        }
        let view = terminalManager.view(for: activePane)
        terminalManager.focusTarget = view
        window?.makeFirstResponder(view)
        content.paneLayout.markActivePane(activePane)
    }

    private func requestSwitchTab(_ tabId: UInt32) {
        guard tabId != lastSnapshot.activeTab else { return }
        pendingActiveTab = tabId
        needsLayoutReload = true
        guard bridge.execute(task: MuxTask.switchTab(tabId)) == 0 else {
            pendingActiveTab = nil
            reportStatusError(MuxtermI18n.shared.tr(.errorSwitchTab, arguments: ["id": "\(tabId)"]))
            return
        }
        // 等 STATE_ACTIVE_TAB_CHANGED 到达后再 refreshUI；此时 snapshot 的
        // panes/layout 才保证属于同一个 active tab。
    }

    private func splitActivePane(horizontal: Bool) {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id ?? lastSnapshot.panes.first?.id else {
            return
        }
        guard bridge.execute(task: MuxTask.splitPane(targetPane: pane, horizontal: horizontal)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorSplitPane, arguments: ["id": "\(pane)"]))
            return
        }
        needsLayoutReload = true
    }

    /// 在当前 tab 的布局叶子中循环切换 pane。
    ///
    /// 这里显式发送目标 pane，而不是发送 NextPane/PrevPane 让核心再次
    /// 从全局 active 状态推断。这样 tab 切换后 Cmd+[ / Cmd+] 的行为只
    /// 依赖当前 tab 快照，不会因为旧 staticlib 或焦点事件顺序回到首 tab。
    private func movePane(offset: Int) {
        let snap = bridge.snapshot()
        let paneIDs = snap.layout?.leafPaneIDs() ?? snap.panes.map(\.id)
        guard let target = PaneNavigation.target(
            paneIDs: paneIDs,
            activePaneID: snap.activePane,
            offset: offset
        ) else { return }

        guard bridge.execute(task: MuxTask.switchPane(target)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorSwitchPane, arguments: ["id": "\(target)"]))
            return
        }
    }

    // MARK: - 命令面板

    private func rootPaletteItems() -> [PaletteItem] {
        let i18n = MuxtermI18n.shared
        var items = [
            PaletteItem(
                title: i18n.tr(.local),
                detail: i18n.tr(.localTmuxSessions),
                keywords: "local tmux attach new 本地",
                kind: .command(.local)
            ),
            PaletteItem(
                title: i18n.tr(.ssh),
                detail: i18n.tr(.sshHosts),
                keywords: "ssh remote host tmux attach new 主机",
                kind: .command(.ssh)
            ),
            PaletteItem(
                title: i18n.tr(.newTab),
                detail: i18n.tr(.newTabDetail),
                keywords: "new tab 新建 标签页",
                kind: .command(.newTab)
            ),
            PaletteItem(
                title: i18n.tr(.splitPaneHorizontal),
                detail: i18n.tr(.splitPaneHorizontalDetail),
                keywords: "split pane horizontal 水平 分割",
                kind: .command(.splitHorizontal)
            ),
            PaletteItem(
                title: i18n.tr(.splitPaneVertical),
                detail: i18n.tr(.splitPaneVerticalDetail),
                keywords: "split pane vertical 竖直 上下",
                kind: .command(.splitVertical)
            ),
            PaletteItem(
                title: i18n.tr(.nextPane),
                detail: i18n.tr(.nextPaneDetail),
                keywords: "pane next cmd bracket 下一个",
                kind: .command(.nextPane)
            ),
            PaletteItem(
                title: i18n.tr(.previousPane),
                detail: i18n.tr(.previousPaneDetail),
                keywords: "pane previous prev cmd bracket 上一个",
                kind: .command(.prevPane)
            ),
            PaletteItem(
                title: i18n.tr(.closePane),
                detail: i18n.tr(.closePaneDetail),
                keywords: "close pane 关闭",
                kind: .command(.closePane)
            ),
            PaletteItem(
                title: i18n.tr(.closeTab),
                detail: i18n.tr(.closeTabDetail),
                keywords: "close tab 关闭 标签页",
                kind: .command(.closeTab)
            ),
            PaletteItem(
                title: i18n.tr(.closeWindow),
                detail: i18n.tr(.closeWindowDetail),
                keywords: "close window 关闭",
                kind: .command(.closeWindow)
            ),
            PaletteItem(
                title: i18n.tr(.language),
                detail: i18n.tr(.languageDetail),
                keywords: "language locale 语言",
                kind: .command(.language)
            ),
            PaletteItem(
                title: i18n.tr(.quitMuxterm),
                detail: i18n.tr(.quitMuxtermDetail),
                keywords: "quit exit 退出",
                kind: .command(.quit)
            ),
        ]

        // detach 只对 tmux/SSH 控制 client 有意义；local shell 不能显示这个命令。
        // 关闭窗口时 CoreBridge.shutdown() 会发送 detach-client，保留 tmux session。
        if terminalManager.usesClientResize {
            items.insert(
                PaletteItem(
                    title: i18n.tr(.detach),
                    detail: i18n.tr(.detachDetail),
                    keywords: "detach tmux session 分离连接",
                    kind: .command(.detach)
                ),
                at: 2
            )
        }
        return items
    }

    private func handlePaletteSelection(_ item: PaletteItem) {
        switch item.kind {
        case .command(.local):
            showSessions(for: .local)
        case .command(.ssh):
            showSSHHosts()
        case .command(.newTab):
            commandPalette.dismiss()
            newTab()
        case .command(.splitHorizontal):
            commandPalette.dismiss()
            splitHorizontal()
        case .command(.splitVertical):
            commandPalette.dismiss()
            splitVertical()
        case .command(.nextPane):
            commandPalette.dismiss()
            nextPane()
        case .command(.prevPane):
            commandPalette.dismiss()
            prevPane()
        case .command(.closePane):
            commandPalette.dismiss()
            closeActivePane()
        case .command(.closeTab):
            commandPalette.dismiss()
            closeActiveTab()
        case .command(.closeWindow):
            commandPalette.dismiss()
            closeActiveWindow()
        case .command(.detach):
            commandPalette.dismiss()
            detachSessionWindow()
        case .command(.language):
            showLanguageOptions()
        case .language(let language):
            _ = MuxtermI18n.shared.setLanguage(language)
            commandPalette.present(items: rootPaletteItems())
        case .command(.quit):
            commandPalette.dismiss()
            NSApp.terminate(nil)
        case .command:
            break
        case .host(let host):
            showSessions(for: .ssh(host))
        case .session(let target, let name):
            attach(target: target, session: name)
        case .newSession(let target):
            commandPalette.dismiss()
            chooseDirectory(for: target)
        }
    }

    private func showLanguageOptions() {
        let i18n = MuxtermI18n.shared
        let items = MuxtermLanguage.allCases.map { language in
            let current = i18n.language == language ? " · \(i18n.tr(.languageCurrent))" : ""
            return PaletteItem(
                title: i18n.tr(language.displayNameKey),
                detail: current.isEmpty ? "" : current,
                keywords: "language locale 语言 \(language.rawValue)",
                kind: .language(language)
            )
        }
        commandPalette.update(
            items: items,
            placeholder: i18n.tr(.language)
        )
    }

    private func showSSHHosts() {
        switch discovery.sshHosts() {
        case .success(let hosts):
            commandPalette.update(
                items: hosts.map { host in
                    PaletteItem(
                        title: host.alias,
                        detail: "\(host.user.map { "\($0)@" } ?? "")\(host.hostname)\(host.port.map { ":\($0)" } ?? "")",
                        keywords: "ssh host machine remote",
                        kind: .host(host)
                    )
                },
                placeholder: MuxtermI18n.shared.tr(.chooseSshHost)
            )
        case .failure(let error):
            commandPalette.dismiss()
            showError(error)
        }
    }

    private func showSessions(for target: ConnectionTarget) {
        commandPalette.update(
            items: [PaletteItem(
                title: MuxtermI18n.shared.tr(.newSession),
                detail: MuxtermI18n.shared.tr(.newSessionDetail),
                keywords: "new create tmux directory folder",
                kind: .newSession(target: target)
            )],
            placeholder: MuxtermI18n.shared.tr(.chooseTmuxSession)
        )

        let finish: (Result<[TmuxSessionInfo], Error>) -> Void = { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let sessions):
                let items = [PaletteItem(
                    title: MuxtermI18n.shared.tr(.newSession),
                    detail: MuxtermI18n.shared.tr(.newSessionDetail),
                    keywords: "new create tmux directory folder",
                    kind: .newSession(target: target)
                )] + sessions.map { session in
                    PaletteItem(
                        title: session.name,
                        detail: MuxtermI18n.shared.tr(
                            .tmuxWindows,
                            arguments: ["count": "\(session.windowCount)"]
                        ) + (session.attached ? " · \(MuxtermI18n.shared.tr(.tmuxAttached))" : ""),
                        keywords: "tmux session attach",
                        kind: .session(target: target, name: session.name)
                    )
                }
                self.commandPalette.update(
                    items: items,
                    placeholder: MuxtermI18n.shared.tr(.chooseTmuxSession)
                )
            case .failure(let error):
                self.commandPalette.dismiss()
                self.showError(error)
            }
        }

        switch target {
        case .local:
            discovery.listLocalSessions(completion: finish)
        case .ssh(let host):
            discovery.listRemoteSessions(host: host, completion: finish)
        }
    }

    private func chooseDirectory(for target: ConnectionTarget) {
        guard let ownerWindow = window else {
            reportStatusError(MuxtermI18n.shared.tr(.errorMainWindowUnavailable))
            return
        }
        switch target {
        case .local:
            let panel = NSOpenPanel()
            panel.title = MuxtermI18n.shared.tr(.chooseTmuxDirectory)
            panel.message = MuxtermI18n.shared.tr(.chooseDirectoryMessage)
            panel.canChooseFiles = false
            panel.canChooseDirectories = true
            panel.allowsMultipleSelection = false
            panel.beginSheetModal(for: ownerWindow) { [weak self] response in
                guard response == .OK, let directory = panel.url?.path else { return }
                self?.createSession(target: target, directory: directory)
            }
        case .ssh(let host):
            let alert = NSAlert()
            alert.messageText = MuxtermI18n.shared.tr(.chooseRemoteDirectory)
            alert.informativeText = MuxtermI18n.shared.tr(
                .remoteDirectoryMessage,
                arguments: ["host": host.alias]
            )
            let field = NSTextField(string: "~")
            field.frame = NSRect(x: 0, y: 0, width: 320, height: 24)
            alert.accessoryView = field
            alert.addButton(withTitle: MuxtermI18n.shared.tr(.createAndAttach))
            alert.addButton(withTitle: MuxtermI18n.shared.tr(.cancel))
            alert.beginSheetModal(for: ownerWindow) { [weak self] response in
                guard response == .alertFirstButtonReturn else { return }
                let directory = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !directory.isEmpty else { return }
                self?.createSession(target: target, directory: directory)
            }
        }
    }

    private func createSession(target: ConnectionTarget, directory: String) {
        discovery.createSession(target: target, directory: directory) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let session):
                self.attach(target: target, session: session)
            case .failure(let error):
                self.showError(error)
            }
        }
    }

    private func attach(target: ConnectionTarget, session: String) {
        commandPalette.dismiss()
        let backend: String
        let socket: String?
        switch target {
        case .local:
            backend = "tmux"
            socket = nil
        case .ssh(let host):
            backend = "ssh"
            socket = host.alias
        }

        // CoreBridge 的 connect 可能等待远端 tmux 初始化，放到后台线程。
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                let nextBridge = try CoreBridge(
                    backendType: backend,
                    socket: socket,
                    session: session
                )
                DispatchQueue.main.async {
                    guard let self else {
                        nextBridge.shutdown()
                        return
                    }
                    self.bridge.shutdown()
                    self.bridge = nextBridge
                    self.terminalManager.updateBridge(nextBridge)
                    self.lastSnapshot = FrameSnapshot()
                    self.pendingActiveTab = nil
                    self.needsLayoutReload = true
                    self.refreshUI()
                    self.focusActiveTerminal()
                }
            } catch {
                DispatchQueue.main.async { [weak self] in
                    self?.showError(error)
                }
            }
        }
    }

    private func closeActiveTab() {
        guard lastSnapshot.tabs.contains(where: { $0.id == lastSnapshot.activeTab }) else { return }
        let tabID = lastSnapshot.activeTab
        guard bridge.execute(task: MuxTask.closeTab(tabID)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCloseTab, arguments: ["id": "\(tabID)"]))
            return
        }
        needsLayoutReload = true
    }

    private func showError(_ error: Error) {
        reportStatusError(error.localizedDescription)
        guard let ownerWindow = window else { return }
        let alert = NSAlert()
        alert.messageText = MuxtermI18n.shared.tr(.errorPaletteFailed)
        alert.informativeText = error.localizedDescription
        alert.alertStyle = .warning
        alert.beginSheetModal(for: ownerWindow)
    }

    // MARK: - 事件循环

    private func startPolling() {
        let timer = Timer(timeInterval: FlatChrome.eventPollInterval, repeats: true) { [weak self] _ in
            self?.pollOnce()
        }
        RunLoop.main.add(timer, forMode: .common)
        pollTimer = timer
    }

    private func pollOnce() {
        let events = bridge.pollEvents()
        if let error = bridge.takeError() {
            reportStatusError(error)
        }
        var outputSeen = false
        var uiStateChanged = false
        for ev in events {
            if ev.isPaneOutput {
                terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                outputSeen = true
            } else if StateEventPolicy.requiresLayoutReload(ev.type) {
                uiStateChanged = true
                needsLayoutReload = true
                if ev.type == STATE_ACTIVE_TAB_CHANGED, pendingActiveTab == ev.tabId {
                    pendingActiveTab = nil
                }
            } else if StateEventPolicy.changesActivePane(ev.type) {
                uiStateChanged = true
            } else if ev.isBackendStatus {
                uiStateChanged = true
            } else if ev.type == STATE_TAB_RENAMED || ev.type == STATE_PANE_RESIZED {
                // 标题/字符格尺寸会改变状态栏或焦点，但不会改变布局树。
                uiStateChanged = true
            }
            if ev.isBackendStatus, ev.paneId == 4 {
                // pane_id 复用状态码：4 = exited
                closeSessionWindow()
                return
            }
        }
        if needsLayoutReload || uiStateChanged {
            refreshUI()
            if uiStateChanged {
                maybeCloseIfSessionEnded()
            }
        } else if outputSeen {
            content.statusBar.update(snapshot: lastSnapshot)
        }
    }

    private func refreshUI() {
        let snap = bridge.snapshot()
        lastSnapshot = snap
        content.tabBar.update(tabs: snap.tabs)
        if needsLayoutReload, pendingActiveTab == nil {
            if content.paneLayout.apply(layout: snap.layout, panes: snap.panes) {
                needsLayoutReload = false
                content.statusBar.clearLayoutSyncError()
            } else {
                content.statusBar.showLayoutSyncing()
            }
        }
        content.statusBar.update(snapshot: snap)
        content.statusBar.updateOutputSnippet(terminalManager.recentOutputSnippet)

        if let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id {
            let view = terminalManager.view(for: activePane)
            terminalManager.focusTarget = view
            content.paneLayout.markActivePane(activePane)
            if window?.firstResponder !== view {
                window?.makeFirstResponder(view)
            }
        }
    }

    private func refreshLocalizedUI() {
        commandPalette.refreshLocalization()
        content.refreshLocalization()
        refreshUI()
    }

    /// session/window 已空时关闭 NSWindow。
    private func maybeCloseIfSessionEnded() {
        let snap = bridge.snapshot()
        if snap.tabs.isEmpty && snap.panes.isEmpty {
            closeSessionWindow()
        }
    }

    private func reportStatusError(_ message: String) {
        content.statusBar.showError(message)
    }

    private func closeSessionWindow() {
        guard !isClosing else { return }
        isClosing = true
        pollTimer?.invalidate()
        pollTimer = nil
        bridge.shutdown()
        window?.close()
    }

    /// 通过 core 的独立 detach FFI 关闭控制 client，保留 tmux session。
    private func detachSessionWindow() {
        guard terminalManager.usesClientResize else { return }
        guard !isClosing else { return }
        guard bridge.detach() == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
            return
        }
        isClosing = true
        pollTimer?.invalidate()
        pollTimer = nil
        // Task::Detach 已关闭 control channel；这里仅回收 core handle，
        // 不会再次发送 detach-client 或杀 tmux session。
        bridge.shutdown()
        window?.close()
    }

    // MARK: - 快捷键

    private func installKeyEquivalents() {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handleKey(event) ? nil : event
        }
    }

    /// 返回 true 表示已消费事件。
    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // macOS 的 Delete/Backspace 可能在 SwiftTerm 的 NSTextInputClient 路径
        // 中被吞掉；明确转成 DEL，保证 shell 和 tmux 收到基础编辑键。
        if event.keyCode == 51,
           !flags.contains(.command),
           !flags.contains(.option),
           let view = window?.firstResponder as? MuxTerminalView
        {
            terminalManager.sendRawInput(to: view, byte: TerminalInputEncoding.backspaceByte)
            return true
        }
        guard let raw = event.charactersIgnoringModifiers, let key = raw.first.map(String.init) else {
            return false
        }
        let chord = KeyChord(
            command: flags.contains(.command),
            shift: flags.contains(.shift),
            option: flags.contains(.option),
            control: flags.contains(.control),
            key: key
        )
        guard let action = KeyBindings.action(for: chord) else {
            // Ctrl+C/D/L 等不是 Muxterm 的窗口快捷键时，窗口级 monitor 先
            // 把它们送成真实控制字节。这样不依赖 SwiftTerm 的 NSText
            // interpretation，也不会把 tmux 的 WriteRaw 内容变成字面文本。
            if flags.contains(.control), !flags.contains(.command), !flags.contains(.option),
               let view = window?.firstResponder as? MuxTerminalView,
               let byte = TerminalInputEncoding.controlByte(for: key)
            {
                terminalManager.sendRawInput(to: view, byte: byte)
                return true
            }
            return false
        }
        switch action {
        case .newTab:
            newTab()
        case .splitHorizontal:
            splitActivePane(horizontal: true)
        case .splitVertical:
            splitActivePane(horizontal: false)
        case .closeWindow:
            closeActiveWindow()
        case .closePane:
            closeActivePane()
        case .switchTab(let n):
            switchToTabIndex(n)
        case .nextPane:
            nextPane()
        case .prevPane:
            prevPane()
        case .commandPalette:
            openCommandPalette()
        case .quit:
            NSApp.terminate(nil)
        }
        return true
    }

    func windowWillClose(_ notification: Notification) {
        if !isClosing {
            isClosing = true
            pollTimer?.invalidate()
            pollTimer = nil
            bridge.shutdown()
        }
    }
}
