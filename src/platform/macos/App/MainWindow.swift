import AppKit
import CMuxterm
import MuxtermChrome

/// 主窗口：持有 CoreBridge + Timer 轮询 `muxterm_poll_events`，分发到 UI。
final class MainWindowController: NSWindowController, NSWindowDelegate {
    /// Project 连接流程的引用包装：异步回调需要可变状态；用
    /// `activeProjectFlow` 身份比较防止旧连接的回调覆盖新连接。
    private final class ProjectConnectFlowBox {
        var flow: ProjectConnectFlow

        init(flow: ProjectConnectFlow) {
            self.flow = flow
        }
    }

    private var bridge: CoreBridge
    private var terminalManager: TerminalManager
    private let content: ContentView
    private let discovery = ConnectionDiscovery()
    private var commandPalette: CommandPaletteController!
    private var quickConnect: QuickConnectController!
    /// 来自 ~/.config/muxterm/config.toml 的自定义快捷键（可选）。
    private var customKeybindings: [KeyChord: KeyAction] = [:]
    private let quickConnectStore: QuickConnectStore
    private var pollTimer: Timer?
    private var lastSnapshot = FrameSnapshot()
    private var needsLayoutReload = true
    /// tmux tab 切换确认门禁：外部关闭 / 快照缺失 / 超时都会放行。
    private var tabSwitchGate = TabSwitchGate()
    private var isClosing = false
    private var languageObserver: NSObjectProtocol?
    /// 已向 tmux 上报过颜色的 pane（`refresh-client -r` 只需每个 pane 一次；
    /// 外观变化时清空重报）。
    private var reportedColourPanes = Set<UInt32>()
    /// 最近一次 status bar 快照（用于周期刷新与位置/样式渲染）。
    private var statusBarSnapshot: StatusBarSnapshot?
    private var statusRefreshTimer: Timer?
    private var lastStatusFetchAt = Date.distantPast
    /// 结构事件后的 status bar 刷新（合并同一轮事件，避免 resize 风暴逐帧查询）。
    private var statusRefreshWorkItem: DispatchWorkItem?
    private var activeProjectFlow: ProjectConnectFlowBox?
    /// Warm connection pool：已使用过的 QuickConnect 目标切换时不立即关闭，
    /// 后台连接继续 poll；按 LRU/TTL/memory pressure 淘汰。
    private let connectionPool: ConnectionPool<WarmConnectionSlot>
    /// 终端字体配置（config.toml `[font]`；Cmd +/- 缩放时保留 family）。
    private var terminalFontSettings: MuxtermTerminalFont.Settings
    /// Cmd +/- / Cmd 0 的字号持久化键（用户偏好覆盖 config 基础字号）。
    private static let fontSizePreferenceKey = "muxterm.terminalFontSize"
    /// 运行时主题持久化键（用户偏好覆盖 config `[theme] name`）。
    private static let themePreferenceKey = "muxterm.theme"
    /// 运行时 status bar 模式持久化键（覆盖 config `[statusbar] mode`）。
    private static let statusBarModePreferenceKey = "muxterm.statusbarMode"

    private static func savedTerminalFontSize() -> CGFloat? {
        guard let saved = UserDefaults.standard.object(forKey: fontSizePreferenceKey) as? Double else {
            return nil
        }
        return CGFloat(saved)
    }

    private func currentTerminalFontSize() -> CGFloat {
        Self.savedTerminalFontSize() ?? terminalFontSettings.size
    }

