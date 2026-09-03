import AppKit
import CMuxterm
import MuxtermChrome

/// 主窗口：持有 CoreBridge + Timer 轮询 `muxterm_poll_events`，分发到 UI。
final class MainWindowController: NSWindowController, NSWindowDelegate {
    private static let tabNumberTopologyEvents: Set<UInt32> = [
        STATE_TAB_ADDED,
        STATE_TAB_CLOSED,
        STATE_TAB_ORDER_CHANGED,
        STATE_PANE_ADDED,
        STATE_PANE_CLOSED,
    ]

    /// Project 连接流程的引用包装：异步回调需要可变状态；用
    /// `activeProjectFlow` 身份比较防止旧连接的回调覆盖新连接。
    private final class ProjectConnectFlowBox {
        var flow: ProjectConnectFlow

        init(flow: ProjectConnectFlow) {
            self.flow = flow
        }
    }

    private struct PendingSearchJump {
        let paneId: UInt32
        let seq: UInt64
        let query: String
    }

    /// Workspace 已经完成视觉切换，但目标 CoreBridge 仍被上一轮后台
    /// FFI 占用时的延迟激活记录。缓存路径先把窗口交给用户，锁释放后再
    /// 做一次权威刷新；generation 防止旧 Workspace 的回调覆盖新选择。
    private struct PendingForegroundActivation {
        let slot: WarmConnectionSlot
        let oldBridge: CoreBridge
        let restoredParkedTree: Bool
        let hasPendingSurfaceCatchUp: Bool
        let created: Bool
        let generation: UInt64
        let startedAt: TimeInterval
        let wasDeferred: Bool
    }

    /// 在 utility 队列完成的权威拓扑读取。主线程只应用这些值类型并挂载
    /// 已缓存的 AppKit 树，不再在 Workspace 点击回调里逐项询问远端。
    private struct ForegroundAuthoritySnapshot {
        let frame: FrameSnapshot
        let allPanes: [Pane]
        let tabIdsByPane: [UInt32: UInt32]
        let tabNumbersByPane: [UInt32: Int]
    }

    private struct CatalogConnection {
        let bridge: CoreBridge
        let target: TargetConfig
    }

    var bridge: CoreBridge
    var terminalManager: TerminalManager
    let content: ContentView
    let workspaceSidebar = WorkspaceSidebarView(frame: NSRect(x: 0, y: 0, width: 240, height: 640))
    private let mainSplitController = NSSplitViewController()
    private var sidebarSplitItem: NSSplitViewItem?
    private let sidebarToggleButton = NSButton()
    private var sidebarTitlebarAccessory: NSTitlebarAccessoryViewController?
    private let discovery = ConnectionDiscovery()
    var commandPalette: CommandPaletteController!
    var unifiedPanel: UnifiedPanelController!
    private var settingsWindow: SettingsWindowController?
    /// 来自 ~/.config/muxterm/config.toml 的自定义快捷键（可选）。
    private var customKeybindings: [KeyChord: KeyAction] = [:]
    private var nextWorkspaceOpenedOrder: UInt64 = 1
    private let quickConnectStore: QuickConnectStore
    private var pollTimer: Timer?
    /// 主窗口 local key monitor 的 token；独立 NSPanel 的事件不能进入这里。
    private var keyMonitor: Any?
    var lastSnapshot = FrameSnapshot()
    private var needsLayoutReload = true
    /// tmux tab 切换确认门禁：外部关闭 / 快照缺失 / 超时都会放行。
    private var tabSwitchGate = TabSwitchGate()
    var isClosing = false
    /// e2e 记录桌面通知文案（不依赖系统通知权限）。
    private(set) var recordedNotifications: [String] = []
    /// Palette discovery/attach 的最后一个可观察结果；测试和诊断不再
    /// 只能从「列表停在 New」反推异步失败。
    private(set) var lastPaletteError: String?
    private(set) var lastPaletteSelection: String?
    /// 注意力 Cmd-Enter 的 replica overlay（W19-E）。
    private var replyOverlayView: MuxTerminalView?
    var replyOverlayPaneId: UInt32?
    /// 搜索跳转：切 tab 完成后再滚到命中行。
    private var pendingSearchJump: PendingSearchJump?
    /// 后台 FFI 正在使用目标 bridge 时，激活先走缓存；主线程只在这里
    /// 保存一次重试，不得在 Workspace 点击路径上阻塞等待。
    private var pendingForegroundActivation: PendingForegroundActivation?
    private var foregroundActivationGeneration: UInt64 = 0
    /// 当前 utility 队列正在读取哪个激活代的权威快照。旧代结果只能丢弃，
    /// 不能覆盖用户已经再次选择的 Workspace。
    private var foregroundAuthorityRefreshGeneration: UInt64?
    /// Agent/Command 点击可能紧跟 Workspace 激活到达；等目标 bridge ready
    /// 后重放，避免点击动作因短暂锁竞争丢失。动作带目标 slot，重复选择
    /// 同一个 Workspace 时保留；切到另一个 Workspace 时只丢弃旧目标动作。
    private struct PendingForegroundAction {
        let slot: WarmConnectionSlot
        let action: () -> Void
    }
    private var pendingForegroundActions: [PendingForegroundAction] = []
    /// pane → 最近一次离开时的稳定行 ID。连接切换时清空，避免把不同
    /// workspace 的 seq 混用。
    private var lastSeenLineSeq: [UInt32: UInt64] = [:]
    /// 离开时 Index 还没有创建 PaneBuf 的 pane。下一轮 poll 在 Core 建好
    /// 稳定行索引后再建立基线，不能把暂时的 seq=0 当作真实行号。
    private var pendingLastSeenPanes = Set<UInt32>()
    private var lastSeenJump: (paneId: UInt32, offset: UInt32)?
    /// 当前 pane 上一次是否已经展示过 last-seen；避免 60Hz poll 重复
    /// 改变全局 overlay 的可见状态。
    private var lastSeenVisiblePane: UInt32?
    /// 当前 pane 在命令时间线中的游标；手动滚轮/搜索会清掉游标，
    /// Cmd+Option+↑/↓ 则按此游标前后移动。
    private var commandTimelineCursor: [UInt32: UInt64] = [:]
    /// 程序化命令跳转触发 native scroll callback 时保留游标一次。
    private var commandNavigationPanes = Set<UInt32>()
    /// 最近一次 poll 的 PaneOutput 条数（W13 洪水上限）。
    private(set) var lastPaneOutputEventCount: Int = 0
    private var languageObserver: NSObjectProtocol?
    /// 已向 tmux 上报过颜色的 pane（`refresh-client -r` 只需每个 pane 一次；
    /// 外观变化时清空重报）。
    private var reportedColourPanes = Set<UInt32>()
    /// 后台 tab 的 Surface 树按 runloop 一拍一棵预热，避免 attach 时一次建完卡死。
    private var tabWarmupScheduled = false
    /// 最近一次 status bar 快照（用于周期刷新与位置/样式渲染）。
    private var statusBarSnapshot: StatusBarSnapshot?
    /// statusbar 需要刷新（tab 增删/激活才置位；layout-change/pane 事件不触发，
    /// 避免多 tab 时每次结构事件都 spawn 1+N 个 tmux 子进程造成卡顿）。
    private var statusBarNeedsRefresh = false
    private var statusRefreshTimer: Timer?
    private var lastStatusFetchAt = Date.distantPast
    /// 结构事件后的 status bar 刷新（合并同一轮事件，避免 resize 风暴逐帧查询）。
    private var statusRefreshWorkItem: DispatchWorkItem?
    /// SSH 连接状态 + 流量监控刷新定时器（每秒更新一次显示）。
    private var trafficMonitorTimer: Timer?
    private var trafficRateSampler = TrafficRateSampler()
    private var activeProjectFlow: ProjectConnectFlowBox?
    /// 后台 warm slot 的 poll 不能和 UI timer 同步执行：远端控制模式在一轮
    /// refresh 中可能解析大量事件，阻塞这里会让 Connect/Cmd-Shift-P 出现
    /// beachball。每个 slot 自身仍用锁串行，主线程只负责投递一次任务。
    private var backgroundPollInFlight = false
    private let backgroundPollQueue = DispatchQueue(
        label: "muxterm.macos.background-poll",
        qos: .utility
    )
    /// 后台 poll 只把 Surface 事件交回主线程；主线程按小批次追赶，避免
    /// 一个高流量远端 pane 把切换、输入和窗口事件挤出 run loop。
    private var surfaceCatchUpSlots: [WarmConnectionSlot] = []
    private var surfaceCatchUpWorkItem: DispatchWorkItem?
    /// Warm connection pool：已使用过的 QuickConnect 目标切换时不立即关闭，
    /// 后台连接继续 poll；按 LRU/TTL/memory pressure 淘汰。
    private let connectionPool: ConnectionPool<WarmConnectionSlot>
    /// 已针对该 slot 数量显示过一次容量提醒；用户选择保留后不在每个 poll
    /// 重复打断，数量变化（新建或关闭）后才重新评估。
    private var capacityWarningPresentedForSlotCount: Int?
    /// 终端字体配置（config.toml `[font]`；Cmd +/- 缩放时保留 family）。
    private var terminalFontSettings: MuxtermTerminalFont.Settings
    /// Cmd +/- / Cmd 0 只写 Core `[font] size`；不再使用 UserDefaults 覆盖。
    private var configuredFontSize: CGFloat = MuxtermTerminalFont.defaultSize

    /// Core 解析后的配置快照（`configDescribeJSON` → `data.values`）。
    private struct ResolvedSettings {
        var fontFamily = MuxtermTerminalFont.defaultFamily
        var fontSize = MuxtermTerminalFont.defaultSize
        var themeName = "light"
        var statusBarMode = StatusBarMode.tmux
        var tabBarPosition = TabBarPosition.bottom
        var poolMaxSlots = MuxtermConfig.defaultPoolMaxSlots
        var projects: [TargetConfig] = []
    }

    private static func resolvedSettings(from bridge: CoreBridge) -> ResolvedSettings {
        var resolved = ResolvedSettings()
        guard let text = bridge.configDescribeJSON(),
              let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = envelope["data"] as? [String: Any],
              let values = payload["values"] as? [String: Any]
        else { return resolved }
        if let font = values["font"] as? [String: Any] {
            resolved.fontFamily = font["family"] as? String ?? resolved.fontFamily
            resolved.fontSize = CGFloat(font["size"] as? Double ?? Double(resolved.fontSize))
        }
        if let theme = values["theme"] as? [String: Any] {
            resolved.themeName = theme["name"] as? String ?? resolved.themeName
        }
        if let statusbar = values["statusbar"] as? [String: Any],
           let mode = statusbar["mode"] as? String,
           let parsed = StatusBarMode(rawValue: mode) {
            resolved.statusBarMode = parsed
        }
        if let ui = values["ui"] as? [String: Any],
           let position = ui["tab_bar_position"] as? String,
           let parsed = TabBarPosition(rawValue: position) {
            resolved.tabBarPosition = parsed
        }
        if let pool = values["pool"] as? [String: Any] {
            resolved.poolMaxSlots = pool["max_slots"] as? Int ?? resolved.poolMaxSlots
        }
        if let projects = values["projects"] as? [[String: Any]] {
            resolved.projects = QuickConnectStore.targetConfigs(from: projects)
        }
        return resolved
    }

