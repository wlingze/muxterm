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

        content.tabBar.onSelectTab = { [weak self] tabId in
            self?.requestSwitchTab(tabId)
        }
        content.tabBar.onNewTab = { [weak self] in
            self?.newTab()
        }
        content.paneLayout.onActivatePane = { [weak self] paneId in
            guard let self else { return }
            if self.bridge.execute(task: MuxTask.switchPane(paneId)) != 0 {
                self.reportStatusError("切换 pane @\(paneId) 失败")
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
        if !isClosing {
            bridge.shutdown()
        }
    }

    // MARK: - 公开动作（菜单 / 快捷键）

    @objc func newTab() {
        guard bridge.execute(task: MuxTask.newTab()) == 0 else {
            reportStatusError("新建 tab 失败")
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
            reportStatusError("关闭 pane @\(pane) 失败")
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
            commandPalette.present(items: Self.rootPaletteItems())
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
            reportStatusError("切换 tab @\(tabId) 失败")
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
            reportStatusError("分割 pane @\(pane) 失败")
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
            reportStatusError("切换 pane @\(target) 失败")
            return
        }
    }

    // MARK: - 命令面板

    private static func rootPaletteItems() -> [PaletteItem] {
        [
            PaletteItem(
                title: "Local",
                detail: "选择本机 tmux session",
                keywords: "local tmux attach new",
                kind: .command(.local)
            ),
            PaletteItem(
                title: "SSH",
                detail: "选择 SSH 主机，再选择远程 tmux session",
                keywords: "ssh remote host tmux attach new",
                kind: .command(.ssh)
            ),
            PaletteItem(
                title: "New Tab",
                detail: "新建本地 tab",
                keywords: "new tab",
                kind: .command(.newTab)
            ),
            PaletteItem(
                title: "Split Pane Horizontally",
                detail: "水平分割当前 pane",
                keywords: "split pane horizontal",
                kind: .command(.splitHorizontal)
            ),
            PaletteItem(
                title: "Split Pane Vertically",
                detail: "竖直分割当前 pane",
                keywords: "split pane vertical",
                kind: .command(.splitVertical)
            ),
            PaletteItem(
                title: "Next Pane",
                detail: "切换到当前 tab 的下一个 pane",
                keywords: "pane next cmd bracket",
                kind: .command(.nextPane)
            ),
            PaletteItem(
                title: "Previous Pane",
                detail: "切换到当前 tab 的上一个 pane",
                keywords: "pane previous prev cmd bracket",
                kind: .command(.prevPane)
            ),
            PaletteItem(
                title: "Close Pane",
                detail: "关闭当前 pane",
                keywords: "close pane",
                kind: .command(.closePane)
            ),
            PaletteItem(
                title: "Close Tab",
                detail: "关闭当前 tab",
                keywords: "close tab",
                kind: .command(.closeTab)
            ),
            PaletteItem(
                title: "Close Window",
                detail: "关闭当前窗口",
                keywords: "close window",
                kind: .command(.closeWindow)
            ),
            PaletteItem(
                title: "Quit Muxterm",
                detail: "退出应用",
                keywords: "quit exit",
                kind: .command(.quit)
            ),
        ]
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
                placeholder: "选择 SSH 主机…"
            )
        case .failure(let error):
            commandPalette.dismiss()
            showError(error)
        }
    }

    private func showSessions(for target: ConnectionTarget) {
        commandPalette.update(
            items: [PaletteItem(
                title: "New",
                detail: "选择目录，创建 tmux session 并 attach",
                keywords: "new create tmux directory folder",
                kind: .newSession(target: target)
            )],
            placeholder: "选择 tmux session…"
        )

        let finish: (Result<[TmuxSessionInfo], Error>) -> Void = { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let sessions):
                let items = [PaletteItem(
                    title: "New",
                    detail: "选择目录，创建 tmux session 并 attach",
                    keywords: "new create tmux directory folder",
                    kind: .newSession(target: target)
                )] + sessions.map { session in
                    PaletteItem(
                        title: session.name,
                        detail: "\(session.windowCount) 个 window\(session.attached ? " · 已连接" : "")",
                        keywords: "tmux session attach",
                        kind: .session(target: target, name: session.name)
                    )
                }
                self.commandPalette.update(items: items, placeholder: "选择 tmux session…")
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
            reportStatusError("无法打开目录选择器：主窗口不可用")
            return
        }
        switch target {
        case .local:
            let panel = NSOpenPanel()
            panel.title = "选择新 tmux session 的工作目录"
            panel.message = "选择目录后，Muxterm 会创建 tmux session 并 attach。"
            panel.canChooseFiles = false
            panel.canChooseDirectories = true
            panel.allowsMultipleSelection = false
            panel.beginSheetModal(for: ownerWindow) { [weak self] response in
                guard response == .OK, let directory = panel.url?.path else { return }
                self?.createSession(target: target, directory: directory)
            }
        case .ssh(let host):
            let alert = NSAlert()
            alert.messageText = "选择远程工作目录"
            alert.informativeText = "输入 \(host.alias) 上新 tmux session 的目录。"
            let field = NSTextField(string: "~")
            field.frame = NSRect(x: 0, y: 0, width: 320, height: 24)
            alert.accessoryView = field
            alert.addButton(withTitle: "创建并连接")
            alert.addButton(withTitle: "取消")
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
            reportStatusError("关闭 tab @\(tabID) 失败")
            return
        }
        needsLayoutReload = true
    }

    private func showError(_ error: Error) {
        reportStatusError(error.localizedDescription)
        guard let ownerWindow = window else { return }
        let alert = NSAlert()
        alert.messageText = "命令面板操作失败"
        alert.informativeText = error.localizedDescription
        alert.alertStyle = .warning
        alert.beginSheetModal(for: ownerWindow)
    }

    // MARK: - 事件循环

    private func startPolling() {
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
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
            // Ctrl+C/L 等不是 Muxterm 的窗口快捷键时，窗口级 monitor 先
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