    init(bridge: CoreBridge, debug: Bool = false) {
        let toml = try? String(contentsOf: KeyBindingsConfig.defaultConfigURL, encoding: .utf8)
        if let toml {
            customKeybindings = KeyBindingsConfig.parse(toml: toml)
        }
        // 终端调色板跟随 [theme] name（默认浅色；运行期切换会覆盖）。
        let configTheme = toml.flatMap { MuxtermTerminalColors.themeName(from: $0) }
        let savedTheme = UserDefaults.standard.string(forKey: Self.themePreferenceKey)
        MuxtermTerminalColors.activePalette = MuxtermTheme.from(
            name: savedTheme ?? configTheme
        ).palette
        let baseFont = MuxtermTerminalFont.settings(from: toml)
        let savedSize = Self.savedTerminalFontSize() ?? baseFont.size
        terminalFontSettings = MuxtermTerminalFont.Settings(
            family: baseFont.family,
            size: MuxtermTerminalFont.clamp(savedSize)
        )
        self.bridge = bridge
        self.terminalManager = TerminalManager(
            bridge: bridge,
            fontFamily: terminalFontSettings.family,
            fontSize: terminalFontSettings.size
        )
        self.content = ContentView(terminalManager: terminalManager)
        content.connectionStatus.isDebug = debug
        // status bar 配色来源：默认 GUI 黑白；`[statusbar] color_mode = "tmux"`
        // 时完全采用 tmux 样式。
        content.statusBar.colorMode = Self.currentStatusBarMode(
            configToml: toml
        )
        connectionPool = ConnectionPool(
            policy: ConnectionPoolPolicy(maxSlots: MuxtermConfig.poolMaxSlots(from: toml))
        )

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

        // QuickConnect 持久化：存到 ~/.config/muxterm/quickconnect.toml（TOML，
        // 与主 config.toml 同一目录，方便用户手改/备份）。
        let configDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/muxterm", isDirectory: true)
        quickConnectStore = QuickConnectStore(
            fileURL: configDir.appendingPathComponent("quickconnect.toml")
        )
        super.init(window: window)
        window.delegate = self

        commandPalette = CommandPaletteController(ownerWindow: window)
        commandPalette.onSelect = { [weak self] item in
            self?.handlePaletteSelection(item)
        }
        quickConnect = QuickConnectController(store: quickConnectStore, ownerWindow: window)
        quickConnect.onConnect = { [weak self] config in
            self?.connect(config: config)
        }
        quickConnect.onNewProject = { [weak self] in
            self?.editProject(nil)
        }
        quickConnect.onEditProject = { [weak self] config in
            self?.editProject(config)
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
        content.statusBar.onSelectWindow = { [weak self] tabId in
            self?.requestSwitchTab(tabId)
        }
        terminalManager.onOutputSnippetChanged = { [weak self] snippet in
            self?.content.connectionStatus.updateOutputSnippet(snippet)
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
            connectionPool.shutdownAll()
            bridge.shutdown()
        }
        statusRefreshTimer?.invalidate()
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

    /// 当前 pane 全屏切换：tmux/ssh 发 `resize-pane -Z`，本地 shell 用布局全屏。
    @objc func toggleActivePaneFullscreen() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        else {
            return
        }
        if terminalManager.usesClientResize {
            if bridge.execute(task: MuxTask.togglePaneFullscreen(pane)) != 0 {
                reportStatusError(
                    MuxtermI18n.shared.tr(.errorCommandFailed)
                )
            }
        } else {
            content.paneLayout.toggleFullscreen(paneId: pane)
        }
    }

    @objc func increaseTerminalFontSize(_ sender: Any?) {
        adjustTerminalFontSize(delta: 1)
    }

    @objc func decreaseTerminalFontSize(_ sender: Any?) {
        adjustTerminalFontSize(delta: -1)
    }

    @objc func resetTerminalFontSize(_ sender: Any?) {
        UserDefaults.standard.removeObject(forKey: Self.fontSizePreferenceKey)
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: terminalFontSettings.size,
            container: content.paneLayout
        )
    }