    init(
        bridge: CoreBridge,
        debug: Bool = false,
        quickConnectStore injectedQuickConnectStore: QuickConnectStore? = nil
    ) {
        discovery.attachedLocalSocket = bridge.sshAlias == nil ? bridge.socket : nil
        discovery.attachedRemoteSocket = bridge.sshAlias == nil ? nil : bridge.socket
        // 统一配置：初始值来自 Core 解析后的快照，不再手写解析 TOML 或读 UserDefaults。
        let resolved = Self.resolvedSettings(from: bridge)
        MuxtermTerminalColors.activePalette = MuxtermTheme.from(
            name: resolved.themeName
        ).palette
        configuredFontSize = MuxtermTerminalFont.clamp(resolved.fontSize)
        terminalFontSettings = MuxtermTerminalFont.Settings(
            family: resolved.fontFamily,
            size: configuredFontSize
        )
        self.bridge = bridge
        self.terminalManager = TerminalManager(
            bridge: bridge,
            fontFamily: terminalFontSettings.family,
            fontSize: terminalFontSettings.size
        )
        self.content = ContentView(terminalManager: terminalManager)
        content.statusBar.setDebug(debug)
        content.statusBar.colorMode = resolved.statusBarMode
        content.applyTabBarPosition(resolved.tabBarPosition)
        connectionPool = ConnectionPool(
            policy: ConnectionPoolPolicy(maxSlots: resolved.poolMaxSlots)
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
        // 启动即按当前主题设置 chrome 外观（默认 light → aqua），
        // 否则 headless/深色系统下 effectiveAppearance 默认是 dark。
        let initialAppearance = NSAppearance(
            named: MuxtermTheme.from(name: resolved.themeName) == .dark ? .darkAqua : .aqua
        )
        window.appearance = initialAppearance
        content.appearance = initialAppearance

        // Project 列表来自 Core 快照；变更通过 CoreBridge 事务写回统一
        // config.toml（`[[projects]]`），不再读写 quickconnect.toml。
        if let injectedQuickConnectStore {
            quickConnectStore = injectedQuickConnectStore
        } else {
            let configBridge = bridge
            quickConnectStore = QuickConnectStore(projects: resolved.projects) { updated in
                do {
                    let transaction = try configBridge.configBegin()
                    try configBridge.configPatch(
                        transaction: transaction,
                        operations: [[
                            "op": "replace",
                            "path": "/projects",
                            "value": QuickConnectStore.projectJSON(from: updated),
                        ]]
                    )
                    try configBridge.configCommit(transaction: transaction)
                } catch {
                    // 失败时保留内存列表，不覆盖用户文件；下次启动仍读 Core 快照。
                    NSLog("muxterm: failed to persist projects: %@", error.localizedDescription)
                }
            }
        }
        super.init(window: window)
        window.delegate = self
        installMainSplit(in: window)
        installSidebarToggle(in: window)
        wireTerminalManagerCallbacks()

        workspaceSidebar.onWorkspaceActivate = { [weak self] workspaceId in
            _ = self?.activateWorkspaceIfAvailable(workspaceId)
        }
        workspaceSidebar.onWorkspaceClose = { [weak self] workspaceId in
            self?.closeWorkspace(workspaceId)
        }
        workspaceSidebar.onAgentActivate = { [weak self] workspaceId, tabId, paneId in
            guard let self, self.activateWorkspaceIfAvailable(workspaceId) else { return }
            self.performWhenForegroundReady { [weak self] in
                guard let self else { return }
                _ = self.bridge.attentionAcknowledge(paneId: paneId)
                self.jumpToPane(tabId: tabId, paneId: paneId)
            }
        }
        workspaceSidebar.onCommandActivate = { [weak self] workspaceId, tabId, paneId in
            guard let self, self.activateWorkspaceIfAvailable(workspaceId) else { return }
            self.performWhenForegroundReady { [weak self] in
                guard let self else { return }
                _ = self.bridge.attentionAcknowledge(paneId: paneId)
                self.jumpToPane(tabId: tabId, paneId: paneId)
            }
        }

        commandPalette = CommandPaletteController(ownerWindow: window)
        commandPalette.onSelect = { [weak self] item in
            self?.handlePaletteSelection(item)
        }
        unifiedPanel = UnifiedPanelController(
            store: quickConnectStore,
            ownerWindow: window,
            snapshot: { [weak self] in
                self?.attentionSnapshotForPanel()
            },
            paneOutput: { [weak self] paneId in
                guard let self, self.pendingForegroundActivation == nil else {
                    return Data()
                }
                return self.bridge.getPaneOutput(paneId: paneId)
            },
            sendInput: { [weak self] paneId, data in
                guard let self else { return }
                self.performWhenForegroundReady {
                    _ = self.bridge.sendInput(paneId: paneId, data: data)
                }
            },
            search: { [weak self] query, scope in
                self?.searchHitsForPanel(query: query, scope: scope) ?? []
            }
        )
        unifiedPanel.onConnect = { [weak self] config in
            self?.connect(config: config)
        }
        unifiedPanel.onLoadExistingConnections = { [weak self] completion in
            guard let self else {
                completion(.success([]))
                return
            }
            self.loadExistingConnections(completion: completion)
        }
        unifiedPanel.onLoadSSHAliases = { [weak self] completion in
            guard let self else {
                completion(.success([]))
                return
            }
            self.loadSSHAliases(completion: completion)
        }
        unifiedPanel.onAttachExistingConnection = { [weak self] choice in
            self?.attachExistingConnection(choice)
        }
        unifiedPanel.onExistingConnectionsError = { [weak self] error in
            self?.reportStatusError(error.localizedDescription)
        }
        unifiedPanel.onNewProject = { [weak self] in
            self?.editProject(nil)
        }
        unifiedPanel.onEditProject = { [weak self] config in
            self?.editProject(config)
        }
        unifiedPanel.onJump = { [weak self] workspaceId, tabId, paneId, seq, query in
            guard let self else { return }
            if let workspaceId {
                guard self.activateWorkspaceIfAvailable(workspaceId) else {
                    return
                }
            }
            self.performWhenForegroundReady { [weak self] in
                self?.jumpToPane(tabId: tabId, paneId: paneId, seq: seq, query: query)
            }
        }
        unifiedPanel.onPreview = { [weak self] workspaceId, paneId in
            guard let self, self.activateWorkspaceIfAvailable(workspaceId) else { return }
            self.performWhenForegroundReady { [weak self] in
                self?.toggleReplyOverlay(paneId: paneId)
            }
        }
        unifiedPanel.onAcknowledge = { [weak self] workspaceId, paneId in
            self?.acknowledgeWorkspacePane(workspaceId: workspaceId, paneId: paneId)
        }
        unifiedPanel.onMute = { [weak self] workspaceId, paneId, seconds in
            self?.muteWorkspacePane(
                workspaceId: workspaceId,
                paneId: paneId,
                seconds: seconds
            )
        }
        unifiedPanel.onDismissed = { [weak self] in
            self?.restoreTerminalFocusIfAllowed()
        }
        commandPalette.onDismissed = { [weak self] in
            self?.restoreTerminalFocusIfAllowed()
        }
        languageObserver = NotificationCenter.default.addObserver(
            forName: .muxtermLanguageChanged,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.refreshLocalizedUI()
        }

        content.statusBar.onSelectTab = { [weak self] tabId in
            self?.requestSwitchTab(tabId)
        }
        content.statusBar.onNewTab = { [weak self] in
            self?.newTab()
        }
        content.statusBar.onRenameTab = { [weak self] tabId in
            self?.promptRenameTab(tabId)
        }
        content.statusBar.onCloseTab = { [weak self] tabId in
            self?.closeTab(tabId)
        }
        content.statusBar.onMoveTab = { [weak self] from, target, before in
            _ = self?.moveTab(from: from, target: target, before: before)
        }
        content.statusBar.allowsTabReordering = terminalManager.usesClientResize
        content.paneLayout.onActivatePane = { [weak self] paneId in
            guard let self else { return }
            self.performWhenForegroundReady { [weak self] in
                guard let self else { return }
                self.focusPaneTerminal(paneId)
                if self.bridge.execute(task: MuxTask.switchPane(paneId)) != 0 {
                    self.reportStatusError(
                        MuxtermI18n.shared.tr(.errorSwitchPane, arguments: ["id": "\(paneId)"])
                    )
                }
            }
        }
        content.paneLayout.onSurfaceBecameReady = { [weak self] paneId, ready in
            guard let self else { return }
            if ready {
                self.scheduleTabTreeWarmup()
            }
            let active = self.lastSnapshot.panes.first(where: \.isActive)?.id
                ?? self.lastSnapshot.panes.first?.id
                ?? (self.pendingForegroundActivation == nil
                    ? self.bridge.snapshot().panes.first(where: \.isActive)?.id
                    : nil)
            guard TerminalInputFocusPolicy.shouldRetryWhenSurfaceReady(
                isActivePane: active == paneId,
                ready: ready
            ) else { return }
            self.focusPaneTerminal(paneId)
        }
        content.paneLayout.onMovePaneToNewTab = { [weak self] paneId in
            _ = self?.movePaneToNewTab(paneId)
        }
        content.paneLayout.allowsPaneBreak = terminalManager.usesClientResize
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
        // 铃铛始终是 Notifications 入口；Quick Connect 由 Cmd-P / 独立菜单负责。
        content.statusBar.onAttentionClick = { [weak self] in
            self?.openAttentionPanel()
        }
        content.jumpLatestButton.target = self
        content.jumpLatestButton.action = #selector(jumpToLatest)
        content.lastSeenButton.target = self
        content.lastSeenButton.action = #selector(jumpToLastSeen)
        content.commandMarkOKButton.target = self
        content.commandMarkOKButton.action = #selector(jumpToLastSuccessfulCommand)
        content.commandMarkFailButton.target = self
        content.commandMarkFailButton.action = #selector(jumpToLastFailedCommand)
        terminalManager.onOutputSnippetChanged = { [weak self] snippet in
            self?.content.statusBar.updateOutputSnippet(snippet)
        }
        terminalManager.onError = { [weak self] message in
            self?.reportStatusError(message)
        }

        // 启动时由 AppDelegate 创建的首个连接也属于当前 Workspace。
        // 过去只有 Quick Connect 后续创建的连接才登记进池，导致初始 local
        // workspace 既不在 Recent，也无法在切走后保持 warm。
        let initialTarget = bridge.resolvedTargetConfig
        let initialKey = ConnectionKey(
            transport: bridge.sshAlias == nil ? "local" : "ssh",
            alias: bridge.sshAlias,
            session: initialTarget?.session ?? bridge.session ?? "",
            runtime: initialTarget?.runtime.rawValue
                ?? (terminalManager.usesClientResize ? "tmux" : "shell"),
            path: initialTarget?.path ?? bridge.startDirectory ?? "",
            socket: initialTarget?.socket ?? bridge.socket,
            workspaceID: initialTarget?.workspaceID
        )
        let initialSlot = WarmConnectionSlot(
            key: initialKey,
            bridge: bridge,
            terminalManager: terminalManager,
            targetConfig: initialTarget,
            now: 0
        )
        initialSlot.openedOrder = nextWorkspaceOpenedOrder
        nextWorkspaceOpenedOrder += 1
        connectionPool.acquire(key: initialKey) { _ in initialSlot }

        installKeyEquivalents()
        applyTheme(currentTheme())
        refreshWorkspaceSidebar(force: true)
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
        trafficMonitorTimer?.invalidate()
        surfaceCatchUpWorkItem?.cancel()
        surfaceCatchUpWorkItem = nil
        surfaceCatchUpSlots.removeAll()
        if let languageObserver {
            NotificationCenter.default.removeObserver(languageObserver)
        }
        if !isClosing {
            connectionPool.shutdownAll()
            bridge.shutdown()
        }
        statusRefreshTimer?.invalidate()
        removeKeyMonitor()
    }

    // MARK: - 公开动作（菜单 / 快捷键）

    @objc func newTab() {
        guard bridge.execute(task: MuxTask.newTab()) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorNewTab))
            return
        }
        // 当拍 snapshot 还是旧 tab。等 TabAdded 再挂新树，避免先拆再等 tmux。
    }

    @objc func renameActiveTab() {
        guard let tab = lastSnapshot.tabs.first(where: \.isActive)
            ?? lastSnapshot.tabs.first else { return }
        promptRenameTab(tab.id)
    }

    @objc func renameCurrentWorkspace() {
        let current = connectionPool.currentTargetConfig?.name
            ?? bridge.session
            ?? "workspace"
        promptForName(title: MuxtermI18n.shared.tr(.renameWorkspace), current: current) {
            [weak self] name in
            _ = self?.renameWorkspace(to: name)
        }
    }

    @discardableResult
    func renameTab(_ tabId: UInt32, to name: String) -> Bool {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return false }
        guard bridge.execute(task: MuxTask.renameTab(tabId, name: name)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
            return false
        }
        return true
    }

    @discardableResult
    func renameWorkspace(to name: String) -> Bool {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return false }
        guard bridge.execute(task: MuxTask.renameWorkspace(name)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
            return false
        }
        applyWorkspaceRename(name)
        return true
    }

    private func promptRenameTab(_ tabId: UInt32) {
        guard let tab = lastSnapshot.tabs.first(where: { $0.id == tabId }) else { return }
        promptForName(title: MuxtermI18n.shared.tr(.renameTab), current: tab.name) {
            [weak self] name in
            _ = self?.renameTab(tabId, to: name)
        }
    }

    private func promptForName(
        title: String,
        current: String,
        completion: @escaping (String) -> Void
    ) {
        guard let ownerWindow = window else { return }
        let alert = NSAlert()
        alert.messageText = title
        let field = NSTextField(string: current)
        field.placeholderString = title
        field.selectText(nil)
        field.frame = NSRect(x: 0, y: 0, width: 320, height: 24)
        alert.accessoryView = field
        alert.addButton(withTitle: MuxtermI18n.shared.tr(.rename))
        alert.addButton(withTitle: MuxtermI18n.shared.tr(.cancel))
        alert.beginSheetModal(for: ownerWindow) { response in
            guard response == .alertFirstButtonReturn else { return }
            completion(field.stringValue)
        }
    }

    private func applyWorkspaceRename(_ name: String) {
        if terminalManager.usesClientResize {
            bridge.session = name
        }
        connectionPool.renameActiveTarget(
            to: name,
            rekeySession: terminalManager.usesClientResize
        )
        window?.title = "\(name) — Muxterm"
    }

    @discardableResult
    func moveTab(from: UInt32, target: UInt32, before: Bool) -> Bool {
        guard terminalManager.usesClientResize, from != target else { return false }
        guard bridge.execute(
            task: MuxTask.moveTab(from: from, target: target, before: before)
        ) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
            return false
        }
        needsLayoutReload = true
        scheduleStatusBarRefresh()
        return true
    }

    @objc func moveActiveTabLeft() {
        moveActiveTab(offset: -1)
    }

    @objc func moveActiveTabRight() {
        moveActiveTab(offset: 1)
    }

    private func moveActiveTab(offset: Int) {
        guard let index = lastSnapshot.tabs.firstIndex(where: \.isActive) else { return }
        let destination = index + offset
        guard lastSnapshot.tabs.indices.contains(destination) else { return }
        _ = moveTab(
            from: lastSnapshot.tabs[index].id,
            target: lastSnapshot.tabs[destination].id,
            before: offset < 0
        )
    }

    @objc func moveActivePaneToNewTab() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id else { return }
        _ = movePaneToNewTab(pane)
    }

    @discardableResult
    func movePaneToNewTab(_ paneId: UInt32) -> Bool {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                _ = self?.movePaneToNewTab(paneId)
            }
            return true
        }
        guard terminalManager.usesClientResize,
              lastSnapshot.panes.count > 1,
              lastSnapshot.panes.contains(where: { $0.id == paneId })
        else { return false }
        guard bridge.execute(task: MuxTask.breakPane(paneId)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
            return false
        }
        needsLayoutReload = true
        scheduleStatusBarRefresh()
        return true
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

    /// 返回快捷键/命令面板使用的 tab 顺序。
    ///
    /// Core snapshot 是跨平台 UI 的唯一拓扑事实源；tmux status 只负责
    /// left/right 文案和样式，不能再提供第二份窗口列表。TmuxRuntime 已
    /// 按权威 window index 排好 `tabs`，这里保持该顺序并使用稳定 tab id。
    private func tabEntriesForSwitching() -> [(index: Int, id: UInt32, name: String)] {
        lastSnapshot.tabs.enumerated().map { position, tab in
            (index: position + 1, id: tab.id, name: tab.name)
        }
    }

    private func tabID(forShortcutIndex oneBased: Int) -> UInt32? {
        guard oneBased >= 1 else { return nil }
        let entries = tabEntriesForSwitching()
        // 快捷键编号是 Core tabs 的 1-based 位置；tmux window_index 只在
        // Runtime 内部用于排序，不泄漏成第二套 UI 编号。
        return entries.first(where: { $0.index == oneBased })?.id
    }

    /// 1-based 序号切换 tab。
    func switchToTabIndex(_ oneBased: Int) {
        guard let tabId = tabID(forShortcutIndex: oneBased) else { return }
        requestSwitchTab(tabId)
    }

    @objc func switchToLastTab() {
        guard let tabId = tabEntriesForSwitching().last?.id else { return }
        requestSwitchTab(tabId)
    }

    /// Cmd+Ctrl+N：按固定打开顺序切换 Workspace，不随最近使用重排。
    /// 与 Linux Ctrl+Alt+N 使用同一组 `switch_workspace_N` 语义。
    func switchToWorkspaceAtFixedIndex(_ oneBased: Int) {
        guard (1...5).contains(oneBased) else { return }
        let ordered = workspaceSidebarFixedSlots()
        guard ordered.indices.contains(oneBased - 1) else { return }
        activate(slot: ordered[oneBased - 1])
    }

    @objc func splitHorizontal() {
        splitActivePane(horizontal: true)
    }

    @objc func splitVertical() {
        splitActivePane(horizontal: false)
    }

    /// 当前 pane 全屏切换：tmux/ssh 发 `resize-pane -Z`，本地 shell 用布局全屏。
    @objc func toggleActivePaneFullscreen() {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.toggleActivePaneFullscreen()
            }
            return
        }
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
        let base = MuxtermTerminalFont.clamp(configuredFontSize)
        persistConfig([["op": "replace", "path": "/font/size", "value": Double(base)]])
        terminalFontSettings.size = base
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: base,
            container: content.paneLayout
        )
    }

    private func adjustTerminalFontSize(delta: Int) {
        let current = terminalFontSettings.size
        let next = MuxtermTerminalFont.zoomed(current, direction: delta)
        guard next != current else { return }
        persistConfig([["op": "replace", "path": "/font/size", "value": Double(next)]])
        terminalFontSettings.size = next
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: next,
            container: content.paneLayout
        )
    }

    /// 当前主题：从 Core 解析后的快照读取，缺省浅色。
    func currentTheme() -> MuxtermTheme {
        MuxtermTheme.from(name: Self.resolvedSettings(from: bridge).themeName)
    }

    /// 应用主题并持久化：更新终端默认色、重报 tmux 颜色，命令面板标题会
    /// 在下次打开时显示当前主题。
    private func applyTheme(_ theme: MuxtermTheme) {
        let name = theme == .dark ? "black" : "white"
        persistConfig([["op": "replace", "path": "/theme/name", "value": name]])
        MuxtermTerminalColors.activePalette = theme.palette
        // Chrome 外观必须跟着主题走（light=aqua, dark=darkAqua），
        // 不能只写 UserDefaults（W19-A：主题切换失败）。
        let appearance = NSAppearance(
            named: theme == .dark ? .darkAqua : .aqua
        )
        window?.appearance = appearance
        content.appearance = appearance
        NSApp.appearance = appearance
        // 强制外观立即传播（headless 下 effectiveAppearance 可能延迟）。
        window?.contentView?.viewDidChangeEffectiveAppearance()
        window?.displayIfNeeded()
        // 主题色变化后终端 SwiftTerm 默认色、光标、ANSI 16 色与 OSC 10/11
        // 都跟随 theme.palette（light=黑字白底，dark=浅字深底）。
        terminalManager.applyPalette(theme.palette)
        // 主题色变化后必须给**所有** pane 重新上报，tmux 才会用新颜色代答
        // OSC 10/11；只报当前 tab 会让后台 tab 的 agent 沿用旧色。
        // 必须报主题真值（浅色=黑字白底）。不能报灰色：会污染整个
        // tmux session，普通 `tmux attach` 里字也会变白。
        reportedColourPanes.removeAll()
        let osc = ColorContrast.oscColors(fg: theme.palette.fg, bg: theme.palette.bg)
        _ = bridge.reportAllPaneColours(
            fgHex: osc.fg,
            bgHex: osc.bg
        )
        // 重新渲染 status bar（GUI 黑白模式跟随主题；tmux 模式样式不变）。
        if statusBarSnapshot != nil {
            content.applyStatusBar(statusBarSnapshot)
        }
    }

    func toggleTheme() {
        let next: MuxtermTheme = currentTheme() == .light ? .dark : .light
        applyTheme(next)
        commandPalette.update(
            items: rootPaletteItems(),
            placeholder: MuxtermI18n.shared.tr(.commandPalette)
        )
    }

    private func toggleStatusBarMode() {
        let next: StatusBarMode = content.statusBar.colorMode == .tmux ? .theme : .tmux
        persistConfig([["op": "replace", "path": "/statusbar/mode", "value": next.rawValue]])
        content.statusBar.colorMode = next
        if statusBarSnapshot != nil {
            content.applyStatusBar(statusBarSnapshot)
        }
        commandPalette.update(
            items: rootPaletteItems(),
            placeholder: MuxtermI18n.shared.tr(.commandPalette)
        )
    }

    @objc func setTabBarTop(_ sender: Any?) {
        persistConfig([["op": "replace", "path": "/ui/tab_bar_position", "value": "top"]])
        content.applyTabBarPosition(.top)
    }

    @objc func setTabBarBottom(_ sender: Any?) {
        persistConfig([["op": "replace", "path": "/ui/tab_bar_position", "value": "bottom"]])
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

    /// Open the Core Schema/Manifest-backed settings window.
    @objc func openPreferences() {
        if let settingsWindow {
            settingsWindow.showWindow(self)
            return
        }
        let controller = SettingsWindowController(bridge: bridge)
        settingsWindow = controller
        controller.showWindow(self)
    }

    @objc func openQuickConnect() {
        guard let unifiedPanel else { return }
        // Recent 由连接池派生（最近打开且仍 warm 的连接）；当前连接用于行高亮。
        unifiedPanel.currentConfig = connectionPool.currentTargetConfig
        quickConnectStore.replaceAllRecents(connectionPool.allRecentTargetConfigs())
        unifiedPanel.show(tab: .workspaces)
    }

    @objc func openSearchPanel() {
        openSearchPanel(scope: .workspace)
    }

    @objc func openWorkspaceSearchPanel() {
        openSearchPanel(scope: .workspace)
    }

    @objc func openGlobalSearchPanel() {
        openSearchPanel(scope: .all)
    }

    func openSearchPanel(scope: SearchScope) {
        guard let unifiedPanel else { return }
        unifiedPanel.show(tab: .search, scope: scope)
    }

    @objc func openAttentionPanel() {
        guard let unifiedPanel else { return }
        unifiedPanel.show(tab: .attention)
    }

    private func installSidebarToggle(in window: NSWindow) {
        sidebarToggleButton.image = NSImage(
            systemSymbolName: "sidebar.left",
            accessibilityDescription: "Toggle Sidebar"
        )
        sidebarToggleButton.title = ""
        sidebarToggleButton.imagePosition = .imageOnly
        sidebarToggleButton.bezelStyle = .texturedRounded
        sidebarToggleButton.setButtonType(.toggle)
        sidebarToggleButton.state = .off
        sidebarToggleButton.target = self
        sidebarToggleButton.action = #selector(toggleWorkspaceSidebar)
        sidebarToggleButton.setAccessibilityIdentifier("muxterm.sidebar.toggle")
        sidebarToggleButton.translatesAutoresizingMaskIntoConstraints = false

        let holder = NSView(frame: NSRect(x: 0, y: 0, width: 38, height: 28))
        holder.addSubview(sidebarToggleButton)
        NSLayoutConstraint.activate([
            sidebarToggleButton.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 4),
            sidebarToggleButton.centerYAnchor.constraint(equalTo: holder.centerYAnchor),
            sidebarToggleButton.widthAnchor.constraint(equalToConstant: 30),
            sidebarToggleButton.heightAnchor.constraint(equalToConstant: 24),
        ])
        let accessory = NSTitlebarAccessoryViewController()
        accessory.layoutAttribute = .left
        accessory.view = holder
        window.addTitlebarAccessoryViewController(accessory)
        sidebarTitlebarAccessory = accessory
    }

    private func installMainSplit(in window: NSWindow) {
        let sidebarController = NSViewController()
        sidebarController.view = workspaceSidebar
        let sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebarController)
        sidebarItem.canCollapse = true
        sidebarItem.minimumThickness = 180
        sidebarItem.maximumThickness = 420
        sidebarItem.preferredThicknessFraction = 0.25

        let contentController = NSViewController()
        contentController.view = content
        let contentItem = NSSplitViewItem(viewController: contentController)
        contentItem.minimumThickness = 320

        mainSplitController.splitView.isVertical = true
        mainSplitController.splitView.dividerStyle = .thin
        mainSplitController.splitView.autosaveName = "muxterm.main.sidebar"
        mainSplitController.splitView.setAccessibilityIdentifier("muxterm.main.split")
        mainSplitController.addSplitViewItem(sidebarItem)
        mainSplitController.addSplitViewItem(contentItem)
        sidebarItem.isCollapsed = true
        sidebarSplitItem = sidebarItem
        window.contentViewController = mainSplitController
    }

    @objc private func toggleWorkspaceSidebar() {
        setWorkspaceSidebarOpen(!isWorkspaceSidebarOpen)
    }

    private var isWorkspaceSidebarOpen: Bool {
        sidebarSplitItem?.isCollapsed == false
    }

    private func setWorkspaceSidebarOpen(_ open: Bool) {
        sidebarSplitItem?.isCollapsed = !open
        sidebarToggleButton.state = open ? .on : .off
        if open {
            refreshWorkspaceSidebar(force: true)
        }
        window?.contentView?.needsLayout = true
    }

    /// In-process E2E uses the same production toggle path.
    func setWorkspaceSidebarOpenForTest(_ open: Bool) {
        setWorkspaceSidebarOpen(open)
    }

    func workspaceSidebarOpenForTest() -> Bool {
        isWorkspaceSidebarOpen
    }

    /// 点击系统通知时始终回到主窗口并显示 Attention，不复用 toggle 语义。
    func revealAttentionFromSystemNotification() {
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
        unifiedPanel.show(tab: .attention)
    }

    /// 统一面板的实时查询范围覆盖当前 warm 连接；当前 Workspace 固定排在
    /// 首位，其余按稳定 Workspace ID 排序。搜索本身需要进入后台 bridge，
    /// 但侧栏/Attention 展示优先读 WarmConnectionSlot 的后台缓存。
    private func forEachPanelBridge(_ body: (CoreBridge, WarmConnectionSlot?) -> Void) {
        // 延迟 Workspace 激活期间，`bridge` 已经指向目标但仍可能被旧的
        // 后台 poll 持有；不要让搜索回调在主线程触碰它。
        if pendingForegroundActivation == nil {
            let activeSlot = connectionPool.activeKey.flatMap { connectionPool.slots[$0] }
            body(bridge, activeSlot)
        }
        let background = connectionPool.slots.values
            .filter { $0.bridge !== bridge && $0.lifecycle != .evicting }
            .sorted {
                QuickConnect.uniqueID(for: $0.targetConfig)
                    < QuickConnect.uniqueID(for: $1.targetConfig)
        }
        for slot in background {
            _ = slot.tryWithBridge { candidate in
                body(candidate, slot)
            }
        }
    }

    private func attentionSnapshot(from candidate: CoreBridge) -> AttentionSnapshot? {
        guard let json = candidate.attentionSnapshotJSON() else { return nil }
        return AttentionSnapshot.decode(Data(json.utf8))
    }

    private func fallbackReplicaID(for target: TargetConfig) -> String {
        let identityPath = target.workspaceID.flatMap { $0.isEmpty ? nil : $0 } ?? target.path
        let session = target.session?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let name = session.isEmpty ? QuickConnect.defaultName(for: identityPath) : session
        let transport = target.transport.label
        if !session.isEmpty, !identityPath.isEmpty {
            return "\(name):\(identityPath)@\(transport)"
        }
        return "\(name)@\(transport)"
    }

    private func workspaceReplicaID(from candidate: CoreBridge, target: TargetConfig) -> String {
        attentionSnapshot(from: candidate)?.workspaces.first?.workspaceId
            ?? fallbackReplicaID(for: target)
    }

    private func workspaceReplicaID(for slot: WarmConnectionSlot) -> String {
        slot.cachedWorkspaceReplicaID ?? fallbackReplicaID(for: slot.targetConfig)
    }

    /// 后台连接的 Attention 展示只读缓存；这样侧栏刷新不会因为远端
    /// bridge 正在 poll 而等待。首轮缓存由 utility poll 尽快填充。
    private func attentionSnapshot(for slot: WarmConnectionSlot) -> AttentionSnapshot? {
        return slot.cachedAttentionSnapshot
    }

    private var activeWorkspaceReplicaID: String? {
        guard let target = connectionPool.currentTargetConfig else { return nil }
        if let activeKey = connectionPool.activeKey,
           let slot = connectionPool.slots[activeKey]
        {
            return workspaceReplicaID(for: slot)
        }
        return workspaceReplicaID(from: bridge, target: target)
    }

    /// Workspace 点击可能已经把窗口切到目标缓存，但目标 bridge 还在等
    /// 上一批后台 FFI 释放。动作必须等到权威刷新完成后再重放，不能在
    /// 主线程直接碰那把锁。
    func performWhenForegroundReady(_ action: @escaping () -> Void) {
        guard !isClosing else { return }
        if let activation = pendingForegroundActivation {
            pendingForegroundActions.append(PendingForegroundAction(
                slot: activation.slot,
                action: action
            ))
        } else {
            action()
        }
    }

    var foregroundActivationIsPending: Bool {
        pendingForegroundActivation != nil
    }

    func workspaceSidebarFixedSlots() -> [WarmConnectionSlot] {
        connectionPool.slots.values
            .filter { $0.lifecycle != .evicting }
            .sorted { lhs, rhs in
                if lhs.openedOrder != rhs.openedOrder {
                    return lhs.openedOrder < rhs.openedOrder
                }
                return lhs.key.session < rhs.key.session
        }
    }

    /// Build the sidebar's pane navigation cache in one topology walk. Core
    /// keeps both the stable TabId and the user-facing 1-based order
    /// authoritative.
    private static func tabTargetsByPane(
        from candidate: CoreBridge
    ) -> (tabIdsByPane: [UInt32: UInt32], tabNumbersByPane: [UInt32: Int]) {
        var tabIdsByPane: [UInt32: UInt32] = [:]
        var tabNumbersByPane: [UInt32: Int] = [:]
        for (index, tab) in candidate.getTabs().enumerated() {
            for pane in candidate.getPanes(tabId: tab.id) {
                tabIdsByPane[pane.id] = tab.id
                tabNumbersByPane[pane.id] = index + 1
            }
        }
        return (tabIdsByPane, tabNumbersByPane)
    }

    private func tabTargetsByPane(
        for slot: WarmConnectionSlot
    ) -> (tabIdsByPane: [UInt32: UInt32], tabNumbersByPane: [UInt32: Int]) {
        if let tabIds = slot.cachedTabIdsByPane,
           let tabNumbers = slot.cachedTabNumbersByPane
        {
            return (tabIds, tabNumbers)
        }
        // Sidebar refresh is a render-only path. If the background topology
        // poll has not populated this slot yet, leave the maps empty and let
        // the next poll publish them; never synchronously walk a remote bridge.
        return (tabIdsByPane: [:], tabNumbersByPane: [:])
    }

    private func sidebarItems() -> [WorkspaceSidebarItem] {
        let slots = workspaceSidebarFixedSlots()
        return slots.enumerated().compactMap { index, slot in
            let shortcut = index < 5 ? index + 1 : nil
            let isActive = slot.lifecycle == .active
            let target = slot.targetConfig
            let structuredAgents = slot.cachedStructuredAgents
            let tabTargets = tabTargetsByPane(for: slot)
            return WorkspaceSidebarItem(
                workspaceId: workspaceReplicaID(for: slot),
                name: target.name,
                runtime: target.runtime.rawValue,
                transport: target.transport.label,
                isActive: isActive,
                shortcut: shortcut,
                structuredAgents: structuredAgents,
                tabNumberByPane: tabTargets.tabNumbersByPane,
                tabIdByPane: tabTargets.tabIdsByPane
            )
        }
    }

    func refreshWorkspaceSidebar(force: Bool = false) {
        guard force || isWorkspaceSidebarOpen else { return }
        let workspaces = sidebarItems()
        workspaceSidebar.setWorkspaces(workspaces)
        let attention = attentionSnapshotForPanel()
        workspaceSidebar.setAgents(WorkspaceSidebarProjection.agents(
            workspaces: workspaces,
            attention: attention
        ))
        workspaceSidebar.setCommands(WorkspaceSidebarProjection.commands(
            workspaces: workspaces,
            attention: attention
        ))
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: lastSnapshot.activeTab,
            paneId: activePaneID
        )
    }

    private func attentionSnapshotForPanel(refreshActive: Bool = false) -> AttentionSnapshot? {
        var workspaces: [WorkspaceAttention] = []
        var seen = Set<String>()
        func append(_ snapshot: AttentionSnapshot) {
            for workspace in snapshot.workspaces where seen.insert(workspace.workspaceId).inserted {
                workspaces.append(workspace)
            }
        }
        let activeSnapshot: AttentionSnapshot?
        if pendingForegroundActivation != nil {
            activeSnapshot = connectionPool.activeKey
                .flatMap { connectionPool.slots[$0]?.cachedAttentionSnapshot }
        } else if refreshActive {
            // 面板打开/刷新是低频的用户动作。此时读取 active bridge 的
            // 权威快照，避免上一拍 sidebar cache 让刚完成的 command
            // 仍显示 process_name=nil；高频侧栏调用默认仍走 cache。
            activeSnapshot = attentionSnapshot(from: bridge)
        } else {
            activeSnapshot = connectionPool.activeKey
                .flatMap { connectionPool.slots[$0]?.cachedAttentionSnapshot }
        }
        if let snapshot = activeSnapshot {
            append(snapshot)
        }
        for slot in connectionPool.slots.values
            where slot.bridge !== bridge && slot.lifecycle != .evicting
        {
            if let snapshot = attentionSnapshot(for: slot) {
                append(snapshot)
            }
        }
        guard !workspaces.isEmpty else { return nil }
        let blockedCount = workspaces.reduce(into: 0) { count, workspace in
            if workspace.blocked > 0 { count += 1 }
        }
        return AttentionSnapshot(blockedCount: blockedCount, workspaces: workspaces)
    }

    private func searchHitsForPanel(query: String, scope: SearchScope) -> [SearchHit] {
        // 在延迟激活窗口内，active bridge 仍可能被上一批后台 FFI 使用。
        // 面板先显示空结果，ready 后 completeForegroundActivation 会刷新。
        guard pendingForegroundActivation == nil else { return [] }
        var allHits: [SearchHit] = []
        var seen = Set<String>()
        let consume: (CoreBridge, WarmConnectionSlot?) -> Void = { candidate, slot in
            guard let json = candidate.searchAllJSON(query: query),
                  let snapshot = SearchSnapshot.decode(Data(json.utf8))
            else {
                return
            }
            // 搜索结果本身也携带稳定 Workspace ID。后台 attention 尚未
            // 首轮到达时先把它写入 slot，后续点击搜索命中无需再做同步
            // FFI 身份探测。
            if let workspaceID = snapshot.hits.first?.workspaceId {
                slot?.cacheWorkspaceReplicaID(workspaceID)
            }
            for hit in snapshot.hits {
                let key = "\(hit.workspaceId)\u{1F}\(hit.tabId)\u{1F}\(hit.paneId)\u{1F}\(hit.seq)"
                if seen.insert(key).inserted {
                    allHits.append(hit)
                }
            }
        }
        if scope == .all {
            forEachPanelBridge(consume)
        } else {
            let activeSlot = connectionPool.activeKey.flatMap { connectionPool.slots[$0] }
            consume(bridge, activeSlot)
        }
        let workspacePaneIDs = Set(
            bridge.getTabs().flatMap { bridge.getPanes(tabId: $0.id).map(\.id) }
        )
        return scope.filter(
            allHits,
            activePane: activePaneID,
            workspaceId: activeWorkspaceReplicaID,
            workspacePaneIDs: workspacePaneIDs
        )
    }

    @discardableResult
    private func withWorkspaceBridge(
        _ workspaceId: String,
        _ body: (CoreBridge) -> Void
    ) -> Bool {
        if activeWorkspaceReplicaID == workspaceId {
            body(bridge)
            return true
        }
        for slot in connectionPool.slots.values where slot.lifecycle != .evicting {
            var matches = workspaceReplicaID(for: slot) == workspaceId
                || QuickConnect.uniqueID(for: slot.targetConfig) == workspaceId
            // 首轮后台 metadata 尚未抵达时，不能让 Attention/Search 的
            // activate 因缓存为空而失败；只在 fast path 未命中时实时确认。
            if !matches, slot.cachedAttentionSnapshot == nil {
                matches = slot.withBridge { candidate in
                    workspaceReplicaID(from: candidate, target: slot.targetConfig) == workspaceId
                        || QuickConnect.uniqueID(for: slot.targetConfig) == workspaceId
                } ?? false
            }
            if matches {
                return slot.withBridge { candidate in
                    body(candidate)
                    return true
                } ?? false
            }
        }
        return false
    }

    /// 测试用：按 Workspace 安全读取 blocked 计数（后台 slot 会锁住 bridge）。
    func testAttentionBlockedCount(workspaceId: String) -> Int {
        var blockedCount = -1
        _ = withWorkspaceBridge(workspaceId) { bridge in
            guard let json = bridge.attentionSnapshotJSON(),
                  let data = json.data(using: .utf8),
                  let snapshot = AttentionSnapshot.decode(data)
            else {
                return
            }
            blockedCount = snapshot.blockedCount
        }
        return blockedCount
    }

    /// E2E 用：同步排空 warm 后台 Workspace，避免 QoS 队列造成偶发超时。
    /// 仍走 WarmConnectionSlot 的生产 drain/apply 路径。
    func testPollBackgroundWorkspaces() {
        // testPollOnce() 可能刚把同一批 slot 投递到后台队列；先等该批
        // FFI 操作结束，再从测试线程查询结果。否则测试直接读取 bridge
        // 时会和 drainBackgroundEvents() 并发访问同一个 C handle。
        backgroundPollQueue.sync {}
        for slot in connectionPool.slots.values where slot.lifecycle == .background {
            while !isClosing {
                let drained = slot.drainBackgroundEvents()
                var pending = slot.hasPendingSurfaceWork
                while pending {
                    pending = slot.applyPendingSurfaceEvents(
                        maxEvents: Int.max,
                        timeBudget: .infinity
                    )
                }
                if !drained && !pending {
                    break
                }
            }
        }
    }

    /// 把另一个隔离 CoreBridge 登记为 warm Workspace 并激活。
    func testActivateWorkspaceBridge(_ nextBridge: CoreBridge, session: String) {
        let currentSession = bridge.session ?? session
        let currentKey = ConnectionKey(
            transport: bridge.sshAlias == nil ? "local" : "ssh",
            alias: bridge.sshAlias,
            session: currentSession,
            runtime: "tmux",
            path: "",
            socket: bridge.socket
        )
        if connectionPool.slots[currentKey] == nil {
            let currentSlot = WarmConnectionSlot(
                key: currentKey,
                bridge: bridge,
                terminalManager: terminalManager,
                now: 0
            )
            currentSlot.openedOrder = nextWorkspaceOpenedOrder
            nextWorkspaceOpenedOrder += 1
            _ = connectionPool.acquire(key: currentKey) { _ in currentSlot }
        }
        let key = ConnectionKey(
            transport: nextBridge.sshAlias == nil ? "local" : "ssh",
            alias: nextBridge.sshAlias,
            session: session,
            runtime: "tmux",
            path: "",
            socket: nextBridge.socket
        )
        nextBridge.session = session
        activate(slot: WarmConnectionSlot(key: key, bridge: nextBridge, now: 0))
    }

    /// Close a warm workspace from the sidebar. The active slot falls forward to
    /// the next warm workspace; closing the last one closes the session window.
    func closeWorkspace(_ workspaceId: String) {
        guard let slot = connectionPool.slots.values.first(where: { candidate in
            candidate.lifecycle != .evicting && workspaceReplicaID(for: candidate) == workspaceId
        }) else { return }

        let wasActive = slot.lifecycle == .active
        let ordered = workspaceSidebarFixedSlots()
        let index = ordered.firstIndex(where: { $0 === slot }) ?? 0
        let fallback = wasActive
            ? ordered.dropFirst(index + 1).first
                ?? ordered.prefix(index).last
            : nil

        connectionPool.close(key: slot.key)
        content.paneLayout.dropParked(except: Array(connectionPool.slots.values.map(\.terminalManager)))
        quickConnectStore.replaceAllRecents(connectionPool.allRecentTargetConfigs())
        if wasActive {
            if let fallback {
                activate(slot: fallback)
            } else {
                closeSessionWindow()
            }
        }
        refreshWorkspaceSidebar(force: true)
    }

    /// 容量是 soft limit：超过阈值时只提醒用户，绝不静默移除后台 Workspace。
    /// 列出的都是最久未使用的后台 slot，当前活动 Workspace 永远不会出现在
    /// 选择框中；用户选择后才调用同一条 sidebar close/evict 资源路径。
    private func presentWorkspaceCapacityWarningIfNeeded() {
        guard !isClosing else { return }
        if !connectionPool.isOverCapacity {
            capacityWarningPresentedForSlotCount = nil
            return
        }
        guard capacityWarningPresentedForSlotCount != connectionPool.slotCount,
              let ownerWindow = window
        else { return }

        let slotCount = connectionPool.slotCount
        let overflow = max(1, slotCount - connectionPool.maxSlots)
        let candidates = connectionPool.oldestBackgroundCandidates(
            limit: min(8, overflow)
        )
        guard !candidates.isEmpty else { return }
        capacityWarningPresentedForSlotCount = slotCount

        let alert = NSAlert()
        alert.messageText = MuxtermI18n.shared.tr(.workspaceCapacityTitle)
        alert.informativeText = MuxtermI18n.shared.tr(
            .workspaceCapacityMessage,
            arguments: [
                "count": "\(slotCount)",
                "limit": "\(connectionPool.maxSlots)",
            ]
        )
        alert.alertStyle = .warning

        let choices = candidates.map { candidate -> (ConnectionCapacityCandidate, NSButton) in
            let config = candidate.targetConfig
            let button = NSButton(
                checkboxWithTitle: "\(config.name) · \(config.runtime.rawValue) @ \(config.transport.label)",
                target: nil,
                action: nil
            )
            button.state = .off
            return (candidate, button)
        }
        let accessory = NSStackView(views: choices.map(\.1))
        accessory.orientation = .vertical
        accessory.alignment = .leading
        accessory.spacing = 6
        accessory.setFrameSize(NSSize(
            width: 380,
            height: max(24, accessory.fittingSize.height)
        ))
        alert.accessoryView = accessory
        alert.addButton(withTitle: MuxtermI18n.shared.tr(.workspaceCapacityCloseSelected))
        alert.addButton(withTitle: MuxtermI18n.shared.tr(.workspaceCapacityKeepAll))

        alert.beginSheetModal(for: ownerWindow) { [weak self] response in
            guard let self else { return }
            if response == .alertFirstButtonReturn {
                for (candidate, checkbox) in choices where checkbox.state == .on {
                    _ = self.connectionPool.close(key: candidate.key)
                }
                self.content.paneLayout.dropParked(
                    except: Array(self.connectionPool.slots.values.map(\.terminalManager))
                )
                self.quickConnectStore.replaceAllRecents(
                    self.connectionPool.allRecentTargetConfigs()
                )
                self.refreshWorkspaceSidebar(force: true)
                self.unifiedPanel.refreshData()
            }
            // 关闭了部分候选后仍然超限时，不在同一轮连续弹窗；下一次新增
            // slot（或手动关闭使数量变化）再重新提醒。
            self.capacityWarningPresentedForSlotCount = self.connectionPool.isOverCapacity
                ? self.connectionPool.slotCount
                : nil
        }
    }

    @discardableResult
    private func activateWorkspaceIfAvailable(_ workspaceId: String) -> Bool {
        if activeWorkspaceReplicaID == workspaceId {
            return true
        }
        for slot in connectionPool.slots.values where slot.lifecycle != .evicting {
            var matches = workspaceReplicaID(for: slot) == workspaceId
                || QuickConnect.uniqueID(for: slot.targetConfig) == workspaceId
            // 首轮后台 metadata 尚未抵达时，不能让侧栏/Attention 的激活
            // 因缓存为空而失败；只在 fast path 未命中时实时确认。
            if !matches, slot.cachedAttentionSnapshot == nil {
                matches = slot.tryWithBridge { candidate in
                    workspaceReplicaID(from: candidate, target: slot.targetConfig) == workspaceId
                        || QuickConnect.uniqueID(for: slot.targetConfig) == workspaceId
                } ?? false
            }
            if matches {
                activate(slot: slot)
                return true
            }
        }
        return false
    }

    /// 面板中的确认/静音是 UI 动作，后台 Workspace 的 bridge 竞争时只
    /// 尝试一次；下一轮后台快照会把列表和 badge 校准回来。这样点击面板
    /// 也不会复制 Legion 的卡顿路径。
    @discardableResult
    private func tryWithWorkspaceBridge(
        _ workspaceId: String,
        _ body: (CoreBridge) -> Void
    ) -> Bool {
        guard pendingForegroundActivation == nil else { return false }
        if activeWorkspaceReplicaID == workspaceId {
            body(bridge)
            return true
        }
        for slot in connectionPool.slots.values where slot.lifecycle != .evicting {
            var matches = workspaceReplicaID(for: slot) == workspaceId
                || QuickConnect.uniqueID(for: slot.targetConfig) == workspaceId
            if !matches, slot.cachedAttentionSnapshot == nil {
                matches = slot.tryWithBridge { candidate in
                    workspaceReplicaID(from: candidate, target: slot.targetConfig) == workspaceId
                        || QuickConnect.uniqueID(for: slot.targetConfig) == workspaceId
                } ?? false
            }
            guard matches else { continue }
            return slot.tryWithBridge { candidate in
                body(candidate)
                return true
            } ?? false
        }
        return false
    }

    private func acknowledgeWorkspacePane(workspaceId: String, paneId: UInt32) {
        performWhenForegroundReady { [weak self] in
            guard let self else { return }
            let acknowledged = self.tryWithWorkspaceBridge(workspaceId) { targetBridge in
                _ = targetBridge.attentionAcknowledge(paneId: paneId)
            }
            guard acknowledged else {
                return
            }
            // Open/Jump 之后列表和 badge 都应立即反映“已读”，不等待下一轮
            // 60Hz poll；后台 Workspace 也必须走它自己的 bridge。
            self.unifiedPanel.refreshData()
        }
    }

    private func muteWorkspacePane(
        workspaceId: String,
        paneId: UInt32,
        seconds: UInt64
    ) {
        performWhenForegroundReady { [weak self] in
            guard let self else { return }
            _ = self.tryWithWorkspaceBridge(workspaceId) { targetBridge in
                _ = targetBridge.attentionMute(paneId: paneId, seconds: seconds)
            }
        }
    }

    /// 跳转到指定 tab + pane（搜索命中 / 注意力行）。
    ///
    /// `tabId` 为 nil 时按 pane 反查（注意力行没有 tab）。tmux window 0
    /// 是真实 tab，不能当哨兵跳过。`seq>0` 时把历史滚到命中行。
    func jumpToPane(tabId: UInt32?, paneId: UInt32, seq: UInt64 = 0, query: String = "") {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.jumpToPane(tabId: tabId, paneId: paneId, seq: seq, query: query)
            }
            return
        }
        let resolvedTab = tabId ?? bridge.tabId(containingPane: paneId)
        if let resolvedTab {
            requestSwitchTab(resolvedTab)
        }
        if bridge.execute(task: MuxTask.switchPane(paneId)) != 0 {
            reportStatusError(
                MuxtermI18n.shared.tr(.errorSwitchPane, arguments: ["id": "\(paneId)"])
            )
            return
        }
        // 侧栏点击需要立即反映乐观切换；下一轮权威 snapshot 会再次校准。
        // 这样点击跨 tab 的 Agent/Command 后不会先跳回旧 row 的高亮。
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: resolvedTab,
            paneId: paneId
        )
        needsLayoutReload = true
        if seq > 0 || !query.isEmpty {
            pendingSearchJump = PendingSearchJump(paneId: paneId, seq: seq, query: query)
            applyPendingSearchJumpIfReady()
        }
    }

    /// 注意力面板 Cmd-Enter：打开/关闭独立 replica overlay（W19-E）。
    /// overlay 用选中 pane 的 snapshot 渲染，I/O 走 overlay，不改主布局。
    func toggleReplyOverlay(paneId: UInt32? = nil) {
        if let overlay = replyOverlayView, !content.replyOverlayContainer.isHidden {
            overlay.removeFromSuperview()
            replyOverlayView = nil
            replyOverlayPaneId = nil
            content.replyOverlayContainer.isHidden = true
            content.replyOverlayContainer.setAccessibilityValue("0")
            return
        }
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.toggleReplyOverlay(paneId: paneId)
            }
            return
        }
        guard unifiedPanel?.modelTab == .attention else { return }
        let targetPaneId = paneId ?? unifiedPanel?.testSelectedAttentionRow()?.pane.paneId
        guard let targetPaneId else { return }
        let overlay = MuxTerminalView(paneId: targetPaneId, frame: .zero)
        overlay.setAccessibilityIdentifier(CmdEnterRouting.overlayIdentifier)
        overlay.setAccessibilityElement(true)
        overlay.inputHandler = self
        overlay.applyPalette(MuxtermTerminalColors.activePalette)
        overlay.translatesAutoresizingMaskIntoConstraints = false
        content.replyOverlayContainer.addSubview(overlay)
        NSLayoutConstraint.activate([
            overlay.leadingAnchor.constraint(equalTo: content.replyOverlayContainer.leadingAnchor),
            overlay.trailingAnchor.constraint(equalTo: content.replyOverlayContainer.trailingAnchor),
            overlay.topAnchor.constraint(equalTo: content.replyOverlayContainer.topAnchor),
            overlay.bottomAnchor.constraint(equalTo: content.replyOverlayContainer.bottomAnchor),
        ])
        replyOverlayView = overlay
        replyOverlayPaneId = targetPaneId
        content.replyOverlayContainer.isHidden = false
        // 手动布局（不依赖容器 Auto Layout，headless 下容器高度可能为 0）。
        window?.layoutIfNeeded()
        content.layoutSubtreeIfNeeded()
        let overlayFrame = content.bounds.insetBy(dx: 24, dy: 24)
        content.replyOverlayContainer.frame = overlayFrame
        overlay.frame = content.replyOverlayContainer.bounds
        overlay.layoutSubtreeIfNeeded()
        _ = overlay.syncSizeToPty(notifyResize: false)
        let visible = bridge.paneVisibleANSI(paneId: targetPaneId)
        let raw = bridge.getPaneOutput(paneId: targetPaneId)
        let data = PanePaintPolicy.firstPaint(visible: visible, raw: raw, rows: 24)
        if !data.isEmpty {
            overlay.feedOutput(data, isSnapshot: true)
        }
        content.replyOverlayContainer.setAccessibilityValue("1")
    }

    /// 回底：把当前 pane 的 viewport 重置到最新（W16a jump-latest）。
    @objc func jumpToLatest() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        else {
            return
        }
        terminalManager.scrollToLatest(paneId: pane)
        content.setJumpLatestVisible(false, unseenLines: 0)
        needsLayoutReload = true
    }

    @objc func jumpToLastSeen() {
        guard let target = lastSeenJump else { return }
        applyPaneViewport(paneId: target.paneId, offset: target.offset)
        // 点击后消费这次离开提示；下一次完整的离开→返回才建立新 marker。
        lastSeenLineSeq.removeValue(forKey: target.paneId)
        lastSeenJump = nil
        setLastSeenVisible(false, paneId: target.paneId)
    }

    @objc private func jumpToLastSuccessfulCommand() {
        guard let pane = activePaneID,
              let mark = commandMarks(for: pane).reversed().first(where: { $0.exitCode == 0 })
        else { return }
        jumpToCommandMark(mark, paneId: pane)
    }

    @objc private func jumpToLastFailedCommand() {
        guard let pane = activePaneID,
              let mark = commandMarks(for: pane).reversed().first(where: {
                  guard let code = $0.exitCode else { return false }
                  return code != 0
              })
        else { return }
        jumpToCommandMark(mark, paneId: pane)
    }

    /// 按 OSC 133 时间线跳到当前命令之前最近的一条命令。
    @objc func jumpToPreviousCommand() {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.jumpToPreviousCommand()
            }
            return
        }
        guard let pane = activePaneID else { return }
        let marks = commandMarks(for: pane)
        guard !marks.isEmpty else { return }
        let target: CoreCommandMark?
        if let current = commandTimelineCursor[pane] {
            target = marks.last(where: { $0.seq < current })
        } else {
            target = marks.last
        }
        if let target {
            jumpToCommandMark(target, paneId: pane)
        }
    }

    /// 按 OSC 133 时间线跳到当前命令之后最近的一条命令；已经在末尾时
    /// 清掉游标并回到实时底部，和向下滚动到底部的语义一致。
    @objc func jumpToNextCommand() {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.jumpToNextCommand()
            }
            return
        }
        guard let pane = activePaneID else { return }
        let marks = commandMarks(for: pane)
        if let current = commandTimelineCursor[pane],
           let target = marks.first(where: { $0.seq > current })
        {
            jumpToCommandMark(target, paneId: pane)
        } else {
            commandTimelineCursor.removeValue(forKey: pane)
            terminalManager.scrollToLatest(paneId: pane)
        }
    }

    private func commandMarks(for paneId: UInt32) -> [CoreCommandMark] {
        bridge.paneCommandMarks(paneId: paneId)
            .filter { $0.exitCode != nil && $0.historyOffset != nil }
    }

    private func jumpToCommandMark(_ mark: CoreCommandMark, paneId: UInt32) {
        guard let offset = mark.historyOffset else { return }
        commandTimelineCursor[paneId] = mark.seq
        commandNavigationPanes.insert(paneId)
        applyPaneViewport(paneId: paneId, offset: offset)
    }

    private var activePaneID: UInt32? {
        if let active = lastSnapshot.panes.first(where: \.isActive)?.id {
            return active
        }
        if lastSnapshot.panes.contains(where: { $0.id == lastSnapshot.activePane }) {
            // pane id 0 is valid; membership, rather than `!= 0`, is the
            // sentinel check for optimistic/authoritative snapshots.
            return lastSnapshot.activePane
        }
        return lastSnapshot.panes.first?.id
    }

    /// 把 core 的 viewport 滚动偏移应用到 SwiftTerm 可见区：
    /// offset>0 时喂滚动窗口 ANSI（历史），offset==0 时恢复 live 输出。
    func applyPaneViewport(paneId: UInt32, offset: UInt32) {
        terminalManager.applyViewport(paneId: paneId, offset: offset)
        content.setJumpLatestVisible(
            offset > 0,
            unseenLines: terminalManager.unseenLineCount(paneId: paneId)
        )
    }

    private func wireTerminalManagerCallbacks() {
        terminalManager.onViewportChanged = { [weak self] paneId, offset in
            guard let self else { return }
            // 用户滚轮/触控板改变视口时，下一次命令导航应从当前状态重新开始；
            // 程序化 command jump 只保留刚设置的游标一次。
            if self.commandNavigationPanes.remove(paneId) == nil {
                self.commandTimelineCursor.removeValue(forKey: paneId)
            }
            self.content.setJumpLatestVisible(
                offset > 0,
                unseenLines: self.terminalManager.unseenLineCount(paneId: paneId)
            )
            guard self.pendingForegroundActivation == nil else { return }
            guard paneId == self.activePaneID else { return }
            self.refreshHistoryChrome(for: paneId)
        }
        terminalManager.onUnseenLinesChanged = { [weak self] paneId, count in
            guard let self,
                  self.pendingForegroundActivation == nil,
                  paneId == self.activePaneID
            else { return }
            let offset = max(0, self.bridge.paneViewport(paneId: paneId))
            self.content.setJumpLatestVisible(offset > 0, unseenLines: count)
        }
    }

    /// Unified Quick Panel 的 Existing Connections 由 Catalog 扁平发现
    /// tmux + Herdr；当前显式 tmux socket 作为附加 identity 合并进去。
    private func loadExistingConnections(
        completion: @escaping (Result<[ExistingConnectionChoice], Error>) -> Void
    ) {
        let target: ConnectionTarget
        if let alias = bridge.sshAlias {
            target = .ssh(SSHHostInfo(alias: alias, hostname: "", user: nil, port: nil))
        } else {
            target = .local
        }
        discovery.listExistingConnections(
            currentTarget: target,
            currentSocket: terminalManager.usesClientResize ? bridge.socket : nil,
            completion: completion
        )
    }

    /// `@alias` 补全读取全部 SSH config alias，不要求该 host 已经有可 attach
    /// 的 workspace；发现过程在后台线程执行。
    private func loadSSHAliases(completion: @escaping (Result<[String], Error>) -> Void) {
        discovery.listSSHAliases(completion: completion)
    }

    /// Existing 行是严格 attach-only：不创建目录、不进入 ProjectConnectFlow。
    private func attachExistingConnection(_ choice: ExistingConnectionChoice) {
        unifiedPanel.dismiss()
        switch choice.config.runtime {
        case .tmux:
            attach(
                target: choice.target,
                session: choice.session.name,
                resolvedSocket: choice.socket
            )
        case .herdr:
            content.setConnectProgress(stage: .attach)
            connectCatalogTarget(config: choice.config, intent: .attachOnly) { [weak self] result in
                self?.finishCatalogConnect(result)
            }
        case .shell:
            // Shell 没有 Discover；防御未知/未来候选时仍走严格 attach-only。
            content.setConnectProgress(stage: .attach)
            connectCatalogTarget(config: choice.config, intent: .attachOnly) { [weak self] result in
                self?.finishCatalogConnect(result)
            }
        }
    }

    /// 按 QuickConnect 目标连接：tmux 有 name → attach，无 name → 创建；
    /// shell runtime → 本地/远程 shell 在 path 启动。
    func connect(config: TargetConfig) {
        unifiedPanel.dismiss()
        content.setConnectProgress(stage: .resolving)
        // recents 由连接池派生：连接成功后 pool.acquire 会更新最近列表。
        switch config.runtime {
        case .tmux:
            connectProject(config: config)
        case .herdr:
            connectHerdrProject(config: config)
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
                    self.content.setConnectProgress(stage: nil)
                    // attachTmux 内部已通过 connectionPool 激活 slot 并切换渲染。
                case .failure(let error):
                    box.flow.attachExistingFailed(message: error.localizedDescription)
                    self.content.setConnectProgress(stage: nil)
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
                    let msg = error.localizedDescription
                    // session 已存在（attach 失败后 create 撞名）：直接重试 attach，
                    // 不报错——session 可能在上一步 attach 和 create 之间被其他进程创建。
                    if msg.contains("duplicate session") {
                        box.flow.createSucceeded()
                        self.runProjectFlow(box, config: config)
                    } else {
                        box.flow.createFailed(message: msg)
                        self.activeProjectFlow = nil
                        self.showError(error, prefix: "create session failed")
                    }
                }
            }
        case .attachCreated:
            attachTmux(config: config, session: box.flow.session) { [weak self] result in
                guard let self, self.activeProjectFlow === box else { return }
                switch result {
                case .success:
                    box.flow.attachCreatedSucceeded()
                    self.activeProjectFlow = nil
                    self.content.setConnectProgress(stage: nil)
                    // attachTmux 内部已通过 connectionPool 激活 slot 并切换渲染。
                case .failure(let error):
                    box.flow.attachCreatedFailed(message: error.localizedDescription)
                    self.activeProjectFlow = nil
                    self.content.setConnectProgress(stage: nil)
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

    /// 在 tmux -CC spawn 前给出一个接近当前窗口的字符网格。
    ///
    /// 必须用真实等宽字体 metrics。以前用 8×17 会把 Menlo 18 的窗口估成
    /// 128×63，attach 先按这个播种，随后才 `refresh-client -C 93x51`。
    private func initialTmuxClientSizeHint() -> (UInt16, UInt16)? {
        let bounds = content.paneLayout.bounds
        let scale = window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 1
        return MuxTerminalGridMetrics.clientSize(
            bounds: bounds.size,
            family: terminalFontSettings.family,
            size: terminalFontSettings.size,
            backingScale: scale
        )
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

        let params = config.transport.attachBackend
        let initialClientSize = initialTmuxClientSizeHint()
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                // SSH：alias 走 sshAlias，socket 不得填 Host 名（否则 `tmux -L ryzen`）。
                let nextBridge = try CoreBridge.connect(
                    backendType: params.type,
                    socket: params.socket,
                    session: session,
                    sshAlias: params.sshAlias,
                    initialClientSize: initialClientSize
                )
                DispatchQueue.main.async {
                    guard let self else {
                        nextBridge.shutdown()
                        return
                    }
                    let slot = WarmConnectionSlot(key: key, bridge: nextBridge, now: 0)
                    if let initialClientSize {
                        slot.terminalManager.noteClientSize(initialClientSize)
                    }
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

    /// Herdr Project 先按普通重连（AttachOnly）解析；只有保存的 Project
    /// 明确无匹配时才尝试 CreateIfMissing，且 Core 仍要求用户已选择 named
    /// session/socket，绝不由 macOS 偷选 default server。
    private func connectHerdrProject(config: TargetConfig) {
        let isSavedProject = quickConnectStore.projects.contains {
            QuickConnect.uniqueID(for: $0) == QuickConnect.uniqueID(for: config)
        }
        content.setConnectProgress(stage: .attach)
        connectCatalogTarget(config: config, intent: .attachOnly) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let connection):
                if isSavedProject {
                    self.quickConnectStore.upsertProject(connection.target)
                }
                self.finishCatalogConnect(.success(connection))
            case .failure where isSavedProject:
                self.connectCatalogTarget(config: config, intent: .createIfMissing) { [weak self] createResult in
                    guard let self else { return }
                    if case .success(let connection) = createResult {
                        self.quickConnectStore.upsertProject(connection.target)
                    }
                    self.finishCatalogConnect(createResult)
                }
            case .failure(let error):
                self.finishCatalogConnect(.failure(error))
            }
        }
    }

    /// descriptor-aware Runtime 建连；Catalog resolver 返回的 canonical target
    /// 用于 warm key 与 Recent，不能从 WorkspaceId 五段字符串反推。
    private func connectCatalogTarget(
        config: TargetConfig,
        intent: CoreTargetOpenIntent,
        completion: @escaping (Result<CatalogConnection, Error>) -> Void
    ) {
        let requestedKey = Self.connectionKey(config: config, session: config.session)
        if let slot = connectionPool.slots[requestedKey], slot.lifecycle != .evicting {
            let canonical = QuickConnect.mergingProjectMetadata(
                resolved: slot.targetConfig,
                requested: config
            )
            slot.targetConfig = canonical
            activate(slot: slot)
            completion(.success(CatalogConnection(bridge: slot.bridge, target: canonical)))
            return
        }

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                let nextBridge = try CoreBridge.connect(target: config, intent: intent)
                let resolved = nextBridge.resolvedTargetConfig ?? config
                let key = Self.connectionKey(config: resolved, session: resolved.session)
                DispatchQueue.main.async {
                    guard let self else {
                        nextBridge.shutdown()
                        return
                    }
                    if let existing = self.connectionPool.slots[key],
                       existing.lifecycle != .evicting
                    {
                        let canonical = QuickConnect.mergingProjectMetadata(
                            resolved: existing.targetConfig,
                            requested: resolved
                        )
                        existing.targetConfig = canonical
                        self.activate(slot: existing)
                        DispatchQueue.global(qos: .utility).async {
                            nextBridge.shutdown()
                        }
                        completion(.success(CatalogConnection(
                            bridge: existing.bridge,
                            target: canonical
                        )))
                        return
                    }
                    let slot = WarmConnectionSlot(
                        key: key,
                        bridge: nextBridge,
                        targetConfig: resolved,
                        now: 0
                    )
                    self.activate(slot: slot)
                    completion(.success(CatalogConnection(bridge: nextBridge, target: resolved)))
                }
            } catch {
                DispatchQueue.main.async {
                    completion(.failure(error))
                }
            }
        }
    }

    private func finishCatalogConnect(_ result: Result<CatalogConnection, Error>) {
        content.setConnectProgress(stage: nil)
        if case .failure(let error) = result {
            showError(error)
        }
    }

    /// 从 TargetConfig + session 构造 pool key（连接身份）。
    private static func connectionKey(
        config: TargetConfig,
        session: String?
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
            session: session ?? (config.runtime == .tmux ? config.name : ""),
            runtime: config.runtime.rawValue,
            path: config.path,
            socket: config.socket,
            workspaceID: config.workspaceID
        )
    }

    /// 激活一个 warm slot：替换 bridge / TerminalManager / PaneLayout 的渲染源。
    /// 旧 slot 由 ConnectionPool.acquire 自动降为 background，不 shutdown；
    /// 新 slot 进入后再由 UI 检查 soft capacity。
    /// 激活已有 warm slot。保持 module-internal，供 in-process E2E 验证跨
    /// Workspace 搜索/Attention 跳转；产品入口仍由 Quick Connect 驱动。
    func activate(slot: WarmConnectionSlot) {
        guard !isClosing else { return }

        // 每次新的 Workspace 选择都使之前的延迟激活失效。旧目标已经
        // 被用户明确切走，不能在稍后拿到锁时把画面抢回来。
        foregroundActivationGeneration &+= 1
        let generation = foregroundActivationGeneration
        foregroundAuthorityRefreshGeneration = nil
        pendingForegroundActivation = nil
        // 只清掉属于已经被用户切走的 Workspace 的动作。属于本次目标
        // 的动作（例如重复点击同一行后排队的 pane 跳转）必须保留到
        // 权威刷新完成，否则点击看起来会“闪一下但没有反应”。
        pendingForegroundActions.removeAll { $0.slot !== slot }

        let oldBridge = bridge
        let startedAt = ProcessInfo.processInfo.systemUptime
        let bridgeReady = slot.prepareForForeground()
        // 只读取队列状态，不在点击回调里同步重放 Surface。远端锁/输出
        // 洪水必须留给后续主线程小批次，避免 SSH Workspace 点击卡住。
        let hasPendingSurfaceCatchUp = slot.hasPendingSurfaceWork
        if slot.openedOrder == 0 {
            slot.openedOrder = nextWorkspaceOpenedOrder
            nextWorkspaceOpenedOrder += 1
        }
        let (_, created) = connectionPool.acquire(key: slot.key) { _ in slot }
        quickConnectStore.replaceAllRecents(connectionPool.allRecentTargetConfigs())
        bridge = slot.bridge
        terminalManager = slot.terminalManager
        // 缓存树挂载和首轮 authority 查询期间，TerminalManager 只能做
        // 本地 SwiftTerm/AppKit 工作；所有 bridge 访问在 utility 队列串行化。
        terminalManager.setBridgeQueriesEnabled(false)
        trafficRateSampler.reset()
        lastSeenLineSeq.removeAll()
        pendingLastSeenPanes.removeAll()
        lastSeenJump = nil
        lastSeenVisiblePane = nil
        content.setLastSeenVisible(false)
        commandTimelineCursor.removeAll()
        commandNavigationPanes.removeAll()
        wireTerminalManagerCallbacks()
        let restoredParkedTree = content.paneLayout.replaceTerminalManager(slot.terminalManager)
        content.paneLayout.dropParked(
            except: Array(connectionPool.slots.values.map(\.terminalManager))
        )
        content.statusBar.allowsTabReordering = terminalManager.usesClientResize
        content.paneLayout.allowsPaneBreak = terminalManager.usesClientResize
        // warm slot 的 TerminalManager 各自保存字体状态：切回时沿用当前字号，
        // 避免旧 slot 还是切换前的小字体。字号没变就不要 resetFont，那会清选区。
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: terminalFontSettings.size,
            container: content.paneLayout
        )
        // warm slot 的视图沿用当前主题 palette（终端跟随主题）。
        terminalManager.applyPalette(MuxtermTerminalColors.activePalette)
        lastSnapshot = slot.lastSnapshot
        // 切连接后旧 status bar 属于上一个 tmux：先清掉，等新快照到达再显示。
        statusBarSnapshot = nil
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        content.applyStatusBar(nil)
        tabSwitchGate = TabSwitchGate()
        needsLayoutReload = WorkspaceSwitchPaintPolicy.needsLayoutReload(
            restoredParkedTree: restoredParkedTree
        )
        let activation = PendingForegroundActivation(
            slot: slot,
            oldBridge: oldBridge,
            restoredParkedTree: restoredParkedTree,
            hasPendingSurfaceCatchUp: hasPendingSurfaceCatchUp,
            created: created,
            generation: generation,
            startedAt: startedAt,
            wasDeferred: !bridgeReady
        )

        NSLog(
            "muxterm: workspace activation begin target=%@ ready=%@ restored=%@",
            slot.targetConfig.name,
            bridgeReady ? "true" : "false",
            restoredParkedTree ? "true" : "false"
        )
        // 无论 bridgeLock 当前是否空闲，都先交付缓存画面，再把权威拓扑
        // 读取放到后台。这样“可见切换”和“远端校准”不再绑在同一帧。
        pendingForegroundActivation = activation
        paintCachedForegroundActivation(slot, restoredParkedTree: restoredParkedTree)
        scheduleForegroundAuthorityRefresh()
    }

    /// 用 warm slot 的缓存完成最小视觉切换。snapshot 可能随后被权威
    /// bridge 修正，但用户不应在 SSH 查询期间看到旧 Workspace 卡住。
    private func paintCachedForegroundActivation(
        _ slot: WarmConnectionSlot,
        restoredParkedTree: Bool
    ) {
        let snapshot = slot.lastSnapshot
        guard !snapshot.tabs.isEmpty || !snapshot.panes.isEmpty else {
            refreshWorkspaceSidebar()
            return
        }

        lastSnapshot = snapshot
        tabSwitchGate.onSnapshot(tabs: snapshot.tabs.map(\.id))
        content.updateTabs(snapshot.tabs)
        terminalManager.updatePaneSizes(snapshot.panes)
        if !restoredParkedTree, needsLayoutReload,
           content.paneLayout.apply(
               layout: snapshot.layout,
               panes: snapshot.panes,
               tabId: snapshot.activeTab
           )
        {
            needsLayoutReload = false
        }
        content.statusBar.updateDebugSnapshot(snapshot)
        content.statusBar.updateOutputSnippet(terminalManager.recentOutputSnippet)
        if let activePane = snapshot.panes.first(where: \.isActive)?.id
            ?? snapshot.panes.first?.id
        {
            content.paneLayout.markActivePane(activePane)
            terminalManager.focusTarget = terminalManager.view(for: activePane)
            restoreTerminalFocusIfAllowed()
        }
        refreshWorkspaceSidebar()
    }

    private static func captureForegroundAuthority(
        from bridge: CoreBridge
    ) -> ForegroundAuthoritySnapshot {
        let frame = bridge.snapshot()
        var allPanes: [Pane] = []
        var tabIdsByPane: [UInt32: UInt32] = [:]
        var tabNumbersByPane: [UInt32: Int] = [:]
        for (index, tab) in frame.tabs.enumerated() {
            let panes = bridge.getPanes(tabId: tab.id)
            allPanes.append(contentsOf: panes)
            for pane in panes {
                tabIdsByPane[pane.id] = tab.id
                tabNumbersByPane[pane.id] = index + 1
            }
        }
        return ForegroundAuthoritySnapshot(
            frame: frame,
            allPanes: allPanes,
            tabIdsByPane: tabIdsByPane,
            tabNumbersByPane: tabNumbersByPane
        )
    }

    /// 在后台锁内读取目标 Workspace 的权威拓扑。即使远端 capture/pause
    /// 需要数百毫秒，也只占 utility 队列，主线程已经在显示 warm cache。
    private func scheduleForegroundAuthorityRefresh() {
        guard let activation = pendingForegroundActivation,
              !isClosing
        else {
            return
        }
        let generation = activation.generation
        guard foregroundAuthorityRefreshGeneration != generation else { return }
        foregroundAuthorityRefreshGeneration = generation
        backgroundPollQueue.async { [weak self] in
            let result = activation.slot.withBridge { candidate in
                Self.captureForegroundAuthority(from: candidate)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self,
                      let pending = self.pendingForegroundActivation,
                      pending.generation == generation,
                      generation == self.foregroundActivationGeneration,
                      self.foregroundAuthorityRefreshGeneration == generation,
                      !self.isClosing
                else {
                    return
                }
                self.foregroundAuthorityRefreshGeneration = nil
                self.pendingForegroundActivation = nil
                if let result {
                    self.applyForegroundAuthoritySnapshot(result)
                }
                self.completeForegroundActivation(activation)
                let actions = self.pendingForegroundActions.filter {
                    $0.slot === activation.slot
                }
                self.pendingForegroundActions.removeAll {
                    $0.slot === activation.slot
                }
                for action in actions {
                    action.action()
                }
            }
        }
    }

    /// 只在主线程应用后台已经取回的值类型快照；这里不调用 CoreBridge。
    private func applyForegroundAuthoritySnapshot(
        _ authority: ForegroundAuthoritySnapshot
    ) {
        let snap = authority.frame
        tabSwitchGate.onSnapshot(tabs: snap.tabs.map(\.id))
        guard tabSwitchGate.isReleased() else { return }
        lastSnapshot = snap
        terminalManager.updatePaneSizes(authority.allPanes.isEmpty ? snap.panes : authority.allPanes)
        content.updateTabs(snap.tabs)
        if needsLayoutReload {
            if content.paneLayout.apply(
                layout: snap.layout,
                panes: snap.panes,
                tabId: snap.activeTab
            ) {
                needsLayoutReload = false
                content.statusBar.clearLayoutSyncError()
            } else {
                content.statusBar.showLayoutSyncing()
            }
        }
        content.paneLayout.pruneTabs(keeping: Set(snap.tabs.map(\.id)))
        content.statusBar.updateDebugSnapshot(snap)
        content.statusBar.updateOutputSnippet(terminalManager.recentOutputSnippet)
        if let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id {
            terminalManager.focusTarget = terminalManager.view(for: activePane)
            content.paneLayout.markActivePane(activePane)
            restoreTerminalFocusIfAllowed()
        }
        cacheActiveSlotSnapshot(
            tabIdsByPane: authority.tabIdsByPane,
            tabNumbersByPane: authority.tabNumbersByPane
        )
    }

    private func completeForegroundActivation(
        _ activation: PendingForegroundActivation
    ) {
        guard !isClosing,
              activation.generation == foregroundActivationGeneration,
              activation.slot.lifecycle == .active,
              bridge === activation.slot.bridge
        else {
            return
        }

        let elapsedMilliseconds =
            (ProcessInfo.processInfo.systemUptime - activation.startedAt) * 1000
        NSLog(
            "muxterm: workspace activation ready target=%@ deferred=%@ elapsed_ms=%.1f",
            activation.slot.targetConfig.name,
            activation.wasDeferred ? "true" : "false",
            elapsedMilliseconds
        )

        // 缓存/权威快照已经完成绘制；从这一刻起恢复正常 bridge 查询，
        // 但几何同步仍走下一拍，避免把恢复动作重新塞回点击栈。
        terminalManager.setBridgeQueriesEnabled(true)
        content.paneLayout.resumeGeometrySync()
        focusActiveTerminal()
        refreshStatusBar(force: true)
        // 切连接后立即更新 SSH 状态 + 流量监控显示。
        updateTrafficMonitor()
        // 这一拍只读后台缓存；下一个正常 poll 再做 active bridge 的
        // attention acknowledge/snapshot，避免再次把远端查询放回点击栈。
        refreshAttentionChrome(allowBridgeQueries: false)
        // 后台 metadata 可能在 lifecycle 切换为 active 的竞态窗口内完成。
        // 这些通知不能再由 background poll 留到下一次切换才消费。
        postAttentionNotifications(
            activation.slot.takePendingAttentionNotifications()
        )
        refreshWorkspaceSidebar()
        unifiedPanel.refreshData()
        if activation.hasPendingSurfaceCatchUp || activation.slot.hasPendingSurfaceWork {
            enqueueSurfaceCatchUp(activation.slot)
        }
        // 若旧 bridge 不在 pool（初始连接或非 pool 路径），切走后直接回收；
        // pool 内的旧 slot 由 acquire 降为 background，保持 warm。
        let oldIsPooled = connectionPool.slots.values.contains {
            $0.bridge === activation.oldBridge
        }
        if !oldIsPooled, activation.oldBridge !== activation.slot.bridge {
            DispatchQueue.global(qos: .utility).async {
                activation.oldBridge.shutdown()
            }
        }
        if activation.created {
            presentWorkspaceCapacityWarningIfNeeded()
        }

        // 颜色查询应答很重要，但不影响 Workspace 首帧；放到后台锁内
        // 上报，完成后下一轮输出即可使用新的 OSC 颜色。
        if WorkspaceSwitchPaintPolicy.shouldReportColours(
            restoredParkedTree: activation.restoredParkedTree
        ) {
            let osc = ColorContrast.oscColors(
                fg: MuxtermTerminalColors.activePalette.fg,
                bg: MuxtermTerminalColors.activePalette.bg
            )
            backgroundPollQueue.async { [weak self] in
                _ = activation.slot.withBridge { candidate in
                    candidate.reportAllPaneColours(fgHex: osc.fg, bgHex: osc.bg)
                }
                DispatchQueue.main.async { [weak self] in
                    guard let self,
                          self.foregroundActivationGeneration == activation.generation,
                          self.bridge === activation.slot.bridge
                    else { return }
                    self.reportedColourPanes.removeAll()
                }
            }
        }
    }

    /// 打开/编辑 project 配置窗口。
    private func editProject(_ config: TargetConfig?) {
        // 配置窗口以 sheet 形式出现，先收起 Cmd-P 面板，避免遮盖。
        unifiedPanel.dismiss()
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
            sshHosts: hosts,
            availableRuntimes: (try? CoreBridge.runtimeCatalog())?
                .compactMap { TargetRuntime(rawValue: $0.id) }
                ?? TargetRuntime.allCases
        )
        win.onSave = { [weak self] saved in
            self?.quickConnectStore.upsertProject(saved)
            // 保存后重新打开面板，方便继续连接/编辑。
            self?.unifiedPanel.present(initial: .workspaces)
        }
        win.onCancel = { [weak self] in
            // 取消/关闭后恢复面板。
            self?.unifiedPanel.present(initial: .workspaces)
        }
    }

    private func overlayOwnsFocus() -> Bool {
        let key = NSApp.keyWindow
        return key === unifiedPanel?.window || key === commandPalette?.window
    }

    func restoreTerminalFocusIfAllowed() {
        focusActiveTerminal()
    }

    private func focusActiveTerminal() {
        let snap: FrameSnapshot
        if !lastSnapshot.panes.isEmpty {
            snap = lastSnapshot
        } else if pendingForegroundActivation == nil {
            snap = bridge.snapshot()
        } else {
            return
        }
        guard let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id else {
            return
        }
        focusPaneTerminal(activePane)
    }

    /// 光标必须落在 SwiftTerm 输入。host 边框高亮不等于键盘在 pane 里。
    private func focusPaneTerminal(_ paneId: UInt32) {
        let view = terminalManager.view(for: paneId)
        terminalManager.focusTarget = view
        content.paneLayout.markActivePane(paneId)
        guard TerminalFocusPolicy.shouldFocusTerminal(
            appActive: NSApp.isActive,
            overlayIsKey: overlayOwnsFocus()
        ) else { return }
        guard let window else { return }
        // Surface 尚未挂进 hierarchy 时，AppKit 的 makeFirstResponder 会
        // 触发 IMK mach-port 错误。seed 完成走 onSurfaceBecameReady 再抢一次。
        guard TerminalInputFocusPolicy.shouldAttemptFocus(
            surfaceReady: terminalManager.isSurfaceReady(for: paneId),
            inWindow: view.window === window,
            windowVisible: window.isVisible,
            windowKey: window.isKeyWindow,
            appActive: NSApp.isActive
        ) else { return }
        if window.firstResponder !== view {
            window.makeFirstResponder(view)
        }
    }

    func requestSwitchTab(_ tabId: UInt32) {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.requestSwitchTab(tabId)
            }
            return
        }
        guard tabId != lastSnapshot.activeTab else { return }
        // AppKit can deliver a button action twice before the next poll updates
        // lastSnapshot. The gate is the single in-flight command for a target;
        // coalesce only the same pending target (different targets remain valid
        // rapid navigation and replace the pending request).
        if tabSwitchGate.pendingTab == tabId, !tabSwitchGate.isReleased() {
            return
        }
        let departingPane = activePaneID
        let departingLatest = departingPane.map {
            bridge.paneLatestLineSeq(paneId: $0)
        }
        tabSwitchGate.request(tab: tabId)
        content.statusBar.markCurrentWindow(tabId)
        let cachedTargetPane: UInt32?
        if let paneId = content.paneLayout.revealCachedTab(tabId) {
            cachedTargetPane = paneId
            focusPaneTerminal(paneId)
        } else {
            cachedTargetPane = nil
            needsLayoutReload = true
        }
        guard bridge.execute(task: MuxTask.switchTab(tabId)) == 0 else {
            tabSwitchGate = TabSwitchGate()
            reportStatusError(MuxtermI18n.shared.tr(.errorSwitchTab, arguments: ["id": "\(tabId)"]))
            return
        }
        // A tab switch with no cached tree has no pane to select yet. Clear
        // stale Agent/Command highlights now; the next authoritative snapshot
        // will select the target row if it is a known sidebar item.
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: tabId,
            paneId: cachedTargetPane
        )
        if let departingPane {
            recordLastSeen(for: departingPane, latest: departingLatest)
        }
        // 等 STATE_ACTIVE_TAB_CHANGED 到达后再用权威 snapshot 对齐；
        // 缓存命中时画面已经切过去了。
    }

    /// 已缓存的 tab：只挂树、对一下 snapshot，不重建、不 refresh-client -C。
    /// 返回 true 表示第一次进入且本地 layout 还没齐，需要走全量 refreshUI。
    @discardableResult
    private func applyCachedTabSwitch(_ tabId: UInt32) -> Bool {
        // requestSwitchTab 已在命令成功后记录了离开基线；同一个
        // STATE_ACTIVE_TAB_CHANGED 只是确认事件，不能再次用事件到达时
        // 更晚的 latest 覆盖原始离开位置。
        let isRequestedSwitch = tabSwitchGate.pendingTab == tabId
        if !isRequestedSwitch, let oldPane = activePaneID {
            recordLastSeen(for: oldPane)
        }
        tabSwitchGate.onTabChanged(to: tabId)
        content.statusBar.markCurrentWindow(tabId)
        let cacheHit = content.paneLayout.revealCachedTab(tabId) != nil
        if cacheHit {
            lastSnapshot = bridge.snapshot()
            content.updateTabs(lastSnapshot.tabs)
            let panes = lastSnapshot.panes.isEmpty
                ? bridge.getPanes(tabId: tabId)
                : lastSnapshot.panes
            terminalManager.updatePaneSizes(panes)
            focusVisibleTab(lastSnapshot)
            return false
        }
        let panes = bridge.getPanes(tabId: tabId)
        let layout = bridge.getLayout(tabId: tabId)
        guard FirstTabPaintPolicy.canPaintFromLocalLayout(
            paneCount: panes.count,
            hasLayout: layout != nil
        ) else {
            return true
        }
        guard content.paneLayout.apply(layout: layout, panes: panes, tabId: tabId) else {
            return true
        }
        lastSnapshot = bridge.snapshot()
        content.updateTabs(lastSnapshot.tabs)
        terminalManager.updatePaneSizes(panes)
        terminalManager.flushSeedsNow(paneIds: Set(panes.map(\.id)))
        focusVisibleTab(lastSnapshot)
        scheduleTabTreeWarmup()
        return false
    }

    private func focusVisibleTab(_ snap: FrameSnapshot) {
        if let activePane = snap.panes.first(where: \.isActive)?.id
            ?? snap.panes.first?.id
        {
            focusPaneTerminal(activePane)
        }
    }

    private func splitActivePane(horizontal: Bool) {
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.splitActivePane(horizontal: horizontal)
            }
            return
        }
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
        if pendingForegroundActivation != nil {
            performWhenForegroundReady { [weak self] in
                self?.movePane(offset: offset)
            }
            return
        }
        // `lastSnapshot` is maintained by the active poll and by the
        // background authority cache. Reading CoreBridge here would make a
        // keyboard shortcut wait for a remote tmux pause/capture round-trip.
        let snap = lastSnapshot
        let snapshotPaneIDs = snap.panes.map(\.id)
        let paneIDs = PaneNavigation.navigationPaneIDs(
            layoutPaneIDs: snap.layout?.leafPaneIDs(),
            paneIDs: snapshotPaneIDs
        )
        guard let target = PaneNavigation.target(
            paneIDs: paneIDs,
            activePaneID: activePaneID ?? snap.activePane,
            offset: offset
        ) else { return }

        guard bridge.execute(task: MuxTask.switchPane(target)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorSwitchPane, arguments: ["id": "\(target)"]))
            return
        }

        // tmux 选择另一个 pane 会自动退出 zoom；重新对目标 pane 执行
        // `resize-pane -Z`，让“切换的是另一个 pane 的全屏状态”成立。
        if terminalManager.usesClientResize {
            let wasZoomed = PaneFullscreenPolicy.zoomedPaneID(
                layoutPaneIDs: snap.layout?.leafPaneIDs() ?? [],
                paneIDs: snapshotPaneIDs
            ) != nil
            if wasZoomed,
               bridge.execute(task: MuxTask.togglePaneFullscreen(target)) != 0
            {
                reportStatusError(
                    MuxtermI18n.shared.tr(.errorCommandFailed)
                )
            }
            if wasZoomed {
                lastSnapshot.layout = .leaf(paneId: target)
                _ = content.paneLayout.apply(
                    layout: lastSnapshot.layout,
                    panes: lastSnapshot.panes,
                    tabId: lastSnapshot.activeTab
                )
            }
        } else if content.paneLayout.testFullscreenPaneID != nil {
            content.paneLayout.setFullscreenPane(paneId: target)
        }

        // select-pane 的状态事件稍后才会到达；先乐观更新焦点、tab pane
        // 高亮和快照，连续 Cmd/Alt+[ ] 因而不依赖下一次远端 poll。
        lastSnapshot.activePane = target
        lastSnapshot.panes = lastSnapshot.panes.map { pane in
            Pane(
                id: pane.id,
                cols: pane.cols,
                rows: pane.rows,
                isActive: pane.id == target
            )
        }
        content.paneLayout.markActivePane(target)
        terminalManager.focusTarget = terminalManager.view(for: target)
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: lastSnapshot.activeTab,
            paneId: target
        )
        restoreTerminalFocusIfAllowed()
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
                title: i18n.tr(.quickConnect),
                detail: i18n.tr(.quickConnectDetail),
                keywords: "quick connect workspace attention search 快速连接 工作区 提醒 搜索",
                kind: .command(.quickConnect)
            ),
            PaletteItem(
                title: i18n.tr(.newTab),
                detail: i18n.tr(.newTabDetail),
                keywords: "new tab 新建 标签页",
                kind: .command(.newTab)
            ),
            PaletteItem(
                title: i18n.tr(.renameTab),
                detail: i18n.tr(.renameTabDetail),
                keywords: "rename tab title 重命名 标签页",
                kind: .command(.renameTab)
            ),
            PaletteItem(
                title: i18n.tr(.renameWorkspace),
                detail: i18n.tr(.renameWorkspaceDetail),
                keywords: "rename workspace title 重命名 工作区",
                kind: .command(.renameWorkspace)
            ),
            PaletteItem(
                title: i18n.tr(.moveTabLeft),
                detail: i18n.tr(.moveTabLeftDetail),
                keywords: "move tab left reorder 向左 移动 标签页",
                kind: .command(.moveTabLeft)
            ),
            PaletteItem(
                title: i18n.tr(.moveTabRight),
                detail: i18n.tr(.moveTabRightDetail),
                keywords: "move tab right reorder 向右 移动 标签页",
                kind: .command(.moveTabRight)
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
                title: i18n.tr(.previousCommand),
                detail: i18n.tr(.previousCommandDetail),
                keywords: "previous command mark timeline history 上一条 命令",
                kind: .command(.previousCommand)
            ),
            PaletteItem(
                title: i18n.tr(.nextCommand),
                detail: i18n.tr(.nextCommandDetail),
                keywords: "next command mark timeline history 下一条 命令",
                kind: .command(.nextCommand)
            ),
            PaletteItem(
                title: i18n.tr(.menuSearchPanes),
                detail: i18n.tr(.searchPanesDetail),
                keywords: "search find pane terminal history 搜索 查找",
                kind: .command(.searchPanes)
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
                title: i18n.tr(.menuIncreaseFontSize),
                detail: i18n.tr(.increaseFontSizeDetail),
                keywords: "font size increase zoom text 字体 放大",
                kind: .command(.increaseFontSize)
            ),
            PaletteItem(
                title: i18n.tr(.menuDecreaseFontSize),
                detail: i18n.tr(.decreaseFontSizeDetail),
                keywords: "font size decrease zoom text 字体 缩小",
                kind: .command(.decreaseFontSize)
            ),
            PaletteItem(
                title: i18n.tr(.menuResetFontSize),
                detail: i18n.tr(.resetFontSizeDetail),
                keywords: "font size reset default 字体 重置",
                kind: .command(.resetFontSize)
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
                title: i18n.tr(.menuTabBarTop),
                detail: i18n.tr(.tabBarTopDetail),
                keywords: "tab bar position top 标签栏 顶部",
                kind: .command(.tabBarTop)
            ),
            PaletteItem(
                title: i18n.tr(.menuTabBarBottom),
                detail: i18n.tr(.tabBarBottomDetail),
                keywords: "tab bar position bottom 标签栏 底部",
                kind: .command(.tabBarBottom)
            ),
            PaletteItem(
                title: i18n.tr(.quitMuxterm),
                detail: i18n.tr(.quitMuxtermDetail),
                keywords: "quit exit 退出",
                kind: .command(.quit)
            ),
        ]

        let tabItems = tabEntriesForSwitching().map { entry in
            PaletteItem(
                title: i18n.tr(.menuSwitchTab, arguments: ["number": "\(entry.index)"]),
                detail: entry.name,
                keywords: "switch tab \(entry.index) \(entry.name) 切换 标签页",
                kind: .command(.switchTab(entry.index))
            )
        }
        if !tabItems.isEmpty {
            let insertion = min(8, items.count)
            items.insert(contentsOf: tabItems, at: insertion)
            if tabItems.count > 1 {
                items.insert(
                    PaletteItem(
                        title: i18n.tr(.switchLastTab),
                        detail: i18n.tr(.switchLastTabDetail),
                        keywords: "switch last final tab 0 最后 标签页",
                        kind: .command(.switchLastTab)
                    ),
                    at: insertion + tabItems.count
                )
            }
        }

        // detach 只对 tmux/SSH 控制 client 有意义；local shell 不能显示这个命令。
        // 关闭窗口时 CoreBridge.shutdown() 会发送 detach-client，保留 tmux session。
        if terminalManager.usesClientResize {
            items.insert(
                PaletteItem(
                    title: i18n.tr(.movePaneToNewTab),
                    detail: i18n.tr(.movePaneToNewTabDetail),
                    keywords: "move break pane new tab 移动 拆分 窗格 标签页",
                    kind: .command(.movePaneToNewTab)
                ),
                at: 5
            )
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
        case .command(.quickConnect):
            commandPalette.dismiss()
            openQuickConnect()
        case .command(.searchPanes):
            commandPalette.dismiss()
            openSearchPanel()
        case .command(.renameTab):
            commandPalette.dismiss()
            renameActiveTab()
        case .command(.renameWorkspace):
            commandPalette.dismiss()
            renameCurrentWorkspace()
        case .command(.moveTabLeft):
            commandPalette.dismiss()
            moveActiveTabLeft()
        case .command(.moveTabRight):
            commandPalette.dismiss()
            moveActiveTabRight()
        case .command(.switchTab(let oneBased)):
            commandPalette.dismiss()
            switchToTabIndex(oneBased)
        case .command(.switchLastTab):
            commandPalette.dismiss()
            switchToLastTab()
        case .command(.movePaneToNewTab):
            commandPalette.dismiss()
            moveActivePaneToNewTab()
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
        case .command(.previousCommand):
            commandPalette.dismiss()
            jumpToPreviousCommand()
        case .command(.nextCommand):
            commandPalette.dismiss()
            jumpToNextCommand()
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
        case .command(.increaseFontSize):
            commandPalette.dismiss()
            increaseTerminalFontSize(nil)
        case .command(.decreaseFontSize):
            commandPalette.dismiss()
            decreaseTerminalFontSize(nil)
        case .command(.resetFontSize):
            commandPalette.dismiss()
            resetTerminalFontSize(nil)
        case .language(let language):
            _ = MuxtermI18n.shared.setLanguage(language)
            commandPalette.present(items: rootPaletteItems())
        case .command(.theme):
            toggleTheme()
        case .command(.statusBarMode):
            toggleStatusBarMode()
        case .command(.tabBarTop):
            commandPalette.dismiss()
            setTabBarTop(nil)
        case .command(.tabBarBottom):
            commandPalette.dismiss()
            setTabBarBottom(nil)
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

    func showSessions(for target: ConnectionTarget) {
        lastPaletteError = nil
        let targetSocket = ConnectionDiscoverySocketPolicy.socket(
            for: target,
            currentSSHHost: bridge.sshAlias,
            currentSocket: bridge.socket
        )
        switch target {
        case .local:
            discovery.attachedLocalSocket = targetSocket
        case .ssh:
            discovery.attachedRemoteSocket = targetSocket
        }
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
                self.lastPaletteError = nil
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
                // W19-B：异步失败不得把面板关成只剩 New session；
                // 保留列表并显示错误，用户仍可重试/新建。
                self.content.setConnectProgress(stage: nil)
                self.lastPaletteError = error.localizedDescription
                self.reportStatusError(error.localizedDescription)
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
        let targetSocket = ConnectionDiscoverySocketPolicy.socket(
            for: target,
            currentSSHHost: bridge.sshAlias,
            currentSocket: bridge.socket
        )
        attach(target: target, session: session, resolvedSocket: targetSocket)
    }

    /// 已解析身份的直接 attach。`resolvedSocket` 允许显式 nil（默认 server），
    /// 因此不能在这里再次按当前 bridge 推断 socket。
    private func attach(
        target: ConnectionTarget,
        session: String,
        resolvedSocket: String?
    ) {
        commandPalette.dismiss()
        lastPaletteSelection = "\(target.displayName):\(session)"
        lastPaletteError = nil
        let params: (type: String, socket: String?, sshAlias: String?)
        switch target {
        case .local:
            params = ("tmux", resolvedSocket, nil)
        case .ssh(let host):
            params = ("ssh", resolvedSocket, host.alias)
        }
        let key = ConnectionKey(
            transport: params.sshAlias == nil ? "local" : "ssh",
            alias: params.sshAlias,
            session: session,
            runtime: "tmux",
            path: "",
            socket: params.socket
        )
        if let slot = connectionPool.slots[key], slot.lifecycle != .evicting {
            activate(slot: slot)
            return
        }

        // CoreBridge 的 connect 可能等待远端 tmux 初始化，放到后台线程。
        let initialClientSize = initialTmuxClientSizeHint()
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                let nextBridge = try CoreBridge.connect(
                    backendType: params.type,
                    socket: params.socket,
                    session: session,
                    sshAlias: params.sshAlias,
                    initialClientSize: initialClientSize
                )
                DispatchQueue.main.async {
                    guard let self else {
                        nextBridge.shutdown()
                        return
                    }
                    let slot = WarmConnectionSlot(key: key, bridge: nextBridge, now: 0)
                    if let initialClientSize {
                        slot.terminalManager.noteClientSize(initialClientSize)
                    }
                    self.activate(slot: slot)
                }
            } catch {
                DispatchQueue.main.async { [weak self] in
                    self?.lastPaletteError = error.localizedDescription
                    self?.showError(error)
                }
            }
        }
    }

    private func closeActiveTab() {
        guard lastSnapshot.tabs.contains(where: { $0.id == lastSnapshot.activeTab }) else { return }
        closeTab(lastSnapshot.activeTab)
    }

    func closeTab(_ tabId: UInt32) {
        guard bridge.execute(task: MuxTask.closeTab(tabId)) == 0 else {
            reportStatusError(MuxtermI18n.shared.tr(.errorCloseTab, arguments: ["id": "\(tabId)"]))
            return
        }
        // 等 TabClosed / ActiveTabChanged。点击当拍不要拆当前树。
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

        // SSH 连接状态 + 流量监控：每秒更新一次显示（不需要 60Hz）。
        let trafficTimer = Timer(timeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.updateTrafficMonitor()
        }
        RunLoop.main.add(trafficTimer, forMode: .common)
        trafficMonitorTimer = trafficTimer
    }

    /// 更新 SSH 连接状态 + 流量监控显示。
    private func updateTrafficMonitor() {
        guard !isClosing else { return }
        let summary = terminalManager.connectionSummary
        let totalBytes = terminalManager.totalBytesReceived
        content.updateConnectionStatus(
            summary,
            trafficRate: trafficRateSampler.sample(
                totalBytes: totalBytes,
                now: ProcessInfo.processInfo.systemUptime
            ),
            totalBytes: totalBytes
        )
    }

    /// 前台 Workspace：每个 pane 的 PTY 都进 Surface，不按活动 tab 过滤。
    /// 后台 Workspace 由 WarmConnectionSlot 关掉 viewCreationEnabled。
    private func shouldHandleSurfaceEvent(paneId: UInt32) -> Bool {
        SurfaceEventPolicy.shouldDeliver(
            viewCreationEnabled: true,
            hasView: terminalManager.hasView(for: paneId)
        )
    }

    func pollOnce() {
        guard !isClosing else { return }
        if pendingForegroundActivation != nil {
            // 目标 bridge 还可能被切换前的后台批次占用。只追赶已经
            // 排队的 Surface，并继续轮询其它 background slot；任何 active
            // bridge 的 FFI 都要等 retry 成功后再恢复。
            if let activeKey = connectionPool.activeKey,
               let slot = connectionPool.slots[activeKey],
               slot.lifecycle == .active
            {
                if slot.applyPendingSurfaceEvents() {
                    enqueueSurfaceCatchUp(slot)
                }
            }
            scheduleBackgroundSlotPoll()
            scheduleForegroundAuthorityRefresh()
            refreshWorkspaceSidebar()
            return
        }
        // 后台排空的事件必须先于 active bridge 的新事件交付。否则切回
        // Workspace 后，新的 PaneOutput 可能越过尚未应用的旧队列。
        if flushActiveSurfaceCatchUpBeforePoll() {
            scheduleBackgroundSlotPoll()
            return
        }
        terminalManager.beginEventBatch()
        defer { terminalManager.endEventBatch() }
        resolvePendingLastSeen()
        scheduleBackgroundSlotPoll()
        let events = bridge.pollEvents()
        if events.contains(where: { Self.tabNumberTopologyEvents.contains($0.type) }),
           let activeSlot = connectionPool.activeKey.flatMap({ connectionPool.slots[$0] })
        {
            activeSlot.invalidateTabNumbers()
        }
        // Core 已在 pollEvents() 中应用了这些状态变化。前台 Workspace 的
        // 每个 pane 都要吃 PTY（SURFACE.md §7：tab 栏上的页都算打开）。
        lastPaneOutputEventCount = events.filter { $0.isPaneOutput || $0.isPaneFrame }.count
        if let error = bridge.takeError() {
            reportStatusError(error)
        }
        var outputSeen = false
        var uiStateChanged = false
        // 轻量更新标记：tab 标题变化 / pane 尺寸变化不需要全量 snapshot。
        var needsTabRefresh = false
        var needsLightweightUpdate = false
        let deferOutputs = EventBatchPlan.hasStructuralEvent(
            types: events.map(\.type),
            requiresLayoutReload: StateEventPolicy.requiresLayoutReload
        )
        let deferSurfaceEvents = deferOutputs || events.contains(where: {
            $0.type == STATE_PANE_RESIZED
        })
        var pendingSnapshots: [(paneId: UInt32, data: Data)] = []
        var pendingFrames: [(paneId: UInt32, data: Data)] = []
        var pendingHistory: [(paneId: UInt32, data: Data)] = []
        var pendingOutputs: [(paneId: UInt32, data: Data)] = []
        for ev in events {
            if ev.isPaneClosed {
                // pane 真正关闭才销毁视图；切 tab / 布局变化保留视图状态。
                terminalManager.removePane(ev.paneId)
            } else if ev.isPaneSnapshot {
                guard shouldHandleSurfaceEvent(paneId: ev.paneId) else {
                    continue
                }
                // 结构/尺寸事件先同步模型和布局，再 reset + feed snapshot，
                // 否则 Cursor/htop 的 CUP 会按旧网格重放。
                if deferSurfaceEvents {
                    pendingSnapshots.append((paneId: ev.paneId, data: ev.data))
                } else {
                    terminalManager.handleSnapshot(paneId: ev.paneId, data: ev.data)
                }
            } else if ev.isPaneFrame {
                guard shouldHandleSurfaceEvent(paneId: ev.paneId) else {
                    continue
                }
                // full frame 和 snapshot 一样必须等结构/尺寸收敛后再进
                // SwiftTerm；但它只清可见网格，不 reset native scrollback。
                if deferSurfaceEvents {
                    pendingFrames.append((paneId: ev.paneId, data: ev.data))
                } else {
                    terminalManager.handleFrame(paneId: ev.paneId, data: ev.data)
                }
                outputSeen = true
            } else if ev.isPaneHistory {
                guard shouldHandleSurfaceEvent(paneId: ev.paneId) else {
                    continue
                }
                if deferSurfaceEvents {
                    pendingHistory.append((paneId: ev.paneId, data: ev.data))
                } else {
                    terminalManager.handleHistory(paneId: ev.paneId, data: ev.data)
                }
            } else if ev.isPaneOutput {
                guard shouldHandleSurfaceEvent(paneId: ev.paneId) else {
                    continue
                }
                // 同批有结构事件（如窗口 resize 的 %layout-change）时，htop
                // 的新尺寸重绘帧会先于模型 resize 到达，必须先收集、等布局
                // 同步完再喂；纯输出批次直接喂，避免额外延迟。
                if deferOutputs {
                    pendingOutputs.append((paneId: ev.paneId, data: ev.data))
                } else {
                    terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                }
                outputSeen = true
            } else if ev.type == STATE_ACTIVE_TAB_CHANGED {
                if applyCachedTabSwitch(ev.tabId) {
                    uiStateChanged = true
                    needsLayoutReload = true
                    statusBarNeedsRefresh = true
                }
            } else if ev.type == STATE_TAB_CLOSED {
                tabSwitchGate.onTabClosed(ev.tabId)
                removeStatusBarWindow(ev.tabId)
                statusBarNeedsRefresh = true
                uiStateChanged = true
                if TabLifecyclePaintPolicy.shouldTouchVisibleLayout(
                    closedIsVisible: lastSnapshot.activeTab == ev.tabId
                ) {
                    let nextId = bridge.snapshot().activeTab
                    if applyCachedTabSwitch(nextId) {
                        needsLayoutReload = true
                    }
                }
            } else if ev.type == STATE_TAB_ADDED {
                statusBarNeedsRefresh = true
                uiStateChanged = true
                if ev.tabId == bridge.snapshot().activeTab,
                   applyCachedTabSwitch(ev.tabId)
                {
                    needsLayoutReload = true
                }
            } else if StateEventPolicy.shouldReloadUI(
                type: ev.type,
                tabId: ev.tabId,
                activeTabId: lastSnapshot.activeTab
            ) {
                uiStateChanged = true
                needsLayoutReload = true
            } else if StateEventPolicy.changesActivePane(ev.type) {
                uiStateChanged = true
                if ev.type == STATE_ACTIVE_PANE_CHANGED {
                    if let departingPane = LastSeenNavigation.departingPane(
                        snapshotPane: lastSnapshot.activePane,
                        eventPane: ev.paneId
                    ) {
                        recordLastSeen(for: departingPane)
                    }
                    bridge.attentionOnBecameVisible(paneId: ev.paneId)
                    focusPaneTerminal(ev.paneId)
                }
            } else if ev.isBackendStatus {
                uiStateChanged = true
            } else if ev.type == STATE_STATUS_SUBSCRIPTION {
                // status-left/right 订阅推送（文档 §B+）：零轮询更新原生条。
                if !ev.name.isEmpty, let value = String(data: ev.data, encoding: .utf8) {
                    if ev.name.hasPrefix("muxterm.pane-cmd") {
                        // pane-cmd 订阅 → AttentionEngine.set_process_name（Linux 同款）。
                        // pane @0 是合法 tmux pane，不能把 0 当作“无 pane”哨兵。
                        _ = bridge.attentionSetProcessName(
                            paneId: ev.paneId,
                            name: value.isEmpty ? nil : value
                        )
                    } else {
                        content.statusBar.applySubscription(name: ev.name, value: value)
                        if let snapshot = statusBarSnapshot {
                            var updated = snapshot
                            if ev.name == "muxterm.status-left" {
                                updated.left = value
                            } else if ev.name == "muxterm.status-right" {
                                updated.right = value
                            }
                            statusBarSnapshot = updated
                        }
                    }
                }
            } else if ev.type == STATE_TAB_RENAMED {
                // 标题变化只更新 tab 列表文字，不需要全量 snapshot + 布局重建。
                // 高频输出时 TAB_RENAMED 频繁触发，走 refreshUI 会卡顿。
                needsTabRefresh = true
            } else if ev.isTabOrderChanged {
                // move-window 保持稳定 TabId 和布局，只刷新权威 tab 顺序。
                needsTabRefresh = true
                statusBarNeedsRefresh = true
            } else if ev.isWorkspaceRenamed {
                applyWorkspaceRename(ev.name)
            } else if ev.type == STATE_PANE_RESIZED {
                // pane 格子变了：立刻把 SwiftTerm 模型对齐（含缩小）。
                // 不能只记轻量更新，否则 attach 的 128x63 会钉在 93x51
                // 窗口上，prompt 掉到可见区域下面。
                if let grid = PaneGridSyncPolicy.grid(fromResizeEvent: ev.data) {
                    terminalManager.applyPaneGrid(
                        paneId: ev.paneId,
                        cols: grid.cols,
                        rows: grid.rows
                    )
                }
                needsLightweightUpdate = true
            }
            if ev.isBackendStatus {
                if ev.paneId == 0 || ev.paneId == 4 {
                    // 0 = disconnected, 4 = exited。
                    // tmux/ssh 控制模式：保留最后一帧 + 水印，不关窗（W16b）。
                    // 本地 shell 的 Exited 仍关窗（session 已结束）。
                    if terminalManager.usesClientResize {
                        content.setDisconnected(true)
                    } else if ev.paneId == 4 {
                        closeSessionWindow()
                        return
                    }
                } else {
                    content.setDisconnected(false)
                }
            }
        }
        if needsLayoutReload || uiStateChanged {
            refreshUI()
            if uiStateChanged {
                maybeCloseIfSessionEnded()
            }
        } else if needsTabRefresh {
            // 只更新 tab 列表文字，不做全量 snapshot + 布局重建。
            let tabs = bridge.getTabs()
            let activeTab = tabs.first(where: \.isActive)?.id ?? tabs.first?.id ?? 0
            lastSnapshot.tabs = tabs
            lastSnapshot.activeTab = activeTab
            content.updateTabs(tabs)
            reportPaneColoursIfNeeded(lastSnapshot.panes)
        } else if outputSeen || needsLightweightUpdate {
            content.statusBar.updateDebugSnapshot(lastSnapshot)
            // 颜色上报只依赖 refreshUI 时，attach 后没有结构事件就永远不会
            // 触发（日志里没有 refresh-client -r 的原因）。纯输出也要补报。
            reportPaneColoursIfNeeded(lastSnapshot.panes)
        }
        if statusBarNeedsRefresh {
            statusBarNeedsRefresh = false
            // 只有 tab 增删/激活才刷新 status bar（走防抖调度，避免
            // 2s 节流把切 tab 后的高亮更新吞掉）；layout-change 不触发，
            // 防止多 tab 时每次结构事件都 spawn 1+N 个子进程。
            scheduleStatusBarRefresh()
        }
        // 布局/尺寸同步完成后再喂输出，避免 resize 竞态。
        for item in pendingSnapshots {
            if shouldHandleSurfaceEvent(paneId: item.paneId) {
                terminalManager.handleSnapshot(paneId: item.paneId, data: item.data)
            }
        }
        for item in pendingFrames {
            if shouldHandleSurfaceEvent(paneId: item.paneId) {
                terminalManager.handleFrame(paneId: item.paneId, data: item.data)
            }
        }
        for item in pendingHistory {
            if shouldHandleSurfaceEvent(paneId: item.paneId) {
                terminalManager.handleHistory(paneId: item.paneId, data: item.data)
            }
        }
        for item in pendingOutputs {
            if shouldHandleSurfaceEvent(paneId: item.paneId) {
                terminalManager.handleOutput(paneId: item.paneId, data: item.data)
            }
        }
        // 结构事件同批的 snapshot 在 refreshUI 之后才喂。seed 可能还要
        // 下一拍才 ready；这里再抢一次，ready 的立刻进输入，没 ready 的
        // 等 onSurfaceBecameReady。不要每个 poll 都抢，以免打断 rename 输入。
        if deferSurfaceEvents {
            restoreTerminalFocusIfAllowed()
        }
        // Index 快照由 Core 在 poll 内消费，事件处理完成后再尝试一次，
        // 让“刚切走就还没有 PaneBuf”的首轮时序也能建立基线。
        resolvePendingLastSeen()
        refreshAttentionChrome()
        if let activePane = activePaneID {
            refreshHistoryChrome(for: activePane)
        }
    }

    /// 后台只排空 FFI。Surface feed 必须 hop 回主线程，交给那个 Workspace
    /// 自己的 TerminalManager；主线程每次只处理一个很小的时间片。
    private func scheduleBackgroundSlotPoll() {
        guard !backgroundPollInFlight else { return }
        let slots = connectionPool.slots.values.filter { $0.lifecycle == .background }
        guard !slots.isEmpty else { return }
        backgroundPollInFlight = true
        backgroundPollQueue.async { [weak self] in
            var dirty: [WarmConnectionSlot] = []
            for slot in slots {
                if slot.drainBackgroundEvents() {
                    dirty.append(slot)
                }
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.backgroundPollInFlight = false
                guard !self.isClosing else { return }
                self.enqueueSurfaceCatchUp(dirty)
                self.refreshWorkspaceSidebar()
            }
        }
    }

    /// 激活后先处理当前 Workspace 的旧 Surface 队列，再 poll 新事件，保持
    /// Runtime 输出顺序。返回 true 表示本轮仍有积压，调用方应暂缓 poll。
    private func flushActiveSurfaceCatchUpBeforePoll() -> Bool {
        guard let activeKey = connectionPool.activeKey,
              let slot = connectionPool.slots[activeKey],
              slot.lifecycle == .active,
              slot.hasPendingSurfaceWork
        else {
            return false
        }
        let hasPending = slot.applyPendingSurfaceEvents()
        if hasPending {
            enqueueSurfaceCatchUp(slot)
        }
        return hasPending
    }

    private func enqueueSurfaceCatchUp(_ slot: WarmConnectionSlot) {
        enqueueSurfaceCatchUp([slot])
    }

    private func enqueueSurfaceCatchUp(_ slots: [WarmConnectionSlot]) {
        guard !isClosing else { return }
        for slot in slots where slot.lifecycle != .evicting {
            if !surfaceCatchUpSlots.contains(where: { $0 === slot }) {
                surfaceCatchUpSlots.append(slot)
            }
        }
        scheduleSurfaceCatchUp()
    }

    private func scheduleSurfaceCatchUp() {
        guard !isClosing,
              surfaceCatchUpWorkItem == nil,
              !surfaceCatchUpSlots.isEmpty
        else {
            return
        }
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.surfaceCatchUpWorkItem = nil
            self.flushSurfaceCatchUpPass()
        }
        surfaceCatchUpWorkItem = work
        // 给 AppKit 一个机会先处理鼠标、键盘和绘制，再继续下一小拍。
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.001, execute: work)
    }

    /// 在一个全局主线程预算内轮转所有 warm Workspace，active 优先。
    private func flushSurfaceCatchUpPass() {
        guard !isClosing else {
            surfaceCatchUpSlots.removeAll()
            return
        }

        let activeKey = connectionPool.activeKey
        let slots = surfaceCatchUpSlots.sorted { lhs, rhs in
            let lhsActive = lhs.key == activeKey
            let rhsActive = rhs.key == activeKey
            if lhsActive != rhsActive {
                return lhsActive
            }
            return lhs.openedOrder < rhs.openedOrder
        }
        surfaceCatchUpSlots.removeAll()

        let started = ProcessInfo.processInfo.systemUptime
        for slot in slots where slot.lifecycle != .evicting {
            guard slot.hasPendingSurfaceWork else { continue }
            let elapsed = ProcessInfo.processInfo.systemUptime - started
            let remainingBudget = SurfaceEventBatchPolicy.timeBudget - elapsed
            guard remainingBudget > 0 else {
                surfaceCatchUpSlots.append(slot)
                continue
            }
            if slot.applyPendingSurfaceEvents(
                maxEvents: SurfaceEventBatchPolicy.maxEventsPerPass,
                timeBudget: remainingBudget
            ) {
                surfaceCatchUpSlots.append(slot)
            }
        }
        scheduleSurfaceCatchUp()
    }

    /// 注意力引擎：更新状态栏红点 + 弹出 blocked/done 通知。
    private func refreshAttentionChrome(allowBridgeQueries: Bool = true) {
        guard !isClosing, pendingForegroundActivation == nil else { return }
        // 前台 pane 输出视为已看见：CommandDone 清成 Idle（Linux 同款），
        // 前台 `sleep && echo` 不弹完成通知。
        let activePane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        if allowBridgeQueries, let activePane {
            _ = bridge.attentionOnBecameVisible(paneId: activePane)
        }
        // 红点与系统通知覆盖所有 warm Workspace；后台 bridge 仍在 core
        // 中维护 Attention 状态，不能只看当前窗口这一条连接。后台 slot
        // 的快照/通知已经由 utility poll 缓存，主线程只消费值类型副本。
        var blockedCount = 0
        let activeSnapshot: AttentionSnapshot?
        if allowBridgeQueries {
            activeSnapshot = attentionSnapshot(from: bridge)
        } else {
            activeSnapshot = connectionPool.activeKey
                .flatMap { connectionPool.slots[$0]?.cachedAttentionSnapshot }
        }
        if let activeSlot = connectionPool.activeKey.flatMap({ connectionPool.slots[$0] }) {
            if let activeSnapshot {
                activeSlot.cacheAttentionSnapshot(activeSnapshot)
            }
            if allowBridgeQueries {
                activeSlot.cacheStructuredAgents(bridge.structuredAgentSnapshot())
            }
        }
        if let activeSnapshot {
            blockedCount += activeSnapshot.blockedCount
        }
        if allowBridgeQueries {
            drainAttentionNotifications(from: bridge)
        }
        for slot in connectionPool.slots.values
            where slot.bridge !== bridge && slot.lifecycle != .evicting
        {
            if let snapshot = attentionSnapshot(for: slot) {
                blockedCount += snapshot.blockedCount
            }
            postAttentionNotifications(slot.takePendingAttentionNotifications())
        }
        content.statusBar.setAttention(StatusBarAttention(count: blockedCount))
        refreshWorkspaceSidebar()
    }

    private func drainAttentionNotifications(from candidate: CoreBridge) {
        guard let json = candidate.attentionTakeNotificationsJSON(),
              let data = json.data(using: .utf8),
              let notifications = AttentionNotifications.decode(data)
        else {
            return
        }
        postAttentionNotifications(notifications.notifications)
    }

    private func postAttentionNotifications(_ notifications: [AttentionNotification]) {
        // 新版 FFI 按 pane 提供结构化记录；旧版 decode 会把 workspace-only
        // 数组转换成同样的兼容记录。通知标题优先使用执行进程名，避免把
        // `local/node` 之类的 workspace 身份误当成 Codex/Cursor 名称。
        for notification in notifications {
            let title = notification.displayProcessName
                ?? notification.workspaceId
            let body = notification.kind == .done
                ? MuxtermI18n.shared.tr(.statusDone)
                : MuxtermI18n.shared.tr(.statusAttention)
            postNotification(title: title, body: body)
        }
    }

    /// 桌面通知（fail-soft：无通知权限时静默；测试进程只记录不弹系统通知）。
    private func postNotification(title: String, body: String) {
        let kind = body.lowercased().contains("complete")
            || body.lowercased().contains("done")
            || body.contains("完成")
            ? "done"
            : "blocked"
        recordedNotifications.append("\(title): \(kind)")
        NativeNotificationService.shared.post(title: title, body: body)
    }

    private func refreshUI() {
        guard pendingForegroundActivation == nil else { return }
        let snap = bridge.snapshot()
        tabSwitchGate.onSnapshot(tabs: snap.tabs.map(\.id))
        if !tabSwitchGate.isReleased() {
            // 乐观切 tab 已经挂了缓存树；不能用还没切过去的 snapshot 盖回去。
            reportPaneColoursIfNeeded(snap.panes)
            return
        }
        lastSnapshot = snap
        // 活动 tab 的 snap.panes 只够画当前 layout。后台 tab 的 Surface 还要
        // 跟着 pane 尺寸走，否则切回去时格子已经对了、pty 还是旧 cols。
        var allPanes: [Pane] = []
        var tabIdsByPane: [UInt32: UInt32] = [:]
        var tabNumbersByPane: [UInt32: Int] = [:]
        for (index, tab) in snap.tabs.enumerated() {
            let panes = bridge.getPanes(tabId: tab.id)
            allPanes.append(contentsOf: panes)
            for pane in panes {
                tabIdsByPane[pane.id] = tab.id
                tabNumbersByPane[pane.id] = index + 1
            }
        }
        terminalManager.updatePaneSizes(allPanes.isEmpty ? snap.panes : allPanes)
        reportPaneColoursIfNeeded(snap.panes)
        content.updateTabs(snap.tabs)
        if needsLayoutReload {
            if content.paneLayout.apply(
                layout: snap.layout,
                panes: snap.panes,
                tabId: snap.activeTab
            ) {
                needsLayoutReload = false
                content.statusBar.clearLayoutSyncError()
            } else {
                content.statusBar.showLayoutSyncing()
            }
        }
        content.paneLayout.pruneTabs(keeping: Set(snap.tabs.map(\.id)))
        content.statusBar.updateDebugSnapshot(snap)
        content.statusBar.updateOutputSnippet(terminalManager.recentOutputSnippet)
        if let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id {
            let viewport = bridge.paneViewport(paneId: activePane)
            content.setJumpLatestVisible(
                viewport > 0,
                unseenLines: terminalManager.unseenLineCount(paneId: activePane)
            )
            refreshHistoryChrome(for: activePane)
        }

        if let activePane = snap.panes.first(where: \.isActive)?.id ?? snap.panes.first?.id {
            terminalManager.focusTarget = terminalManager.view(for: activePane)
            content.paneLayout.markActivePane(activePane)
            restoreTerminalFocusIfAllowed()
        }
        applyPendingSearchJumpIfReady()
        scheduleTabTreeWarmup()
        cacheActiveSlotSnapshot(
            tabIdsByPane: snap.tabs.isEmpty ? nil : tabIdsByPane,
            tabNumbersByPane: snap.tabs.isEmpty ? nil : tabNumbersByPane
        )
    }

    private func cacheActiveSlotSnapshot(
        tabIdsByPane: [UInt32: UInt32]? = nil,
        tabNumbersByPane: [UInt32: Int]? = nil
    ) {
        guard let activeKey = connectionPool.activeKey,
              let slot = connectionPool.slots[activeKey],
              slot.bridge === bridge
        else {
            return
        }
        slot.cacheSnapshot(lastSnapshot)
        if let tabIdsByPane, let tabNumbersByPane {
            slot.cacheTabTargets(
                tabIdsByPane: tabIdsByPane,
                tabNumbersByPane: tabNumbersByPane
            )
        }
    }

    /// 每拍只预热一个还没点过的 tab，第一次点击就能走缓存树。
    private func scheduleTabTreeWarmup() {
        guard pendingForegroundActivation == nil,
              TabWarmupPolicy.canStart(
            activeSurfaceReady: activeSurfaceReadyForTabWarmup()
        ) else {
            return
        }
        guard !tabWarmupScheduled else { return }
        tabWarmupScheduled = true
        DispatchQueue.main.asyncAfter(
            deadline: .now() + TabWarmupPolicy.delayAfterFirstPaint
        ) { [weak self] in
            guard let self else { return }
            self.tabWarmupScheduled = false
            guard self.pendingForegroundActivation == nil,
                  TabWarmupPolicy.canStart(
                activeSurfaceReady: self.activeSurfaceReadyForTabWarmup()
            ) else {
                return
            }
            self.warmNextBackgroundTab()
        }
    }

    private func activeSurfaceReadyForTabWarmup() -> Bool {
        guard let activePane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        else {
            return false
        }
        return terminalManager.isSurfaceReady(for: activePane)
    }

    private func warmNextBackgroundTab() {
        guard pendingForegroundActivation == nil,
              activeSurfaceReadyForTabWarmup()
        else { return }
        let current = lastSnapshot.activeTab
        for tab in lastSnapshot.tabs where tab.id != current {
            if content.paneLayout.hasCachedTab(tab.id) { continue }
            let panes = bridge.getPanes(tabId: tab.id)
            let layout = bridge.getLayout(tabId: tab.id)
            guard FirstTabPaintPolicy.canPaintFromLocalLayout(
                paneCount: panes.count,
                hasLayout: layout != nil
            ) else {
                continue
            }
            if content.paneLayout.prewarm(tabId: tab.id, layout: layout, panes: panes) {
                scheduleTabTreeWarmup()
                return
            }
        }
    }

    /// 切 tab/pane 完成后：按 seq 喂历史帧，并用 SwiftTerm findNext 高亮 query。
    private func applyPendingSearchJumpIfReady() {
        guard pendingForegroundActivation == nil,
              let jump = pendingSearchJump
        else { return }
        guard tabSwitchGate.isReleased() else { return }
        guard lastSnapshot.panes.contains(where: { $0.id == jump.paneId }) else { return }
        pendingSearchJump = nil
        if jump.seq > 0 {
            let offset = max(0, bridge.paneViewportOffsetForSeq(paneId: jump.paneId, seq: jump.seq))
            if offset > 0 {
                let uoff = UInt32(offset)
                _ = bridge.setPaneViewport(paneId: jump.paneId, offset: uoff)
                applyPaneViewport(paneId: jump.paneId, offset: uoff)
                content.setJumpLatestVisible(true)
            }
        }
        let q = jump.query.trimmingCharacters(in: .whitespacesAndNewlines)
        if !q.isEmpty {
            let view = terminalManager.view(for: jump.paneId)
            view.clearSearch()
            _ = view.findNext(q)
        }
    }

    private func refreshLocalizedUI() {
        commandPalette.refreshLocalization()
        unifiedPanel.refreshLocalization()
        content.refreshLocalization()
        refreshUI()
    }

    /// 记录离开 pane 时最后一条稳定终端行；回到该 pane 后若 seq 前进，
    /// 显示“上次看到这里”按钮，并用 core 行索引跳回，而不是猜文本位置。
    private func recordLastSeen(for paneId: UInt32, latest: Int64? = nil) {
        // 一个离开周期只建立一次基线。tab 申请和 active-tab 确认事件
        // 都可能到这里；后到的调用不能把已经记录的离开位置推迟。
        guard lastSeenLineSeq[paneId] == nil else {
            pendingLastSeenPanes.remove(paneId)
            lastSeenJump = lastSeenJump?.paneId == paneId ? nil : lastSeenJump
            if lastSeenVisiblePane == paneId {
                setLastSeenVisible(false, paneId: paneId)
            }
            return
        }
        guard let seq = LastSeenNavigation.baselineSequence(
            latest: latest ?? bridge.paneLatestLineSeq(paneId: paneId)
        ) else {
            pendingLastSeenPanes.insert(paneId)
            return
        }
        pendingLastSeenPanes.remove(paneId)
        lastSeenLineSeq[paneId] = seq
        lastSeenJump = lastSeenJump?.paneId == paneId ? nil : lastSeenJump
        if lastSeenVisiblePane == paneId {
            setLastSeenVisible(false, paneId: paneId)
        }
    }

    /// 处理离开时 Core 尚未创建 PaneBuf 的情况。`pollEvents()` 会先在
    /// Core 内消费 Index 快照，再把可见事件交给这里，因此在一轮 poll
    /// 前后各尝试一次即可覆盖正常的异步建索引时序。
    private func resolvePendingLastSeen() {
        guard !pendingLastSeenPanes.isEmpty else { return }
        for paneId in Array(pendingLastSeenPanes) {
            recordLastSeen(for: paneId)
        }
    }

    /// 测试用：last-seen 状态机的三个输入，便于 E2E 失败定位。
    func testLastSeenDiagnostics(paneId: UInt32) -> String {
        let latest = bridge.paneLatestLineSeq(paneId: paneId)
        let seen = lastSeenLineSeq[paneId]
        let rawOffset = seen
            .map { bridge.paneViewportOffsetForSeq(paneId: paneId, seq: $0) }
            ?? -1
        return "latest=\(latest) seen=\(seen.map(String.init) ?? "nil") rawOffset=\(rawOffset)"
    }

    private func refreshHistoryChrome(for paneId: UInt32) {
        guard pendingForegroundActivation == nil, paneId == activePaneID else { return }
        let latest = bridge.paneLatestLineSeq(paneId: paneId)
        let seen = lastSeenLineSeq[paneId]
        let rawOffset = seen.map {
            bridge.paneViewportOffsetForSeq(paneId: paneId, seq: $0)
        } ?? -1
        if let offset = LastSeenNavigation.targetOffset(
            latest: latest,
            seen: seen,
            rawOffset: rawOffset
        ) {
            lastSeenJump = (paneId, offset)
            setLastSeenVisible(true, paneId: paneId)
        } else {
            // latest 没有前进、seq 已 stale 或 core 查询失败时，都必须
            // 清掉旧目标，不能保留上一轮可用的 offset。
            lastSeenJump = nil
            setLastSeenVisible(false, paneId: paneId)
        }

        var ok: (command: String, exitCode: Int, offset: UInt32)?
        var fail: (command: String, exitCode: Int, offset: UInt32)?
        for mark in bridge.paneCommandMarks(paneId: paneId).reversed() {
                // Core 返回 nil history_offset 时表示 seq 已淘汰；绝不能
                // 回退成 0，否则点击红/绿刻度会错误跳到 live 底部。
                guard let code = mark.exitCode, let offset = mark.historyOffset else { continue }
                if code == 0, ok == nil {
                    ok = (mark.command, code, offset)
                } else if code != 0, fail == nil {
                    fail = (mark.command, code, offset)
                }
                if ok != nil, fail != nil { break }
        }
        content.setCommandMarks(ok: ok, fail: fail)
    }

    private func setLastSeenVisible(_ visible: Bool, paneId: UInt32) {
        if visible {
            guard lastSeenVisiblePane != paneId else { return }
            lastSeenVisiblePane = paneId
        } else {
            // 清除来自旧 active pane 的 marker 也必须是幂等的。切 tab/pane
            // 时刷新函数传入的是新 pane，不能因为 paneId 不同而把旧按钮留在
            // 左上角，造成所有页面闪烁或残留。
            guard lastSeenVisiblePane != nil else { return }
            lastSeenVisiblePane = nil
        }
        content.setLastSeenVisible(visible)
    }

    /// session/window 已空时关闭 NSWindow。
    private func maybeCloseIfSessionEnded() {
        let snap = bridge.snapshot()
        if snap.tabs.isEmpty && snap.panes.isEmpty {
            closeSessionWindow()
        }
    }

    /// 通过 Core SettingsService 事务写配置；失败只提示，不直接改文件。
    private func persistConfig(_ operations: [[String: Any]]) {
        do {
            let transaction = try bridge.configBegin()
            try bridge.configPatch(transaction: transaction, operations: operations)
            try bridge.configCommit(transaction: transaction)
        } catch {
            reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
        }
    }

    private func reportStatusError(_ message: String) {
        content.statusBar.showError(message)
    }

    /// 新出现的 pane 需要把客户端主题色上报给 tmux，否则 tmux 代答
    /// OSC 10/11 颜色查询时用的是自己的默认色板（codex 黑底黑字/白底白字）。
    private func reportPaneColoursIfNeeded(_ panes: [Pane]) {
        guard terminalManager.usesClientResize else { return }
        let fresh = Set(panes.map(\.id)).subtracting(reportedColourPanes)
        guard !fresh.isEmpty else { return }
        let osc = ColorContrast.oscColors(
            fg: MuxtermTerminalColors.activePalette.fg,
            bg: MuxtermTerminalColors.activePalette.bg
        )
        for id in fresh {
            if bridge.reportPaneColours(paneId: id, fgHex: osc.fg, bgHex: osc.bg) == 0 {
                reportedColourPanes.insert(id)
            }
        }
    }

    /// 本地移除 statusbar 里已关闭的窗口条目（前端驱动，立即反馈）。
    private func removeStatusBarWindow(_ tabId: UInt32) {
        guard let snapshot = statusBarSnapshot else { return }
        let updated = snapshot.removingWindow(tabId)
        guard updated.windows.count != snapshot.windows.count else { return }
        statusBarSnapshot = updated
        content.applyStatusBar(updated)
    }

    /// 抓取并应用 tmux status bar 快照（只读查询，后台执行）。
    private func refreshStatusBar(force: Bool) {
        guard terminalManager.usesClientResize,
              pendingForegroundActivation == nil
        else { return }
        if !force, Date().timeIntervalSince(lastStatusFetchAt) < 2 {
            return
        }
        lastStatusFetchAt = Date()
        let bridge = self.bridge
        let slot = connectionPool.slots.values.first { $0.bridge === bridge }
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result: (json: String?, subscriptions: Bool)?
            if let slot {
                result = slot.withBridge { candidate in
                    (
                        json: candidate.statusBarSnapshotJSON(),
                        subscriptions: candidate.statusSubscriptionActive()
                    )
                }
            } else {
                result = (
                    json: bridge.statusBarSnapshotJSON(),
                    subscriptions: bridge.statusSubscriptionActive()
                )
            }
            guard let result,
                  let json = result.json,
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
                guard self.bridge === bridge,
                      self.pendingForegroundActivation == nil
                else { return }
                self.statusBarSnapshot = snapshot
                self.content.applyStatusBar(snapshot)
                // 文档 §B+：tmux ≥3.2 用 refresh-client -B 订阅推送（零轮询）；
                // 只有老版本才保留 status-interval 轮询定时器。
                if !result.subscriptions {
                    self.scheduleStatusRefresh(snapshot)
                }
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

    /// 测试用：关掉桥接和窗口，不走 Exited 业务路径。
    func testShutdown() {
        guard !isClosing else { return }
        isClosing = true
        // 统一面板是独立 NSPanel：不关会留在 NSApp.windows 里干扰后续测试。
        unifiedPanel?.dismiss()
        // 命令面板同样是独立浮动窗口；只关主窗口会让它继续挡住
        // 后续窗口的键盘事件（尤其是 Cmd-Shift-P / tab 切换）。
        commandPalette?.window?.orderOut(nil)
        // 主题外观复位，避免后续测试读到残留 dark appearance。
        window?.appearance = nil
        content.appearance = nil
        NSApp.appearance = nil
        pollTimer?.invalidate()
        pollTimer = nil
        trafficMonitorTimer?.invalidate()
        trafficMonitorTimer = nil
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        statusRefreshWorkItem?.cancel()
        statusRefreshWorkItem = nil
        cancelSurfaceCatchUp()
        backgroundPollQueue.sync {}
        connectionPool.shutdownAll()
        bridge.shutdown()
        window?.close()
    }

    private func closeSessionWindow() {
        guard !isClosing else { return }
        isClosing = true
        unifiedPanel?.dismiss()
        commandPalette?.window?.orderOut(nil)
        pollTimer?.invalidate()
        pollTimer = nil
        trafficMonitorTimer?.invalidate()
        trafficMonitorTimer = nil
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        statusRefreshWorkItem?.cancel()
        statusRefreshWorkItem = nil
        cancelSurfaceCatchUp()
        backgroundPollQueue.sync {}
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
        unifiedPanel?.dismiss()
        commandPalette?.window?.orderOut(nil)
        pollTimer?.invalidate()
        pollTimer = nil
        trafficMonitorTimer?.invalidate()
        trafficMonitorTimer = nil
        statusRefreshTimer?.invalidate()
        statusRefreshTimer = nil
        statusRefreshWorkItem?.cancel()
        statusRefreshWorkItem = nil
        cancelSurfaceCatchUp()
        // Task::Detach 已关闭 control channel；这里仅回收 core handle，
        // 不会再次发送 detach-client 或杀 tmux session。
        bridge.shutdown()
        window?.close()
    }

    // MARK: - 快捷键

    private func installKeyEquivalents() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.routeMonitoredKeyEvent(event)
        }
    }

    /// local monitor 的单事件决策，拆出供真实 `NSApp.sendEvent` 回归检查。
    /// 返回 nil 表示 monitor 已消费；返回 event 表示继续 AppKit responder chain。
    func routeMonitoredKeyEvent(_ event: NSEvent) -> NSEvent? {
        // CommandPalette/UnifiedPanel/ProjectConfig are independent NSPanel
        // windows. Their text fields own Backspace and other editing keys;
        // the main terminal shortcut router must never consume those events.
        if let eventWindow = event.window, eventWindow !== window {
            return event
        }
        if handleKey(event) {
            return nil
        }
        // 普通文字、IME 组合/提交和 Enter 必须原样交还 AppKit。local monitor
        // 回调栈里手工调用 SwiftTerm keyDown/interpretKeyEvents 会绕开正常的
        // responder/IMK 调度，并可能触发 IMKCFRunLoopWakeUpReliable mach-port
        // 错误。真正的窗口快捷键已由 handleKey 消费并在上面返回 nil。
        return event
    }

    /// 返回 true 表示已消费事件。in-process e2e 经 `testDispatchKeyEvent` 调用。
    func handleKey(_ event: NSEvent) -> Bool {
        if let eventWindow = event.window, eventWindow !== window {
            return false
        }
        let eventFlags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let isReturn = event.keyCode == 36 || event.keyCode == 76
        // Cmd-P 统一面板可见时，Tab/Shift+Tab/Esc/Enter 走面板。
        // （headless e2e 里 key window 可能为 nil，用 isVisible 判断。）
        if unifiedPanel?.window?.isVisible == true {
            switch event.keyCode {
            case 53: // Escape
                unifiedPanel.dismiss()
                return true
            case 48: // Tab
                unifiedPanel.cycleTabForTest(back: event.modifierFlags.contains(.shift))
                return true
            case 36, 76: // Return / keypad Enter
                if eventFlags.contains(.command) {
                    // Cmd-Enter：注意力面板 → replica overlay；否则主窗口 zoom。
                    toggleReplyOverlay()
                    return true
                }
                unifiedPanel.activateForTest()
                return true
            default:
                break
            }
        }
        // overlay 已打开：再按 Cmd-Enter 关掉。
        if !content.replyOverlayContainer.isHidden, eventFlags.contains(.command), isReturn {
            toggleReplyOverlay()
            return true
        }
        if isReturn {
            // 落到下面的 KeyChord 匹配（Cmd-Enter 主窗口 zoom）。
        }
        let flags = eventFlags
        // macOS 的 Delete/Backspace 可能在 SwiftTerm 的 NSTextInputClient 路径
        // 中被吞掉；明确转成 DEL，保证 shell 和 tmux 收到基础编辑键。
        if event.keyCode == 51,
           !flags.contains(.option),
           let view = window?.firstResponder as? MuxTerminalView
        {
            // 输入法候选态（marked text）：Backspace 必须交给 IME 处理，
            // 否则会把 DEL 发给终端，误删输入框里已经提交的原文。
            if view.hasMarkedText() {
                return false
            }
            if flags.contains(.command), !flags.contains(.control) {
                // Cmd+Backspace → 删到行首（Ctrl-U）。不交给 SwiftTerm，
                // 否则落到 Unhandle selector deleteToBeginningOfLine:。
                terminalManager.sendRawInput(to: view, byte: 0x15)
                return true
            }
            if !flags.contains(.command) {
                terminalManager.sendRawInput(to: view, byte: TerminalInputEncoding.backspaceByte)
                return true
            }
        }
        // Return / keypad Enter 在带修饰键时 charactersIgnoringModifiers
        // 可能为空，不能靠字符匹配，否则 Cmd/Alt+Enter 永远进不了全屏。
        let key: String
        if event.keyCode == 36 || event.keyCode == 76 {
            key = "\r"
        } else if event.keyCode == 126 {
            key = "up"
        } else if event.keyCode == 125 {
            key = "down"
        } else if let raw = event.charactersIgnoringModifiers, let first = raw.first {
            key = String(first)
        } else {
            return false
        }
        let chord = KeyChord(
            command: flags.contains(.command),
            shift: flags.contains(.shift),
            option: flags.contains(.option),
            control: flags.contains(.control),
            key: key
        )
        if TerminalEditShortcutPolicy.shouldDeferToMenu(
            command: chord.command,
            shift: chord.shift,
            option: chord.option,
            control: chord.control,
            key: chord.key
        ), let view = window?.firstResponder as? MuxTerminalView {
            switch chord.key {
            case "c":
                view.copy(nil)
            case "v":
                view.paste(nil)
            case "a":
                view.selectAll(nil)
            default:
                break
            }
            return true
        }
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
        case .switchLastTab:
            switchToLastTab()
        case .nextPane:
            nextPane()
        case .prevPane:
            prevPane()
        case .previousCommand:
            jumpToPreviousCommand()
        case .nextCommand:
            jumpToNextCommand()
        case .commandPalette:
            openCommandPalette()
        case .quickConnect:
            openQuickConnect()
        case .attention:
            openAttentionPanel()
        case .searchWorkspace:
            openSearchPanel(scope: .workspace)
        case .searchGlobal:
            openSearchPanel(scope: .all)
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
        case .toggleSidebar:
            toggleWorkspaceSidebar()
        case .switchWorkspace(let n):
            switchToWorkspaceAtFixedIndex(n)
        }
        return true
    }

    func windowDidBecomeKey(_ notification: Notification) {
        restoreTerminalFocusIfAllowed()
    }

    func windowWillClose(_ notification: Notification) {
        removeKeyMonitor()
        if !isClosing {
            isClosing = true
            pollTimer?.invalidate()
            pollTimer = nil
            trafficMonitorTimer?.invalidate()
            trafficMonitorTimer = nil
            statusRefreshTimer?.invalidate()
            statusRefreshTimer = nil
            statusRefreshWorkItem?.cancel()
            statusRefreshWorkItem = nil
            cancelSurfaceCatchUp()
            backgroundPollQueue.sync {}
            connectionPool.shutdownAll()
            bridge.shutdown()
        }
    }

    private func cancelSurfaceCatchUp() {
        surfaceCatchUpWorkItem?.cancel()
        surfaceCatchUpWorkItem = nil
        surfaceCatchUpSlots.removeAll()
    }

    private func removeKeyMonitor() {
        guard let keyMonitor else { return }
        NSEvent.removeMonitor(keyMonitor)
        self.keyMonitor = nil
    }
}

// MARK: - TerminalInputHandler（reply overlay 输入）

extension MainWindowController: TerminalInputHandler {
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        let paneId = replyOverlayPaneId ?? view.paneId
        let payload = Data(data)
        // W19-E：overlay 快速回复不清 Blocked（注意力行保留，Enter 仍可跳转）。
        performWhenForegroundReady { [weak self] in
            _ = self?.bridge.sendInputQuiet(paneId: paneId, data: payload)
        }
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {
        // overlay 不写回 tmux 尺寸（不改主布局 PTY）。
    }
}