    private func adjustTerminalFontSize(delta: Int) {
        let current = currentTerminalFontSize()
        let next = MuxtermTerminalFont.zoomed(current, direction: delta)
        guard next != current else { return }
        UserDefaults.standard.set(Double(next), forKey: Self.fontSizePreferenceKey)
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: next,
            container: content.paneLayout
        )
    }

    /// 当前主题：运行期选择优先，其次 config `[theme] name`，缺省浅色。
    private func currentTheme() -> MuxtermTheme {
        let saved = UserDefaults.standard.string(forKey: Self.themePreferenceKey)
        let config = (try? String(contentsOf: KeyBindingsConfig.defaultConfigURL, encoding: .utf8))
            .flatMap { MuxtermTerminalColors.themeName(from: $0) }
        return MuxtermTheme.from(name: saved ?? config)
    }

    /// 应用主题并持久化：更新终端默认色、重报 tmux 颜色，命令面板标题会
    /// 在下次打开时显示当前主题。
    private func applyTheme(_ theme: MuxtermTheme) {
        UserDefaults.standard.set(theme.rawValue, forKey: Self.themePreferenceKey)
        MuxtermTerminalColors.activePalette = theme.palette
        terminalManager.applyTheme(
            fgHex: theme.palette.fg,
            bgHex: theme.palette.bg
        )
        // 主题色变化后必须给**所有** pane 重新上报，tmux 才会用新颜色代答
        // OSC 10/11；只报当前 tab 会让后台 tab 的 codex 输入框沿用旧色。
        reportedColourPanes.removeAll()
        _ = bridge.reportAllPaneColours(
            fgHex: theme.palette.fg,
            bgHex: theme.palette.bg
        )
        // 重新渲染 status bar（GUI 黑白模式跟随主题；tmux 模式样式不变）。
        if let snapshot = statusBarSnapshot {
            content.statusBar.apply(snapshot: snapshot)
        }
    }

    private func toggleTheme() {
        let next: MuxtermTheme = currentTheme() == .light ? .dark : .light
        applyTheme(next)
        commandPalette.update(
            items: rootPaletteItems(),
            placeholder: MuxtermI18n.shared.tr(.commandPalette)
        )
    }

    /// 当前 status bar 模式：运行期选择优先，其次 config，缺省 tmux。
    private static func currentStatusBarMode(configToml: String?) -> StatusBarMode {
        let saved = UserDefaults.standard.string(forKey: statusBarModePreferenceKey)
        if let saved, let mode = StatusBarMode(rawValue: saved) {
            return mode
        }
        return StatusBarMode.from(toml: configToml)
    }

    private func toggleStatusBarMode() {
        let next: StatusBarMode = content.statusBar.colorMode == .tmux ? .theme : .tmux
        UserDefaults.standard.set(next.rawValue, forKey: Self.statusBarModePreferenceKey)
        content.statusBar.colorMode = next
        if let snapshot = statusBarSnapshot {
            content.statusBar.apply(snapshot: snapshot)
        }
        commandPalette.update(
            items: rootPaletteItems(),
            placeholder: MuxtermI18n.shared.tr(.commandPalette)
        )
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

    @objc func openQuickConnect() {
        guard let quickConnect else { return }
        // Recent 由连接池派生（最近打开且仍 warm 的连接）；当前连接用于行高亮。
        quickConnect.currentConfig = connectionPool.currentTargetConfig
        quickConnectStore.replaceRecents(connectionPool.recentTargetConfigs())
        if quickConnect.window?.isKeyWindow == true {
            quickConnect.dismiss()
        } else {
            quickConnect.present()
        }
    }

    /// 按 QuickConnect 目标连接：tmux 有 name → attach，无 name → 创建；
    /// shell runtime → 本地/远程 shell 在 path 启动。
    private func connect(config: TargetConfig) {
        quickConnect.dismiss()
        // recents 由连接池派生：连接成功后 pool.acquire 会更新最近列表。
        switch config.runtime {
        case .tmux:
            connectProject(config: config)
        case .shell:
            switch config.transport {
            case .local:
                startShell(config: config)
            case .ssh:
                // 远程 shell 没有独立裸 shell 后端：按 Project 语义走
                // attach 已有 tmux session → 失败创建 → attach。
                connectProject(config: config)
            }
        }
    }

    /// Project 连接流程：先 attach 已有 session；明确失败后创建 detached
    /// session（twork 语义：session 名 = 显式 name / path basename），
    /// 创建成功后再 attach 同一 session。local / ssh 共用同一状态机。
    private func connectProject(config: TargetConfig) {
        let box = ProjectConnectFlowBox(flow: ProjectConnectFlow(config: config))
        activeProjectFlow = box
        runProjectFlow(box, config: config)
    }

    private func runProjectFlow(_ box: ProjectConnectFlowBox, config: TargetConfig) {
        switch box.flow.state {
        case .attachExisting:
            attachTmux(config: config, session: box.flow.session) { [weak self] result in
                guard let self, self.activeProjectFlow === box else { return }
                switch result {
                case .success:
                    box.flow.attachExistingSucceeded()
                    self.activeProjectFlow = nil
                    // attachTmux 内部已通过 connectionPool 激活 slot 并切换渲染。
                case .failure(let error):
                    box.flow.attachExistingFailed(message: error.localizedDescription)
                    self.runProjectFlow(box, config: config)
                }
            }
        case .createDetached:
            let target: ConnectionTarget
            switch config.transport {
            case .local:
                target = .local
            case .ssh(let name):
                target = .ssh(SSHHostInfo(alias: name, hostname: "", user: nil, port: nil))
            }
            discovery.createSession(
                named: box.flow.session,
                target: target,
                directory: box.flow.directory
            ) { [weak self] result in
                guard let self, self.activeProjectFlow === box else { return }
                switch result {
                case .success:
                    box.flow.createSucceeded()
                    self.runProjectFlow(box, config: config)
                case .failure(let error):
                    box.flow.createFailed(message: error.localizedDescription)
                    self.activeProjectFlow = nil
                    self.showError(error, prefix: "create session failed")
                }
            }
        case .attachCreated:
            attachTmux(config: config, session: box.flow.session) { [weak self] result in
                guard let self, self.activeProjectFlow === box else { return }
                switch result {
                case .success:
                    box.flow.attachCreatedSucceeded()
                    self.activeProjectFlow = nil
                    // attachTmux 内部已通过 connectionPool 激活 slot 并切换渲染。
                case .failure(let error):
                    box.flow.attachCreatedFailed(message: error.localizedDescription)
                    self.activeProjectFlow = nil
                    self.showError(error, prefix: "attach created session failed")
                }
            }
        case .done:
            break
        case .failed(let failure):
            let prefix: String
            switch failure.stage {
            case .attachExisting: prefix = "attach existing session failed"
            case .create: prefix = "create session failed"
            case .attachCreated: prefix = "attach created session failed"
            }
            reportStatusError("\(prefix): \(failure.detail)")
        }
    }

    private func attachTmux(
        config: TargetConfig,
        session: String,
        completion: @escaping (Result<CoreBridge, Error>) -> Void
    ) {
        let key = Self.connectionKey(config: config, session: session)
        // 先尝试复用 warm slot：命中则直接切换渲染，不重复建连。
        if let slot = connectionPool.slots[key], slot.lifecycle != .evicting {
            activate(slot: slot)
            completion(.success(slot.bridge))
            return
        }

        let backend: String
        let socket: String?
        switch config.transport {
        case .local:
            backend = "tmux"
            socket = nil
        case .ssh(let name):
            backend = "ssh"
            socket = name
        }
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
                    let slot = WarmConnectionSlot(key: key, bridge: nextBridge, now: 0)
                    self.activate(slot: slot)
                    completion(.success(nextBridge))
                }
            } catch {
                DispatchQueue.main.async {
                    completion(.failure(error))
                }
            }
        }
    }

    private func startShell(config: TargetConfig) {
        // 本地 shell 用指定目录启动（muxterm_new_connect 的 local 分支带 workdir）。
        switch config.transport {
        case .local:
            let sessionName = QuickConnect.defaultName(for: config.path)
            let key = ConnectionKey(
                transport: "local",
                alias: nil,
                session: sessionName,
                runtime: "shell",
                path: config.path
            )
            if let slot = connectionPool.slots[key], slot.lifecycle != .evicting {
                activate(slot: slot)
                return
            }
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                do {
                    let nextBridge = try CoreBridge.connect(
                        backendType: "local",
                        startDirectory: config.path
                    )
                    DispatchQueue.main.async {
                        guard let self else {
                            nextBridge.shutdown()
                            return
                        }
                        let slot = WarmConnectionSlot(key: key, bridge: nextBridge, now: 0)
                        self.activate(slot: slot)
                    }
                } catch {
                    DispatchQueue.main.async { [weak self] in
                        self?.showError(error)
                    }
                }
            }
        case .ssh:
            break // 已在 connect(config:) 中走 connectProject
        }
    }

    /// 从 TargetConfig + session 构造 pool key（连接身份）。
    private static func connectionKey(
        config: TargetConfig,
        session: String
    ) -> ConnectionKey {
        let alias: String?
        if case .ssh(let name) = config.transport {
            alias = name
        } else {
            alias = nil
        }
        return ConnectionKey(
            transport: config.transport.isSSH ? "ssh" : "local",
            alias: alias,
            session: session,
            runtime: config.runtime.rawValue,
            path: config.path
        )
    }

    /// 激活一个 warm slot：替换 bridge / TerminalManager / PaneLayout 的渲染源。
    /// 旧 slot 由 ConnectionPool.acquire 自动降为 background，不 shutdown。
    private func activate(slot: WarmConnectionSlot) {
        let oldBridge = bridge
        connectionPool.acquire(key: slot.key) { _ in slot }
        bridge = slot.bridge
        terminalManager = slot.terminalManager
        content.paneLayout.replaceTerminalManager(slot.terminalManager)
        // warm slot 的 TerminalManager 各自保存字体状态：切回时沿用当前字号，
        // 避免旧 slot 还是切换前的小字体。
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: currentTerminalFontSize(),
            container: content.paneLayout
        )
        // warm slot 的视图也要沿用当前主题（浅/深色）。
        terminalManager.applyTheme(
            fgHex: MuxtermTerminalColors.activePalette.fg,
            bgHex: MuxtermTerminalColors.activePalette.bg
        )
        // 连接建立/切换后给全部 pane 上报一次颜色，避免后台 tab 的 codex
        // 输入框使用 tmux 默认（或上一个连接）的颜色代答。
        _ = bridge.reportAllPaneColours(
            fgHex: MuxtermTerminalColors.activePalette.fg,
            bgHex: MuxtermTerminalColors.activePalette.bg
        )
        lastSnapshot = slot.lastSnapshot
        // 切连接后旧 status bar 属于上一个 tmux：先清掉，等新快照到达再显示。
        statusBarSnapshot = nil
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        content.applyStatusBar(nil)
        reportedColourPanes.removeAll()
        tabSwitchGate = TabSwitchGate()
        needsLayoutReload = true
        refreshUI()
        focusActiveTerminal()
        refreshStatusBar(force: true)
        // 若旧 bridge 不在 pool（初始连接或非 pool 路径），切走后直接回收；
        // pool 内的旧 slot 由 acquire 降为 background，保持 warm。
        let oldIsPooled = connectionPool.slots.values.contains { $0.bridge === oldBridge }
        if !oldIsPooled, oldBridge !== slot.bridge {
            DispatchQueue.global(qos: .utility).async {
                oldBridge.shutdown()
            }
        }
    }

    /// 打开/编辑 project 配置窗口。
    private func editProject(_ config: TargetConfig?) {
        // 配置窗口以 sheet 形式出现，先收起 Cmd-P 面板，避免遮盖。
        quickConnect.dismiss()
        let hosts: [SSHHostInfo]
        switch discovery.sshHosts() {
        case .success(let value):
            hosts = value
        case .failure:
            hosts = []
        }
        let win = TargetConfigWindow(
            editing: config,
            owner: window,
            store: quickConnectStore,
            sshHosts: hosts
        )
        win.onSave = { [weak self] saved in
            self?.quickConnectStore.upsertProject(saved)
            // 保存后重新打开面板，方便继续连接/编辑。
            self?.quickConnect.present()
        }
        win.onCancel = { [weak self] in
            // 取消/关闭后恢复面板。
            self?.quickConnect.present()
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
        tabSwitchGate.request(tab: tabId)
        needsLayoutReload = true
        guard bridge.execute(task: MuxTask.switchTab(tabId)) == 0 else {
            tabSwitchGate = TabSwitchGate()
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
                title: i18n.tr(.togglePaneFullscreen),
                detail: i18n.tr(.togglePaneFullscreenDetail),
                keywords: "pane fullscreen zoom toggle 全屏 放大",
                kind: .command(.togglePaneFullscreen)
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
                title: i18n.tr(
                    .themeSwitchTo,
                    arguments: ["theme": currentTheme() == .light ? "Dark" : "Light"]
                ),
                detail: i18n.tr(.themeDetail, arguments: ["theme": currentTheme().displayName]),
                keywords: "theme light dark 主题 浅色 深色",
                kind: .command(.theme)
            ),
            PaletteItem(
                title: i18n.tr(
                    .statusBarModeSwitchTo,
                    arguments: ["mode": content.statusBar.colorMode == .tmux ? "Theme" : "Tmux"]
                ),
                detail: i18n.tr(
                    .statusBarModeDetail,
                    arguments: ["mode": content.statusBar.colorMode == .tmux ? "Tmux" : "Theme"]
                ),
                keywords: "statusbar status bar mode tmux theme 状态栏 模式",
                kind: .command(.statusBarMode)
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
        case .command(.togglePaneFullscreen):
            commandPalette.dismiss()
            toggleActivePaneFullscreen()
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
        case .command(.theme):
            toggleTheme()
        case .command(.statusBarMode):
            toggleStatusBarMode()
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
        let alias: String?
        switch target {
        case .local:
            backend = "tmux"
            socket = nil
            alias = nil
        case .ssh(let host):
            backend = "ssh"
            socket = host.alias
            alias = host.alias
        }
        let key = ConnectionKey(
            transport: alias == nil ? "local" : "ssh",
            alias: alias,
            session: session,
            runtime: "tmux",
            path: ""
        )
        if let slot = connectionPool.slots[key], slot.lifecycle != .evicting {
            activate(slot: slot)
            return
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
                    let slot = WarmConnectionSlot(key: key, bridge: nextBridge, now: 0)
                    self.activate(slot: slot)
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

    private func showError(_ error: Error, prefix: String? = nil) {
        let detail = prefix.map { "\($0): \(error.localizedDescription)" }
            ?? error.localizedDescription
        reportStatusError(detail)
        guard let ownerWindow = window else { return }
        let alert = NSAlert()
        alert.messageText = MuxtermI18n.shared.tr(.errorPaletteFailed)
        alert.informativeText = detail
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
        terminalManager.beginEventBatch()
        defer { terminalManager.endEventBatch() }
        connectionPool.pollBackgroundSlots()
        let events = bridge.pollEvents()
        if let error = bridge.takeError() {
            reportStatusError(error)
        }
        var outputSeen = false
        var uiStateChanged = false
        let deferOutputs = EventBatchPlan.hasStructuralEvent(
            types: events.map(\.type),
            requiresLayoutReload: StateEventPolicy.requiresLayoutReload
        )
        var pendingOutputs: [(paneId: UInt32, data: Data)] = []
        for ev in events {
            if ev.isPaneClosed {
                // pane 真正关闭才销毁视图；切 tab / 布局变化保留视图状态。
                terminalManager.removePane(ev.paneId)
            } else if ev.isPaneOutput {
                // 同批有结构事件（如窗口 resize 的 %layout-change）时，htop
                // 的新尺寸重绘帧会先于模型 resize 到达，必须先收集、等布局
                // 同步完再喂；纯输出批次直接喂，避免额外延迟。
                if deferOutputs {
                    pendingOutputs.append((paneId: ev.paneId, data: ev.data))
                } else {
                    terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                }
                outputSeen = true
            } else if StateEventPolicy.requiresLayoutReload(ev.type) {
                uiStateChanged = true
                needsLayoutReload = true
                if ev.type == STATE_ACTIVE_TAB_CHANGED {
                    tabSwitchGate.onTabChanged(to: ev.tabId)
                } else if ev.type == STATE_TAB_CLOSED {
                    tabSwitchGate.onTabClosed(ev.tabId)
                }
            } else if StateEventPolicy.changesActivePane(ev.type) {
                uiStateChanged = true
            } else if ev.isBackendStatus {
                uiStateChanged = true
            } else if ev.type == STATE_TAB_RENAMED {
                // 标题会改变状态栏或焦点，但不会改变布局树。
                uiStateChanged = true
            } else if ev.type == STATE_PANE_RESIZED {
                // 标题/字符格尺寸改变只影响状态栏/焦点，不改变布局树。
                // 模型尺寸跟随 SwiftTerm 视图像素自适应（syncSizeToPty），
                // 不能用 tmux 报告的 PaneInfo 强制设置——resize 时 tmux 对
                // 后台窗口的尺寸滞后，强制设置会造成模型与视图错位（黑框）。
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
            content.connectionStatus.update(snapshot: lastSnapshot)
            // 颜色上报只依赖 refreshUI 时，attach 后没有结构事件就永远不会
            // 触发（日志里没有 refresh-client -r 的原因）。纯输出也要补报。
            reportPaneColoursIfNeeded(lastSnapshot.panes)
        }
        if needsLayoutReload {
            // 结构事件（窗口增删/重命名/布局变化）后刷新 status bar；
            // 走防抖调度，避免 2s 节流把切 tab 后的高亮更新吞掉。
            scheduleStatusBarRefresh()
        }
        // 布局/尺寸同步完成后再喂输出，避免 resize 竞态。
        for item in pendingOutputs {
            terminalManager.handleOutput(paneId: item.paneId, data: item.data)
        }
    }

    private func refreshUI() {
        let snap = bridge.snapshot()
        lastSnapshot = snap
        terminalManager.updatePaneSizes(snap.panes)
        // 请求切换的 tab 已不存在（shell 退出/外部关闭）：立即放行门禁。
        tabSwitchGate.onSnapshot(tabs: snap.tabs.map(\.id))
        reportPaneColoursIfNeeded(snap.panes)
        content.tabBar.update(tabs: snap.tabs)
        if needsLayoutReload, tabSwitchGate.isReleased() {
            if content.paneLayout.apply(layout: snap.layout, panes: snap.panes) {
                needsLayoutReload = false
                content.connectionStatus.clearLayoutSyncError()
            } else {
                content.connectionStatus.showLayoutSyncing()
            }
        }
        content.connectionStatus.update(snapshot: snap)
        content.connectionStatus.updateOutputSnippet(terminalManager.recentOutputSnippet)

        if let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id {
            let view = terminalManager.view(for: activePane)
            terminalManager.focusTarget = view
            content.paneLayout.markActivePane(activePane)
            // 只在视图已挂进窗口且焦点确实不同时才切换，避免对未挂载视图
            // 反复 makeFirstResponder 触发 IMK mach port 报错和切换卡顿。
            if view.window != nil, window?.firstResponder !== view {
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
        content.connectionStatus.showError(message)
    }

    /// 新出现的 pane 需要把客户端主题色上报给 tmux，否则 tmux 代答
    /// OSC 10/11 颜色查询时用的是自己的默认色板（codex 黑底黑字/白底白字）。
    private func reportPaneColoursIfNeeded(_ panes: [Pane]) {
        guard terminalManager.usesClientResize else { return }
        let fresh = Set(panes.map(\.id)).subtracting(reportedColourPanes)
        guard !fresh.isEmpty, let colors = terminalManager.themeHexColors() else { return }
        for id in fresh {
            if bridge.reportPaneColours(paneId: id, fgHex: colors.fg, bgHex: colors.bg) == 0 {
                reportedColourPanes.insert(id)
            }
        }
    }

    /// 抓取并应用 tmux status bar 快照（只读查询，后台执行）。
    private func refreshStatusBar(force: Bool) {
        guard terminalManager.usesClientResize else { return }
        if !force, Date().timeIntervalSince(lastStatusFetchAt) < 2 {
            return
        }
        lastStatusFetchAt = Date()
        let bridge = self.bridge
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let json = bridge.statusBarSnapshotJSON(),
                  let data = json.data(using: .utf8),
                  let response = try? JSONDecoder().decode(StatusBarResponse.self, from: data),
                  response.ok,
                  let snapshot = response.status
            else {
                return
            }
            DispatchQueue.main.async {
                guard let self else { return }
                // 查询期间用户可能已经切走连接：旧连接的快照不能覆盖新连接。
                guard self.bridge === bridge else { return }
                self.statusBarSnapshot = snapshot
                self.content.applyStatusBar(snapshot)
                self.scheduleStatusRefresh(snapshot)
            }
        }
    }

    /// 结构事件（切 tab / 建删窗口 / 布局变化）后防抖刷新 status bar：
    /// 同一轮事件只触发一次查询，且不受 2s 周期节流限制。
    private func scheduleStatusBarRefresh() {
        guard terminalManager.usesClientResize else { return }
        statusRefreshWorkItem?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.statusRefreshWorkItem = nil
            self.refreshStatusBar(force: true)
        }
        statusRefreshWorkItem = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
    }

    /// 按 tmux `status-interval` 周期刷新（时钟/时间类 right 段需要）。
    private func scheduleStatusRefresh(_ snapshot: StatusBarSnapshot) {
        statusRefreshTimer?.invalidate()
        guard snapshot.enabled else { return }
        let interval = TimeInterval(max(5, Int(snapshot.interval)))
        let timer = Timer(timeInterval: interval, repeats: true) { [weak self] _ in
            self?.refreshStatusBar(force: true)
        }
        RunLoop.main.add(timer, forMode: .common)
        statusRefreshTimer = timer
    }

    private func closeSessionWindow() {
        guard !isClosing else { return }
        isClosing = true
        pollTimer?.invalidate()
        pollTimer = nil
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        statusRefreshWorkItem?.cancel()
        statusRefreshWorkItem = nil
        connectionPool.shutdownAll()
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
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        statusRefreshWorkItem?.cancel()
        statusRefreshWorkItem = nil
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
            // 输入法候选态（marked text）：Backspace 必须交给 IME 处理，
            // 否则会把 DEL 发给终端，误删输入框里已经提交的原文。
            if view.hasMarkedText() {
                return false
            }
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
        guard let action = KeyBindings.action(for: chord, custom: customKeybindings) else {
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
        case .quickConnect:
            openQuickConnect()
        case .quit:
            NSApp.terminate(nil)
        case .increaseFontSize:
            increaseTerminalFontSize(nil)
        case .decreaseFontSize:
            decreaseTerminalFontSize(nil)
        case .resetFontSize:
            resetTerminalFontSize(nil)
        case .togglePaneFullscreen:
            toggleActivePaneFullscreen()
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
