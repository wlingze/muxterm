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

    private struct CatalogConnection {
        let bridge: CoreBridge
        let target: TargetConfig
    }

    /// Frontend-only placeholder shown while Core resolves/opens a Workspace.
    /// It is never inserted into WorkspacePool and owns no Runtime/connection.
    private struct PendingWorkspaceOpen {
        let id: String
        let config: TargetConfig
        var stage: ConnectProgressStage
    }

    /// 当前窗口展示的是一个真实 Workspace，还是前端聚合槽。
    /// 聚合槽只保存源身份；Core 仍然只看到真实 Workspace/Tab/Pane。
    private enum WorkspacePresentation: Equatable {
        case workspace
        case shells(workspaceId: String?, tabId: UInt32?)
        case agents(AgentAggregateKey?)
        case connecting(String)
    }

    var bridge: CoreBridge
    var terminalManager: TerminalManager
    let content: ContentView
    let workspaceSidebar = WorkspaceSidebarView(frame: NSRect(x: 0, y: 0, width: 240, height: 640))
    private let mainSplitController = NSSplitViewController()
    private var sidebarSplitItem: NSSplitViewItem?
    private let discovery = ConnectionDiscovery()
    var commandPalette: CommandPaletteController!
    var unifiedPanel: UnifiedPanelController!
    private var settingsWindow: SettingsWindowController?
    /// 来自 ~/.config/muxterm/config.toml 的自定义快捷键（可选）。
    private var customKeybindings: [KeyChord: KeyAction] = [:]
    private var nextWorkspaceOpenedOrder: UInt64 = 1
    private var workspacePresentation: WorkspacePresentation = .workspace
    /// Agents 是前端投影槽；离开后仍应回到用户最后查看的源 Tab。
    private var lastAgentAggregateKey: AgentAggregateKey?
    private var pendingWorkspaceOpen: PendingWorkspaceOpen?
    /// 在 Agents 槽关闭一页只隐藏投影，不关闭源 pane。源 agent 消失后会清理。
    private var hiddenAgentTabs = Set<AgentAggregateKey>()
    /// 用户在 Attention 中隐藏的 pane。只影响前端投影，不确认或静音 Core。
    private var hiddenAttentionKeys = Set<AttentionVisibilityKey>()
    private var structuredAgentTestOverrides: [String: [StructuredPaneAgent]] = [:]
    private var quickConnectStore: QuickConnectStore!
    private var imagePasteBridge: CoreBridge?
    private var pollTimer: Timer?
    /// 主窗口 local key monitor 的 token；独立 NSPanel 的事件不能进入这里。
    private var keyMonitor: Any?
    var lastSnapshot = FrameSnapshot()
    /// Core 整池 Attention 快照。侧栏 Agents/Commands 与 Attention 面板共用，
    /// 避免再按 replica 切片后对不上 herdr 的 wN。
    private var lastPoolAttentionSnapshot: AttentionSnapshot?
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
    private var replyOverlayWorkspaceID: String?
    /// 搜索跳转：切 tab 完成后再滚到命中行。
    private var pendingSearchJump: PendingSearchJump?
    /// 面板行可能在后台 Workspace 的身份缓存到达前被选中。保留最后一次
    /// 跳转请求，下一轮 poll 再解析，不能因为一次非阻塞 identity 查询失败
    /// 就把用户动作静默丢掉。
    private struct PendingPanelJump {
        let workspaceId: String?
        let tabId: UInt32?
        let paneId: UInt32
        let seq: UInt64
        let query: String
    }
    private var pendingPanelJump: PendingPanelJump?
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
    /// last-seen 只在返回 pane 后短暂提供入口，避免长期盖住终端内容。
    private var lastSeenVisibleUntil: TimeInterval?
    private static let lastSeenPresentationDuration: TimeInterval = 4
    /// Core 在索引快照切换时可能短暂返回 rawOffset=-1。保留已经展示的
    /// marker 一个很短的窗口，避免用户点击时目标被一次瞬时查询失败清掉。
    private var lastSeenOffsetFailureSince: [UInt32: TimeInterval] = [:]
    private static let lastSeenOffsetFailureGrace: TimeInterval = 1.0
    /// 当前 pane 在命令时间线中的游标；手动滚轮/搜索会清掉游标，
    /// Cmd+Option+↑/↓ 则按此游标前后移动。
    private var commandTimelineCursor: [UInt32: UInt64] = [:]
    /// Event-pump snapshot of OSC 133 marks. The workspace key prevents a
    /// delayed pane-id reuse from exposing another scene's command history.
    private struct CommandMarksKey: Hashable {
        let workspaceID: String?
        let paneID: UInt32
    }
    private var commandMarksCache: [CommandMarksKey: [CoreCommandMark]] = [:]
    private struct PaneHistoryKey: Hashable {
        let workspaceID: String?
        let paneID: UInt32
    }
    private struct PaneHistoryOffsetKey: Hashable {
        let workspaceID: String?
        let paneID: UInt32
        let seq: UInt64
    }
    /// Event-pump snapshots used by last-seen UI and test diagnostics. UI
    /// callbacks must not re-enter the Core index for these values.
    private var paneLatestLineSeqCache: [PaneHistoryKey: Int64] = [:]
    private var paneViewportOffsetForSeqCache: [PaneHistoryOffsetKey: Int32] = [:]
    /// Native scroll callbacks already carry the local viewport offset. Keep
    /// it for UI-only unseen-line updates; the event pump refreshes it from
    /// Core when the authoritative snapshot changes.
    private(set) var viewportOffsets: [UInt32: UInt32] = [:]
    /// 程序化命令跳转触发 native scroll callback 时保留游标一次。
    private var commandNavigationPanes = Set<UInt32>()
    /// 最近一次 poll 的 PaneOutput 条数（W13 洪水上限）。
    private(set) var lastPaneOutputEventCount: Int = 0
    private var languageObserver: NSObjectProtocol?
    private struct ColourPaneKey: Hashable {
        let workspaceID: String?
        let paneID: UInt32
    }

    /// 已向 tmux 上报过颜色的 workspace/pane（`refresh-client -r` 只需每个
    /// pane 一次；外观变化时清空重报）。
    private var reportedColourPanes = Set<ColourPaneKey>()
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
    /// UI tasks are owned until the single main-thread event pump dispatches
    /// them to Core.  The queue stores the workspace identity so a fast scene
    /// switch cannot retarget a command that was already clicked.
    private var commandQueue = MacCommandQueue()
    /// Attention mutations update the panel after the queued Core command has
    /// crossed the event-pump boundary, rather than refreshing stale data from
    /// the click handler.
    private var attentionPanelRefreshPending = false
    /// A shared Core handle is temporarily owned by a catalog open operation;
    /// the main-thread event pump pauses until the owned result is installed.
    private var sharedCoreOperationInFlight = false
    /// Configuration completions are retained until the queued transaction is
    /// executed by the event pump.  This keeps Settings from closing before
    /// Core has accepted its draft.
    private var nextConfigRequestID: UInt64 = 1
    private var pendingConfigCompletions: [UInt64: (Result<Void, Error>) -> Void] = [:]
    /// Search completions are delivered by the event pump so panel text input
    /// never calls the Core index synchronously.
    private var nextSearchRequestID: UInt64 = 1
    private var pendingSearchCompletions: [UInt64: ([SearchHit]) -> Void] = [:]
    /// Pane-output snapshots are read only by the event pump and delivered to
    /// the overlay only if that exact overlay is still mounted.
    private var nextPaneOutputRequestID: UInt64 = 1
    private var pendingPaneOutputCompletions: [UInt64: (Data) -> Void] = [:]
    /// Core work needed after a cached scene switch.  It is deliberately
    /// resumed by `pollOnce()`, never from the click/activation stack.
    private var deferredBridgeWorkScene: WorkspaceScene?
    /// 后台 poll 只把 Surface 事件交回主线程；主线程按小批次追赶，避免
    /// 一个高流量远端 pane 把切换、输入和窗口事件挤出 run loop。
    private var surfaceCatchUpScenes: [WorkspaceScene] = []
    private var surfaceCatchUpWorkItem: DispatchWorkItem?
    /// Surface 积压时降低 attention JSON/sidebar 刷新频率。
    private var lastAttentionChromeRefreshAt: TimeInterval = 0
    /// SceneStack：已打开 Workspace 的 scene 从 open 到 close 常驻；
    /// 隐藏 scene 不参与绘制，容量/TTL/memory pressure 只在明确关闭时回收。
    private let sceneStack: SceneStack<WorkspaceScene>
    /// 已针对 scene 数量显示过一次容量提醒；用户选择保留后不在每拍
    /// 重复打断，数量变化（新建或关闭）后才重新评估。
    private var capacityWarningPresentedForSceneCount: Int?
    /// 终端字体配置（config.toml `[font]`；Cmd +/- 缩放时保留 family）。
    private var terminalFontSettings: MuxtermTerminalFont.Settings
    /// Cmd +/- / Cmd 0 只写 Core `[font] size`；不再使用 UserDefaults 覆盖。
    private var configuredFontSize: CGFloat = MuxtermTerminalFont.defaultSize
    /// 当前窗口已应用的主题。配置持久化经主线程 event pump 异步提交，
    /// UI 在提交前不能回读 Core 旧快照，否则连续切换会两次都选中同一主题。
    private var appliedTheme = MuxtermTheme.light

    /// Core 解析后的配置快照（`configDescribeJSON` → `data.values`）。
    private struct ResolvedSettings {
        var fontFamily = MuxtermTerminalFont.defaultFamily
        var fontSize = MuxtermTerminalFont.defaultSize
        var themeName = "light"
        var statusBarMode = StatusBarMode.tmux
        var tabBarPosition = TabBarPosition.bottom
        var tabBarStyle = TabBarStyle.equalWidth
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
        if let ui = values["ui"] as? [String: Any] {
            if let position = ui["tab_bar_position"] as? String,
               let parsed = TabBarPosition(rawValue: position) {
                resolved.tabBarPosition = parsed
            }
            if let style = ui["tab_bar_style"] as? String,
               let parsed = TabBarStyle(rawValue: style) {
                resolved.tabBarStyle = parsed
            }
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
        let initialWorkspace = bridge.workspaceList().first
        let initialWorkspaceID = initialWorkspace?.id
        appliedTheme = MuxtermTheme.from(name: resolved.themeName)
        MuxtermTerminalColors.activePalette = appliedTheme.palette
        configuredFontSize = MuxtermTerminalFont.clamp(resolved.fontSize)
        terminalFontSettings = MuxtermTerminalFont.Settings(
            family: resolved.fontFamily,
            size: configuredFontSize
        )
        self.bridge = bridge
        self.terminalManager = TerminalManager(
            bridge: bridge,
            workspaceID: initialWorkspaceID,
            runtimeID: initialWorkspace?.runtime,
            fontFamily: terminalFontSettings.family,
            fontSize: terminalFontSettings.size
        )
        self.content = ContentView(terminalManager: terminalManager)
        content.statusBar.setDebug(debug)
        content.statusBar.colorMode = resolved.statusBarMode
        content.statusBar.tabBarStyle = resolved.tabBarStyle
        content.applyTabBarPosition(resolved.tabBarPosition)
        sceneStack = SceneStack(
            policy: SceneStackPolicy(maxScenes: resolved.poolMaxSlots)
        )

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Muxterm"
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.titlebarSeparatorStyle = .none
        // 终端必须完整接收 mouseDown/drag 做文本选择；窗口移动只由顶部
        // StatusBarView 的空白区域显式处理。
        window.isMovableByWindowBackground = false
        // Muxterm 的顶部状态栏已经承担窗口 chrome；隐藏悬浮在内容上的
        // traffic lights，把整行宽度留给 Workspace 与 tab。
        window.standardWindowButton(.closeButton)?.isHidden = true
        window.standardWindowButton(.miniaturizeButton)?.isHidden = true
        window.standardWindowButton(.zoomButton)?.isHidden = true
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

        super.init(window: window)
        // Project 列表来自 Core 快照；变更通过主线程 event pump 排队写回
        // 统一 config.toml（`[[projects]]`），不再从 UI 回调直接碰 Core。
        if let injectedQuickConnectStore {
            quickConnectStore = injectedQuickConnectStore
        } else {
            quickConnectStore = QuickConnectStore(projects: resolved.projects) { [weak self] updated in
                guard let self else { return }
                let operations: [[String: Any]] = [[
                    "op": "replace",
                    "path": "/projects",
                    "value": QuickConnectStore.projectJSON(from: updated),
                ]]
                _ = self.enqueueConfigTransaction(operations) { result in
                    if case .failure(let error) = result {
                        // 失败时保留内存列表，不覆盖用户文件；下次启动仍读 Core 快照。
                        CoreBridge.log(
                            "failed to persist projects: \(error.localizedDescription)",
                            level: "error"
                        )
                    }
                }
            }
        }
        window.delegate = self
        installMainSplit(in: window)
        content.statusBar.onToggleSidebar = { [weak self] in
            self?.toggleWorkspaceSidebar()
        }
        content.statusBar.onWorkspaceClick = { [weak self] in
            self?.openQuickConnect()
        }
        wireTerminalManagerCallbacks()

        workspaceSidebar.onWorkspaceActivate = { [weak self] workspaceId in
            self?.activateSidebarWorkspace(workspaceId)
        }
        workspaceSidebar.onWorkspaceClose = { [weak self] workspaceId in
            self?.closeWorkspace(workspaceId)
        }
        workspaceSidebar.onWorkspaceReorder = { [weak self] ids in
            self?.reorderWorkspaces(ids)
        }
        workspaceSidebar.onAgentActivate = { [weak self] workspaceId, tabId, paneId in
            guard let self else { return }
            let resolvedTabId = tabId
                ?? self.scene(forWorkspaceId: workspaceId)?.cachedTabIdsByPane?[paneId]
            guard let resolvedTabId else {
                self.routePanelJump(
                    workspaceId: workspaceId,
                    tabId: tabId,
                    paneId: paneId,
                    seq: 0,
                    query: ""
                )
                return
            }
            self.activateAgents(preferred: AgentAggregateKey(
                workspaceId: workspaceId,
                sourceTabId: resolvedTabId
            ))
        }
        workspaceSidebar.onCommandActivate = { [weak self] workspaceId, tabId, paneId in
            self?.routePanelJump(
                workspaceId: workspaceId,
                tabId: tabId,
                paneId: paneId,
                seq: 0,
                query: ""
            )
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
            sendInput: { [weak self] paneId, data in
                guard let self else { return }
                self.performIfWindowOpen {
                    _ = self.enqueueCoreInput(
                        workspaceID: self.activeSceneWorkspaceID,
                        paneId: paneId,
                        data: data
                    )
                }
            },
            search: { [weak self] request in
                guard let self else {
                    request.completion([])
                    return
                }
                self.requestSearchHitsForPanel(
                    query: request.query,
                    scope: request.scope,
                    completion: request.completion
                )
            },
            filterSearchHits: { [weak self] hits, scope in
                self?.filterSearchHitsForPanel(hits, scope: scope) ?? []
            },
            workspaceIndex: { [weak self] config in
                self?.workspaceShortcutIndex(for: config)
            },
            connectedWorkspaces: { [weak self] in
                self?.sceneStack.allRecentTargetConfigs() ?? []
            },
            sidebarWorkspaces: { [weak self] in
                self?.sidebarItems() ?? []
            },
            activityWorkspaces: { [weak self] in
                self?.runtimeSidebarItems() ?? []
            }
        )
        unifiedPanel.onWorkspaceActivate = { [weak self] workspaceId in
            self?.activateSidebarWorkspace(workspaceId)
        }
        unifiedPanel.onWorkspaceClose = { [weak self] workspaceId in
            self?.closeWorkspace(workspaceId)
        }
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
            self?.routePanelJump(
                workspaceId: workspaceId,
                tabId: tabId,
                paneId: paneId,
                seq: seq,
                query: query
            )
        }
        unifiedPanel.onPreview = { [weak self] workspaceId, paneId in
            guard let self, self.activateWorkspaceIfAvailable(workspaceId) else { return }
            self.performIfWindowOpen { [weak self] in
                self?.toggleReplyOverlay(paneId: paneId, workspaceId: workspaceId)
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
        unifiedPanel.isAttentionHidden = { [weak self] key in
            self?.hiddenAttentionKeys.contains(key) == true
        }
        unifiedPanel.onAttentionVisibilityChange = { [weak self] key, hidden in
            guard let self else { return }
            if hidden {
                self.hiddenAttentionKeys.insert(key)
            } else {
                self.hiddenAttentionKeys.remove(key)
            }
            self.refreshAttentionChrome(allowBridgeQueries: false, force: true)
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
            self?.renamePresentedTab(tabId)
        }
        content.statusBar.onCloseTab = { [weak self] tabId in
            self?.closeTab(tabId)
        }
        content.statusBar.onMoveTab = { [weak self] from, target, before in
            _ = self?.movePresentedTab(from: from, target: target, before: before)
        }
        content.statusBar.allowsTabReordering = terminalManager.usesClientResize
        content.paneLayout.onActivatePane = { [weak self] paneId in
            guard let self else { return }
            self.performIfWindowOpen { [weak self] in
                guard let self else { return }
                let changed = self.lastSnapshot.activePane != paneId
                self.activatePaneLocally(paneId)
                self.focusPaneTerminal(paneId)
                // 同一 pane 内的点击/拖选只恢复原生焦点，不重复请求远端切换。
                guard changed else { return }
                _ = self.enqueueCoreTask(
                    MuxTask.switchPane(paneId),
                    failureMessage: MuxtermI18n.shared.tr(
                        .errorSwitchPane,
                        arguments: ["id": "\(paneId)"]
                    )
                )
            }
        }
        content.paneLayout.onSurfaceBecameReady = { [weak self] paneId, ready in
            guard let self else { return }
            let active = self.lastSnapshot.panes.first(where: \.isActive)?.id
                ?? self.lastSnapshot.panes.first?.id
            guard TerminalInputFocusPolicy.shouldRetryWhenSurfaceReady(
                isActivePane: active == paneId,
                ready: ready
            ) else { return }
            self.focusPaneTerminal(paneId)
        }
        content.paneLayout.onMovePaneToNewTab = { [weak self] paneId in
            _ = self?.movePane(paneId, toTab: nil)
        }
        content.paneLayout.onSwapPanes = { [weak self] a, b in
            _ = self?.swapPanes(a, b)
        }
        content.paneLayout.moveDestinationsProvider = { [weak self] in
            self?.paneMoveDestinations() ?? []
        }
        content.paneLayout.onPaneTitleAction = { [weak self] paneId, action in
            guard let self else { return }
            switch action {
            case .splitHorizontal:
                self.splitPane(paneId, horizontal: true)
            case .splitVertical:
                self.splitPane(paneId, horizontal: false)
            case .fullscreen:
                self.togglePaneFullscreen(paneId)
            case .cycleLayout:
                self.cycleActiveLayout()
            case .moveToNewTab:
                _ = self.movePane(paneId, toTab: nil)
            case .moveToTab(let tabId):
                _ = self.movePane(paneId, toTab: tabId)
            case .close:
                self.closePane(paneId)
            }
        }
        content.paneLayout.allowsPaneBreak = terminalManager.supportsMultiTab
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
        content.statusBar.onConnectionRefresh = { [weak self] in
            self?.updateTrafficMonitor()
        }
        // 铃铛始终打开 Attention 面板。
        content.statusBar.onAttentionClick = { [weak self] in
            self?.openAttentionPanel()
        }
        content.jumpLatestButton.target = self
        content.jumpLatestButton.action = #selector(jumpToLatest)
        content.lastSeenButton.target = self
        content.lastSeenButton.action = #selector(jumpToLastSeen)
        content.commandMarkRail.onSelectMark = { [weak self] mark in
            guard let self, let pane = self.activePaneID else { return }
            self.commandTimelineCursor[pane] = mark.seq
            self.commandNavigationPanes.insert(pane)
            self.applyPaneViewport(paneId: pane, offset: mark.offset)
        }
        content.commandMarkRail.onSelectOffset = { [weak self] offset in
            guard let self, let pane = self.activePaneID else { return }
            if offset == 0 {
                self.jumpToLatest()
            } else {
                self.applyPaneViewport(paneId: pane, offset: offset)
            }
        }
        terminalManager.onOutputSnippetChanged = { [weak self] snippet in
            self?.content.statusBar.updateOutputSnippet(snippet)
        }

        // 启动时由 AppDelegate 创建的首个连接也属于当前 Workspace。
        // 过去只有 Quick Connect 后续创建的连接才登记进池，导致初始 local
        // workspace 既不在 Recent，也无法在切走后保持常驻。
        let initialTarget = bridge.resolvedTargetConfig
        let initialKey = SceneKey(
            transport: bridge.sshAlias == nil ? "local" : "ssh",
            alias: bridge.sshAlias,
            session: initialTarget?.session ?? bridge.session ?? "",
            runtime: initialTarget?.runtime.rawValue
                ?? (terminalManager.usesClientResize ? "tmux" : "shell"),
            path: initialTarget?.path ?? bridge.startDirectory ?? "",
            socket: initialTarget?.socket ?? bridge.socket,
            workspaceID: initialTarget?.workspaceID
        )
        let initialSlot = WorkspaceScene(
            key: initialKey,
            bridge: bridge,
            workspaceID: initialWorkspaceID,
            terminalManager: terminalManager,
            targetConfig: initialTarget,
            now: 0
        )
        initialSlot.openedOrder = nextWorkspaceOpenedOrder
        nextWorkspaceOpenedOrder += 1
        sceneStack.activate(key: initialKey) { _ in initialSlot }
        bridge.selectWorkspace(initialWorkspaceID)
        if initialSlot.targetConfig.runtime == .shell {
            workspacePresentation = .shells(
                workspaceId: workspaceReplicaID(for: initialSlot),
                tabId: initialSlot.lastSnapshot.activeTab
            )
        }

        content.updateBanner.onPrimaryAction = { [weak self] in
            self?.performUpdateAction()
        }
        content.updateBanner.onDismiss = { [weak self] in
            self?.content.updateBanner.isHidden = true
        }
        installKeyEquivalents()
        applyTheme(currentTheme(), persist: false)
        refreshWorkspaceSidebar(force: true)
        startPolling()
        DispatchQueue.main.async { [weak self] in
            self?.pollOnce()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    deinit {
        pollTimer?.invalidate()
        trafficMonitorTimer?.invalidate()
        deferredBridgeWorkScene = nil
        surfaceCatchUpWorkItem?.cancel()
        surfaceCatchUpWorkItem = nil
        surfaceCatchUpScenes.removeAll()
        if let languageObserver {
            NotificationCenter.default.removeObserver(languageObserver)
        }
        if !isClosing {
            sceneStack.shutdownAll()
            bridge.shutdown()
        }
        statusRefreshTimer?.invalidate()
        removeKeyMonitor()
    }

    // MARK: - 公开动作（菜单 / 快捷键）

    @objc func openShellsFromMenu(_ sender: Any?) {
        activateShells(selectFirstLocal: true)
    }

    @objc func newTab() {
        switch workspacePresentation {
        case .agents:
            return
        case .shells:
            createLocalShellTab()
            return
        case .connecting:
            return
        case .workspace:
            break
        }
        guard enqueueCoreTask(
            MuxTask.newTab(),
            failureMessage: MuxtermI18n.shared.tr(.errorNewTab)
        ) else {
            reportStatusError(MuxtermI18n.shared.tr(.errorNewTab))
            return
        }
        // 当拍 snapshot 还是旧 tab。等 TabAdded 再挂新树，避免先拆再等 tmux。
    }

    @objc func renameActiveTab() {
        guard case .workspace = workspacePresentation else { return }
        guard let tab = lastSnapshot.tabs.first(where: \.isActive)
            ?? lastSnapshot.tabs.first else { return }
        promptRenameTab(tab.id)
    }

    @objc func renameCurrentWorkspace() {
        let current = sceneStack.currentTargetConfig?.name
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
        return enqueueCoreTask(
            MuxTask.renameTab(tabId, name: name),
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        )
    }

    @discardableResult
    func renameWorkspace(to name: String) -> Bool {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return false }
        guard enqueueCoreTask(
            MuxTask.renameWorkspace(name),
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        ) else { return false }
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
        sceneStack.renameActiveTarget(
            to: name,
            rekeySession: terminalManager.usesClientResize
        )
        window?.title = "\(name) — Muxterm"
    }

    @discardableResult
    func moveTab(from: UInt32, target: UInt32, before: Bool) -> Bool {
        guard case .workspace = workspacePresentation else { return false }
        guard terminalManager.usesClientResize, from != target else { return false }
        guard enqueueCoreTask(
            MuxTask.moveTab(from: from, target: target, before: before),
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        ) else { return false }
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
    func swapPanes(_ a: UInt32, _ b: UInt32) -> Bool {
        guard a != b,
              lastSnapshot.panes.contains(where: { $0.id == a }),
              lastSnapshot.panes.contains(where: { $0.id == b })
        else { return false }
        guard enqueueCoreTask(
            MuxTask.swapPanes(a, b),
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        ) else { return false }
        needsLayoutReload = true
        scheduleStatusBarRefresh()
        return true
    }

    @discardableResult
    func movePaneToNewTab(_ paneId: UInt32) -> Bool {
        movePane(paneId, toTab: nil)
    }

    @discardableResult
    func movePane(_ paneId: UInt32, toTab tabId: UInt32?) -> Bool {
        guard case .workspace = workspacePresentation,
              terminalManager.supportsMultiTab,
              lastSnapshot.panes.count > 1,
              lastSnapshot.panes.contains(where: { $0.id == paneId })
        else { return false }
        let task: MuxTask
        if let tabId {
            guard lastSnapshot.tabs.contains(where: { $0.id == tabId }) else { return false }
            task = MuxTask.joinPane(paneId, tabId: tabId)
        } else {
            task = MuxTask.breakPane(paneId)
        }
        guard enqueueCoreTask(
            task,
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        ) else { return false }
        needsLayoutReload = true
        scheduleStatusBarRefresh()
        return true
    }

    private func paneMoveDestinations() -> [(tabId: UInt32?, title: String)] {
        let tabs = lastSnapshot.tabs
        let destinations = PaneMoveMenuPolicy.destinations(
            tabIds: tabs.map(\.id),
            currentTabId: lastSnapshot.activeTab,
            paneCountInCurrentTab: lastSnapshot.panes.count
        )
        return destinations.map { tabId in
            if let tabId {
                let title = tabs.first(where: { $0.id == tabId })?.name ?? "Tab"
                return (tabId, title)
            }
            return (nil, MuxtermI18n.shared.tr(.movePaneToNewTab))
        }
    }

    @objc func closeActivePane() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id ?? lastSnapshot.panes.first?.id else {
            return
        }
        closePane(pane)
    }

    private func closePane(_ pane: UInt32) {
        // 唯一 pane 时关 pane 会触发后端关 window；UI 侧随后收到 Exited 再关窗口。
        _ = enqueueCoreTask(
            MuxTask.closePane(pane),
            failureMessage: MuxtermI18n.shared.tr(
                .errorClosePane,
                arguments: ["id": "\(pane)"]
            )
        )
        needsLayoutReload = true
    }

    @objc func closeActiveWindow() {
        closeSessionWindow()
    }

    /// 每次只关闭最上面一层，不能穿过面板关闭背后的 pane。
    func closeFocusedSurface() {
        if unifiedPanel?.window?.isVisible == true {
            unifiedPanel.dismiss()
        } else if commandPalette?.window?.isVisible == true {
            commandPalette.dismiss()
        } else if !content.replyOverlayContainer.isHidden {
            toggleReplyOverlay()
        } else {
            closeActivePane()
        }
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
    /// left/right 文案和样式，不能再提供第二份窗口列表。Runtime 已按权威
    /// window index 排好 `tabs`，这里保持该顺序并使用稳定 tab id。
    private func tabEntriesForSwitching() -> [(index: Int, id: UInt32, name: String)] {
        presentedTabs().enumerated().map { position, tab in
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
        // Shells / Agents 是前端固定槽，不占真实 Workspace 的数字编号。
        let ordered = runtimeSidebarItems().filter {
            $0.runtime != TargetRuntime.shell.rawValue
        }
        guard !ordered.isEmpty else { return }
        let target: WorkspaceSidebarItem
        if oneBased == 0 {
            target = ordered[ordered.count - 1]
        } else {
            guard (1...9).contains(oneBased), ordered.indices.contains(oneBased - 1) else { return }
            target = ordered[oneBased - 1]
        }
        activateSidebarWorkspace(target.workspaceId)
    }

    @objc func splitHorizontal() {
        splitActivePane(horizontal: true)
    }

    @objc func splitVertical() {
        splitActivePane(horizontal: false)
    }

    @objc func splitAuto() {
        splitActivePaneAuto()
    }

    private func splitActivePaneAuto() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        else {
            return
        }
        let size = lastSnapshot.panes.first(where: { $0.id == pane })
        let cols = Int(size?.cols ?? 80)
        let rows = Int(size?.rows ?? 24)
        splitPane(pane, horizontal: AutoSplitPolicy.horizontal(cols: cols, rows: rows))
    }

    /// 当前 pane 全屏切换：tmux/ssh 发 `resize-pane -Z`，本地 shell 用布局全屏。
    @objc func toggleActivePaneFullscreen() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        else {
            return
        }
        togglePaneFullscreen(pane)
    }

    @objc func cycleActiveLayout() {
        let tab = lastSnapshot.activeTab
        guard tab != 0 else { return }
        _ = enqueueCoreTask(
            MuxTask.cycleLayout(tab),
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        )
        needsLayoutReload = true
    }

    private func togglePaneFullscreen(_ pane: UInt32) {
        if terminalManager.usesClientResize {
            _ = enqueueCoreTask(
                MuxTask.togglePaneFullscreen(pane),
                failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
            )
        } else {
            content.paneLayout.toggleFullscreen(paneId: pane)
            content.paneLayout.refreshSurfaceGeometry(paneId: pane)
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

    /// 当前主题：返回窗口已应用的本地状态。
    func currentTheme() -> MuxtermTheme {
        appliedTheme
    }

    /// 应用主题：更新终端默认色、重报 tmux 颜色，用户操作时再异步持久化。
    private func applyTheme(_ theme: MuxtermTheme, persist: Bool = true) {
        appliedTheme = theme
        if persist {
            let name = theme == .dark ? "black" : "white"
            persistConfig([["op": "replace", "path": "/theme/name", "value": name]])
        }
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
        _ = enqueueCoreColours(
            workspaceID: activeSceneWorkspaceID,
            .all(fgHex: osc.fg, bgHex: osc.bg)
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
        let controller = SettingsWindowController(
            bridge: bridge,
            quickConnectStore: quickConnectStore,
            configTransaction: { [weak self] request in
                self?.enqueueConfigTransaction(
                    request.operations,
                    completion: request.completion
                ) ?? false
            }
        )
        settingsWindow = controller
        controller.onApplied = { [weak self] operations in
            for operation in operations {
                guard operation["path"] as? String == "/ui/tab_bar_style",
                      let value = operation["value"] as? String,
                      let style = TabBarStyle(rawValue: value)
                else { continue }
                self?.content.statusBar.tabBarStyle = style
            }
        }
        controller.showWindow(self)
    }

    @objc func openQuickConnect() {
        guard let unifiedPanel else { return }
        // Recent 由 SceneStack 派生；当前 scene 用于行高亮。
        unifiedPanel.currentConfig = sceneStack.currentTargetConfig
        quickConnectStore.replaceAllRecents(sceneStack.allRecentTargetConfigs())
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

    private func installMainSplit(in window: NSWindow) {
        let sidebarController = NSViewController()
        sidebarController.view = workspaceSidebar
        let sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebarController)
        sidebarItem.canCollapse = true
        sidebarItem.minimumThickness = 160
        sidebarItem.maximumThickness = 420
        sidebarItem.preferredThicknessFraction = 0.25

        let contentController = NSViewController()
        contentController.view = content
        let contentItem = NSSplitViewItem(viewController: contentController)
        contentItem.minimumThickness = 300

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
        guard let sidebarSplitItem else { return }
        sidebarSplitItem.isCollapsed = !open
        content.statusBar.sidebarOpen = open
        if open {
            refreshWorkspaceSidebar(force: true)
        }
        // NSSplitViewController owns the divider geometry. Laying out only
        // NSWindow.contentView can leave the toggle changed while the sidebar
        // remains visually collapsed.
        mainSplitController.view.needsLayout = true
        mainSplitController.view.layoutSubtreeIfNeeded()
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

    private func attentionSnapshot(from candidate: CoreBridge) -> AttentionSnapshot? {
        guard let json = candidate.attentionSnapshotJSON() else { return nil }
        return AttentionSnapshot.decode(Data(json.utf8))
    }

    private func fallbackReplicaID(for target: TargetConfig) -> String {
        WorkspaceReplicaID.from(target)
    }

    private func workspaceReplicaID(for slot: WorkspaceScene) -> String {
        let computed = fallbackReplicaID(for: slot.targetConfig)
        if slot.targetConfig.path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
           let cached = slot.cachedWorkspaceReplicaID, !cached.isEmpty
        {
            return cached
        }
        return computed
    }

    /// 隐藏 scene 的 Attention 展示只读 ViewStore；侧栏刷新不触碰远端
    /// bridge，所有更新来自主线程 event pump。
    private func attentionSnapshot(for slot: WorkspaceScene) -> AttentionSnapshot? {
        return slot.cachedAttentionSnapshot
    }

    private var activeWorkspaceReplicaID: String? {
        guard let target = sceneStack.currentTargetConfig else { return nil }
        if let activeKey = sceneStack.activeKey,
           let slot = sceneStack.scenes[activeKey]
        {
            return workspaceReplicaID(for: slot)
        }
        return fallbackReplicaID(for: target)
    }

    /// UI 命令在同一主线程事件泵上顺序进入 Core；场景切换本身只执行本地
    /// Scene 操作，不需要等待任何后台 FFI。
    func performIfWindowOpen(_ action: @escaping () -> Void) {
        guard !isClosing else { return }
        action()
    }

    private var activeSceneWorkspaceID: String? {
        guard let activeKey = sceneStack.activeKey,
              let scene = sceneStack.scenes[activeKey]
        else {
            return nil
        }
        return scene.workspaceID
    }

    /// Enqueue a UI task without synchronously touching the Core handle.
    /// `pollOnce()` is the only consumer of this queue.
    @discardableResult
    private func enqueueCoreCommand(_ command: QueuedMuxCommand) -> Bool {
        guard !isClosing else { return false }
        commandQueue.enqueue(command)
        return true
    }

    /// Serialize a configuration draft and hand its transaction to the same
    /// event-pump boundary as terminal commands. The completion runs after
    /// Core has committed (or rejected) the draft.
    @discardableResult
    private func enqueueConfigTransaction(
        _ operations: [[String: Any]],
        completion: @escaping (Result<Void, Error>) -> Void = { _ in }
    ) -> Bool {
        guard !isClosing else { return false }
        guard let data = try? JSONSerialization.data(withJSONObject: operations),
              let operationsJSON = String(data: data, encoding: .utf8)
        else {
            completion(.failure(CoreBridgeDiscoveryError.message(
                "config patch encoding failed"
            )))
            return false
        }

        let requestID = nextConfigRequestID
        nextConfigRequestID += 1
        pendingConfigCompletions[requestID] = completion
        let accepted = enqueueCoreCommand(.config(
            operationsJSON: operationsJSON,
            requestID: requestID
        ))
        if !accepted {
            pendingConfigCompletions.removeValue(forKey: requestID)
        }
        return accepted
    }

    @discardableResult
    private func enqueueCoreTask(
        _ task: MuxTask,
        failureMessage: String
    ) -> Bool {
        enqueueCoreTask(
            workspaceID: activeSceneWorkspaceID,
            task,
            failureMessage: failureMessage
        )
    }

    /// Enqueue a task for an explicitly identified Workspace. Overlay actions
    /// can target a hidden scene, so they must not inherit the active scene at
    /// the time the event pump eventually drains the command.
    @discardableResult
    private func enqueueCoreTask(
        workspaceID: String?,
        _ task: MuxTask,
        failureMessage: String
    ) -> Bool {
        guard !isClosing else { return false }
        return enqueueCoreCommand(QueuedMuxCommand(
            workspaceID: workspaceID,
            task: QueuedMuxTask(
                type: task.type,
                targetPane: task.targetPane,
                targetTab: task.targetTab,
                dir: task.dir,
                name: task.name,
                coalescing: task.type == TASK_SWITCH_TAB ? .switchTab : .none
            ),
            failureMessage: failureMessage
        ))
    }

    @discardableResult
    private func enqueueCoreInput(
        workspaceID: String?,
        paneId: UInt32,
        data: Data,
        quiet: Bool = false,
        errorKey: MuxtermTextKey = .errorSendInput
    ) -> Bool {
        guard !data.isEmpty else { return true }
        return enqueueCoreCommand(.input(
            workspaceID: workspaceID,
            paneID: paneId,
            data: data,
            quiet: quiet,
            failureMessage: MuxtermI18n.shared.tr(
                errorKey,
                arguments: ["id": "\(paneId)"]
            )
        ))
    }

    @discardableResult
    private func enqueueCoreAttention(
        workspaceID: String?,
        _ attention: QueuedMuxAttention,
        refreshPanel: Bool = true
    ) -> Bool {
        let queued = enqueueCoreCommand(.attention(
            workspaceID: workspaceID,
            attention,
            failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
        ))
        if queued, refreshPanel {
            attentionPanelRefreshPending = true
        }
        return queued
    }

    @discardableResult
    private func enqueueCoreColours(
        workspaceID: String?,
        _ colours: QueuedMuxColours
    ) -> Bool {
        enqueueCoreCommand(.colours(
            workspaceID: workspaceID,
            colours
        ))
    }

    private func finishConfigRequest(
        _ requestID: UInt64?,
        result: Result<Void, Error>
    ) {
        guard let requestID,
              let completion = pendingConfigCompletions.removeValue(forKey: requestID)
        else {
            return
        }
        completion(result)
    }

    /// Enqueue a Core index search and return immediately to AppKit. The
    /// result is filtered against the current owned scene topology when the
    /// event-pump command completes.
    private func requestSearchHitsForPanel(
        query: String,
        scope: SearchScope,
        completion: @escaping ([SearchHit]) -> Void
    ) {
        let requestID = nextSearchRequestID
        nextSearchRequestID += 1
        pendingSearchCompletions[requestID] = { [weak self] hits in
            guard let self else { return }
            self.cacheActiveWorkspaceIdentity(from: hits)
            var uniqueHits: [SearchHit] = []
            var seen = Set<String>()
            for hit in hits {
                let key = "\(hit.workspaceId)\u{1F}\(hit.tabId)\u{1F}\(hit.paneId)\u{1F}\(hit.seq)"
                if seen.insert(key).inserted {
                    uniqueHits.append(hit)
                }
            }
            completion(self.filterSearchHitsForPanel(uniqueHits, scope: scope))
        }
        let accepted = enqueueCoreCommand(.search(
            query: query,
            requestID: requestID
        ))
        if !accepted {
            pendingSearchCompletions.removeValue(forKey: requestID)
            completion([])
        }
    }

    private func cacheActiveWorkspaceIdentity(from hits: [SearchHit]) {
        guard let activeKey = sceneStack.activeKey,
              let activeScene = sceneStack.scenes[activeKey]
        else {
            return
        }
        let expectedWorkspaceID = workspaceReplicaID(for: activeScene)
        guard hits.contains(where: { $0.workspaceId == expectedWorkspaceID }) else { return }
        activeScene.cacheWorkspaceReplicaID(expectedWorkspaceID)
    }

    private func finishSearchRequest(_ requestID: UInt64, hits: [SearchHit]) {
        pendingSearchCompletions.removeValue(forKey: requestID)?(hits)
    }

    /// Enqueue a pane-output snapshot without touching the Core handle from a
    /// panel or overlay action.
    @discardableResult
    private func enqueuePaneOutput(
        workspaceID: String?,
        paneID: UInt32,
        completion: @escaping (Data) -> Void
    ) -> Bool {
        guard !isClosing else { return false }
        let requestID = nextPaneOutputRequestID
        nextPaneOutputRequestID += 1
        pendingPaneOutputCompletions[requestID] = completion
        let accepted = enqueueCoreCommand(.paneOutput(
            workspaceID: workspaceID,
            paneID: paneID,
            requestID: requestID
        ))
        if !accepted {
            pendingPaneOutputCompletions.removeValue(forKey: requestID)
            completion(Data())
        }
        return accepted
    }

    private func finishPaneOutputRequest(_ requestID: UInt64, data: Data) {
        pendingPaneOutputCompletions.removeValue(forKey: requestID)?(data)
    }

    private func filterSearchHitsForPanel(
        _ hits: [SearchHit],
        scope: SearchScope
    ) -> [SearchHit] {
        let activePaneIDs: Set<UInt32>
        if let activeKey = sceneStack.activeKey,
           let activeScene = sceneStack.scenes[activeKey]
        {
            activePaneIDs = paneIDsForScene(activeScene)
        } else {
            activePaneIDs = Set(lastSnapshot.panes.map(\.id))
        }
        return scope.filter(
            hits,
            activePane: activePaneID,
            workspaceId: activeWorkspaceReplicaID,
            workspacePaneIDs: activePaneIDs
        )
    }

    private func paneIDsForScene(_ scene: WorkspaceScene) -> Set<UInt32> {
        var paneIDs = Set(scene.lastSnapshot.panes.map(\.id))
        if let cachedPaneIDs = scene.cachedTabIdsByPane?.keys {
            paneIDs.formUnion(cachedPaneIDs)
        }
        return paneIDs
    }

    private func scene(_ scene: WorkspaceScene, matchesWorkspaceId workspaceId: String) -> Bool {
        workspaceReplicaID(for: scene) == workspaceId
            || scene.cachedWorkspaceReplicaID == workspaceId
            || fallbackReplicaID(for: scene.targetConfig) == workspaceId
            || WorkspaceReplicaID.from(
                session: scene.targetConfig.session,
                path: "",
                transport: scene.targetConfig.transport.label
            ) == workspaceId
            || QuickConnect.uniqueID(for: scene.targetConfig) == workspaceId
    }

    private func scene(forWorkspaceId workspaceId: String) -> WorkspaceScene? {
        let matches = sceneStack.scenes.values.filter {
            $0.visibility != .closed && scene($0, matchesWorkspaceId: workspaceId)
        }
        return matches.first(where: { $0.visibility == .visible }) ?? matches.first
    }

    private func cachedTabID(containingPane paneID: UInt32) -> UInt32? {
        if let activeKey = sceneStack.activeKey,
           let scene = sceneStack.scenes[activeKey],
           let tabID = scene.cachedTabIdsByPane?[paneID]
        {
            return tabID
        }
        guard lastSnapshot.panes.contains(where: { $0.id == paneID }) else {
            return nil
        }
        return lastSnapshot.activeTab
    }

    /// Dispatch queued commands at the same serialized boundary that drains
    /// workspace events.  Explicit workspace dispatch keeps a queued command
    /// attached to its originating scene after a subsequent scene switch.
    private func flushCoreCommandQueue() {
        let commands = commandQueue.drain()
        guard !commands.isEmpty else { return }
        var attentionMutationSucceeded = false
        for command in commands {
            let result: Int32
            switch command.operation {
            case .task(let queuedTask):
                let task = MuxTask(
                    type: queuedTask.type,
                    targetPane: queuedTask.targetPane,
                    targetTab: queuedTask.targetTab,
                    dir: queuedTask.dir,
                    name: queuedTask.name
                )
                if let workspaceID = command.workspaceID {
                    result = bridge.execute(task: task, workspaceID: workspaceID)
                } else {
                    result = bridge.execute(task: task)
                }
            case .imagePaste(let paneID, let png):
                do {
                    guard imagePasteBridge == nil else {
                        throw CoreBridgeDiscoveryError.message(MuxtermI18n.shared.tr(.imagePasteBusy))
                    }
                    guard let workspaceID = command.workspaceID else {
                        throw CoreBridgeDiscoveryError.message(MuxtermI18n.shared.tr(.errorCoreUnavailable))
                    }
                    try bridge.startImagePaste(workspaceID: workspaceID, paneID: paneID, png: png)
                    imagePasteBridge = bridge
                    content.statusBar.setImagePasteInProgress(true)
                } catch {
                    reportStatusError(MuxtermI18n.shared.tr(.imagePasteFailed) + ": " + error.localizedDescription)
                }
                result = 0
            case .input(let paneID, let data, let quiet):
                if let workspaceID = command.workspaceID {
                    if quiet {
                        result = bridge.sendInputQuiet(
                            workspaceID: workspaceID,
                            paneId: paneID,
                            data: data
                        )
                    } else {
                        result = bridge.sendInput(
                            workspaceID: workspaceID,
                            paneId: paneID,
                            data: data
                        )
                    }
                } else if quiet {
                    result = bridge.sendInputQuiet(paneId: paneID, data: data)
                } else {
                    result = bridge.sendInput(paneId: paneID, data: data)
                }
            case .resize(let resize):
                switch resize {
                case .pane(let paneID, let cols, let rows):
                    if let workspaceID = command.workspaceID {
                        result = bridge.resizePane(
                            workspaceID: workspaceID,
                            paneId: paneID,
                            cols: cols,
                            rows: rows
                        )
                    } else {
                        result = bridge.resizePane(paneId: paneID, cols: cols, rows: rows)
                    }
                case .paneAxis(let paneID, let horizontal, let size):
                    if let workspaceID = command.workspaceID {
                        result = bridge.resizePaneAxis(
                            workspaceID: workspaceID,
                            paneId: paneID,
                            horizontal: horizontal,
                            size: size
                        )
                    } else {
                        result = bridge.resizePaneAxis(
                            paneId: paneID,
                            horizontal: horizontal,
                            size: size
                        )
                    }
                case .client(let cols, let rows):
                    if let workspaceID = command.workspaceID {
                        result = bridge.resizeClient(
                            workspaceID: workspaceID,
                            cols: cols,
                            rows: rows
                        )
                    } else {
                        result = bridge.resizeClient(cols: cols, rows: rows)
                    }
                }
            case .viewport(let paneID, let offset):
                if let workspaceID = command.workspaceID {
                    result = bridge.setPaneViewport(
                        workspaceID: workspaceID,
                        paneId: paneID,
                        offset: offset
                    )
                } else {
                    result = bridge.setPaneViewport(paneId: paneID, offset: offset)
                }
            case .closeWorkspace:
                if let workspaceID = command.workspaceID {
                    result = bridge.closeWorkspace(workspaceID: workspaceID)
                } else {
                    result = -1
                }
            case .colours(let colours):
                switch colours {
                case .pane(let paneID, let fgHex, let bgHex):
                    if let workspaceID = command.workspaceID {
                        result = bridge.reportPaneColours(
                            workspaceID: workspaceID,
                            paneId: paneID,
                            fgHex: fgHex,
                            bgHex: bgHex
                        )
                    } else {
                        result = bridge.reportPaneColours(
                            paneId: paneID,
                            fgHex: fgHex,
                            bgHex: bgHex
                        )
                    }
                case .all(let fgHex, let bgHex):
                    if let workspaceID = command.workspaceID {
                        result = bridge.reportAllPaneColours(
                            workspaceID: workspaceID,
                            fgHex: fgHex,
                            bgHex: bgHex
                        )
                    } else {
                        result = bridge.reportAllPaneColours(fgHex: fgHex, bgHex: bgHex)
                    }
                }
            case .attention(let attention):
                switch attention {
                case .becameVisible(let paneID):
                    if let workspaceID = command.workspaceID {
                        result = bridge.attentionOnBecameVisible(
                            workspaceID: workspaceID,
                            paneId: paneID
                        )
                    } else {
                        result = bridge.attentionOnBecameVisible(paneId: paneID)
                    }
                case .acknowledge(let paneID):
                    if let workspaceID = command.workspaceID {
                        result = bridge.attentionAcknowledge(
                            workspaceID: workspaceID,
                            paneId: paneID
                        )
                    } else {
                        result = bridge.attentionAcknowledge(paneId: paneID)
                    }
                case .mute(let paneID, let seconds):
                    if let workspaceID = command.workspaceID {
                        result = bridge.attentionMute(
                            workspaceID: workspaceID,
                            paneId: paneID,
                            seconds: seconds
                        )
                    } else {
                        result = bridge.attentionMute(
                            paneId: paneID,
                            seconds: seconds
                        )
                    }
                }
            case .config(let config):
                do {
                    guard let data = config.operationsJSON.data(using: .utf8),
                          let operations = try JSONSerialization.jsonObject(with: data)
                              as? [[String: Any]]
                    else {
                        throw CoreBridgeDiscoveryError.message(
                            "config patch encoding failed"
                        )
                    }
                    let transaction = try bridge.configBegin()
                    do {
                        try bridge.configPatch(
                            transaction: transaction,
                            operations: operations
                        )
                        try bridge.configCommit(transaction: transaction)
                    } catch {
                        bridge.configCancel(transaction: transaction)
                        throw error
                    }
                    finishConfigRequest(config.requestID, result: .success(()))
                    result = 0
                } catch {
                    finishConfigRequest(config.requestID, result: .failure(error))
                    result = -1
                }
            case .search(let search):
                var hits: [SearchHit] = []
                if let json = bridge.searchAllJSON(query: search.query),
                   let data = json.data(using: .utf8),
                   let snapshot = SearchSnapshot.decode(data)
                {
                    hits = snapshot.hits
                }
                finishSearchRequest(search.requestID, hits: hits)
                result = 0
            case .paneOutput(let request):
                let data: Data
                if let workspaceID = command.workspaceID {
                    data = bridge.getPaneOutput(
                        workspaceID: workspaceID,
                        paneId: request.paneID
                    )
                } else {
                    data = bridge.getPaneOutput(paneId: request.paneID)
                }
                finishPaneOutputRequest(request.requestID, data: data)
                result = 0
            }
            if result != 0 {
                switch command.operation {
                case .colours(.pane(let paneID, _, _)):
                    reportedColourPanes.remove(ColourPaneKey(
                        workspaceID: command.workspaceID,
                        paneID: paneID
                    ))
                case .colours(.all):
                    reportedColourPanes.removeAll()
                default:
                    break
                }
            }
            if result != 0, !command.failureMessage.isEmpty {
                reportStatusError(command.failureMessage)
            }
            if result == 0 {
                switch command.operation {
                case .attention(.acknowledge), .attention(.mute):
                    attentionMutationSucceeded = true
                default:
                    break
                }
            }
        }
        if attentionMutationSucceeded {
            if let snapshot = attentionSnapshot(from: bridge) {
                lastPoolAttentionSnapshot = snapshot
                updateSceneAttentionStores(snapshot: snapshot)
            }
            refreshAttentionChrome(allowBridgeQueries: false, force: true)
        }
        if attentionPanelRefreshPending {
            attentionPanelRefreshPending = false
            unifiedPanel.refreshData()
        }
    }

    private func pollImagePaste() {
        guard let source = imagePasteBridge else { return }
        do {
            guard try source.pollImagePaste() != nil else { return }
        } catch {
            reportStatusError(MuxtermI18n.shared.tr(.imagePasteFailed) + ": " + error.localizedDescription)
        }
        imagePasteBridge = nil
        content.statusBar.setImagePasteInProgress(false)
    }

    private func reorderWorkspaces(_ workspaceIds: [String]) {
        var next: UInt64 = 1
        var seen = Set<String>()
        for workspaceId in workspaceIds {
            guard seen.insert(workspaceId).inserted else { continue }
            guard let slot = sceneStack.scenes.values.first(where: {
                $0.visibility != .closed && workspaceReplicaID(for: $0) == workspaceId
            }) else { continue }
            slot.openedOrder = next
            next += 1
        }
        let remaining = workspaceSidebarScenes().filter { slot in
            !seen.contains(workspaceReplicaID(for: slot))
        }
        for slot in remaining {
            slot.openedOrder = next
            next += 1
        }
        nextWorkspaceOpenedOrder = next
        refreshWorkspaceSidebar(force: true)
    }

    func workspaceSidebarScenes() -> [WorkspaceScene] {
        sceneStack.scenes.values
            .filter { $0.visibility != .closed }
            .sorted { lhs, rhs in
                if lhs.openedOrder != rhs.openedOrder {
                    return lhs.openedOrder < rhs.openedOrder
                }
                return lhs.key.session < rhs.key.session
        }
    }

    private func tabTargetsByPane(
        for slot: WorkspaceScene
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

    private func workspaceShortcutIndex(for config: TargetConfig) -> Int? {
        if config.runtime == .shell {
            return nil
        }
        let targetID = QuickConnect.uniqueID(for: config)
        let orderedTargetIDs = workspaceSidebarScenes().filter {
            $0.targetConfig.runtime != .shell
        }.map {
            QuickConnect.uniqueID(for: $0.targetConfig)
        }
        return WorkspaceShortcutIndex.byWorkspaceID(orderedTargetIDs)[targetID]
    }

    /// 所有真实 Workspace 的只读侧栏输入。聚合模型从这里取源事实；这里
    /// 不会制造 Shells/Agents 假 Workspace。
    func runtimeSidebarItems() -> [WorkspaceSidebarItem] {
        let slots = workspaceSidebarScenes()
        let workspaceIDs = slots.map { workspaceReplicaID(for: $0) }
        return slots.enumerated().compactMap { index, slot in
            let target = slot.targetConfig
            let workspaceID = workspaceIDs[index]
            let structuredAgents = structuredAgentTestOverrides[workspaceID]
                ?? slot.cachedStructuredAgents
            let tabTargets = tabTargetsByPane(for: slot)
            return WorkspaceSidebarItem(
                workspaceId: workspaceID,
                name: target.name,
                runtime: target.runtime.rawValue,
                transport: target.transport.label,
                isActive: slot.visibility == .visible,
                structuredAgents: structuredAgents,
                tabNumberByPane: tabTargets.tabNumbersByPane,
                tabIdByPane: tabTargets.tabIdsByPane
            )
        }
    }

    private func sidebarItems() -> [WorkspaceSidebarItem] {
        let runtimeItems = runtimeSidebarItems()
        let regularItems = runtimeItems.filter { $0.runtime != TargetRuntime.shell.rawValue }
        let orderedIDs = regularItems.map(\.workspaceId)
        let shortcuts = WorkspaceShortcutIndex.byWorkspaceID(orderedIDs)
        let shellsActive: Bool
        let agentsActive: Bool
        switch workspacePresentation {
        case .shells:
            shellsActive = true
            agentsActive = false
        case .agents:
            shellsActive = false
            agentsActive = true
        case .workspace:
            shellsActive = false
            agentsActive = false
        case .connecting:
            shellsActive = false
            agentsActive = false
        }
        var result = [
            WorkspaceSidebarItem(
                workspaceId: AggregateWorkspaceIdentity.shells,
                name: "Shells",
                runtime: "aggregate",
                transport: "local + ssh",
                isActive: shellsActive,
                shortcut: nil,
                isClosable: false,
                isReorderable: false
            ),
            WorkspaceSidebarItem(
                workspaceId: AggregateWorkspaceIdentity.agents,
                name: "Agents",
                runtime: "aggregate",
                transport: "all workspaces",
                isActive: agentsActive,
                shortcut: nil,
                isClosable: false,
                isReorderable: false
            ),
        ]
        if let pending = pendingWorkspaceOpen {
            result.append(WorkspaceSidebarItem(
                workspaceId: pending.id,
                name: pending.config.name,
                runtime: pending.config.runtime.rawValue,
                transport: pending.config.transport.label,
                isActive: workspacePresentation == .connecting(pending.id),
                shortcut: nil,
                isClosable: false,
                isReorderable: false,
                openingStage: pending.stage.rawValue,
                openingTargetID: QuickConnect.uniqueID(for: pending.config)
            ))
        }
        result.append(contentsOf: regularItems.map { item in
            WorkspaceSidebarItem(
                workspaceId: item.workspaceId,
                name: item.name,
                runtime: item.runtime,
                transport: item.transport,
                isActive: workspacePresentation == .workspace && item.isActive,
                shortcut: shortcuts[item.workspaceId],
                structuredAgents: item.structuredAgents,
                tabNumberByPane: item.tabNumberByPane,
                tabIdByPane: item.tabIdByPane
            )
        })
        return result
    }

    private var presentedWorkspaceSelectionID: String? {
        switch workspacePresentation {
        case .workspace:
            return activeWorkspaceReplicaID
        case .shells:
            return AggregateWorkspaceIdentity.shells
        case .agents:
            return AggregateWorkspaceIdentity.agents
        case .connecting(let id):
            return id
        }
    }

    private func shellAggregateTabs() -> [ShellAggregateTab] {
        let sources = workspaceSidebarScenes().compactMap { slot -> ShellAggregateSource? in
            guard slot.targetConfig.runtime == .shell else { return nil }
            return ShellAggregateSource(
                workspaceId: workspaceReplicaID(for: slot),
                transport: slot.targetConfig.transport.label,
                openedOrder: slot.openedOrder,
                tabs: slot.lastSnapshot.tabs.map {
                    AggregateSourceTab(id: $0.id, title: $0.name)
                }
            )
        }
        return AggregateWorkspaceProjection.shellTabs(sources: sources)
    }

    private func projectedAgentItems() -> [AgentSidebarItem] {
        WorkspaceSidebarProjection.agents(
            workspaces: runtimeSidebarItems(),
            attention: attentionSnapshotForPanel()
        )
    }

    private func presentedTabActivities() -> [UInt32: AgentSidebarIndicator] {
        let workspaces = runtimeSidebarItems()
        let source = WorkspaceSidebarProjection.tabActivities(
            workspaces: workspaces,
            attention: attentionSnapshotForPanel(),
            hidden: hiddenAttentionKeys
        )
        func indicator(workspaceId: String, tabId: UInt32) -> AgentSidebarIndicator? {
            source[WorkspaceTabActivityKey(workspaceId: workspaceId, tabId: tabId)]
        }

        switch workspacePresentation {
        case .workspace:
            guard let workspaceId = activeWorkspaceReplicaID else { return [:] }
            return Dictionary(uniqueKeysWithValues: lastSnapshot.tabs.compactMap { tab in
                indicator(workspaceId: workspaceId, tabId: tab.id).map { (tab.id, $0) }
            })
        case .shells:
            return Dictionary(uniqueKeysWithValues: shellAggregateTabs().compactMap { tab in
                indicator(workspaceId: tab.workspaceId, tabId: tab.sourceTabId).map {
                    (tab.displayId, $0)
                }
            })
        case .agents:
            return Dictionary(uniqueKeysWithValues: agentAggregateTabs().compactMap { tab in
                indicator(workspaceId: tab.workspaceId, tabId: tab.sourceTabId).map {
                    (tab.displayId, $0)
                }
            })
        case .connecting:
            return [:]
        }
    }

    private func agentAggregateTabs(
        agents: [AgentSidebarItem]? = nil
    ) -> [AgentAggregateTab] {
        let source = agents ?? projectedAgentItems()
        let liveKeys = Set(source.compactMap { agent -> AgentAggregateKey? in
            guard let sourceTabId = agent.tabId else { return nil }
            return AgentAggregateKey(
                workspaceId: agent.workspaceId,
                sourceTabId: sourceTabId
            )
        })
        hiddenAgentTabs.formIntersection(liveKeys)
        return AggregateWorkspaceProjection.agentTabs(
            agents: source.filter { agent in
                guard let sourceTabId = agent.tabId else { return false }
                return !hiddenAgentTabs.contains(AgentAggregateKey(
                    workspaceId: agent.workspaceId,
                    sourceTabId: sourceTabId
                ))
            },
            workspaceOrder: runtimeSidebarItems().map(\.workspaceId)
        )
    }

    func presentedTabs() -> [Tab] {
        switch workspacePresentation {
        case .workspace:
            return lastSnapshot.tabs
        case .shells(let selectedWorkspaceId, let selectedTabId):
            let tabs = shellAggregateTabs()
            let selected = tabs.first {
                $0.workspaceId == selectedWorkspaceId && $0.sourceTabId == selectedTabId
            } ?? tabs.first
            return tabs.map {
                Tab(
                    id: $0.displayId,
                    name: $0.title,
                    isActive: $0.workspaceId == selected?.workspaceId
                        && $0.sourceTabId == selected?.sourceTabId
                )
            }
        case .agents(let selectedKey):
            let tabs = agentAggregateTabs()
            let selected = tabs.first { $0.key == selectedKey } ?? tabs.first
            return tabs.map {
                Tab(id: $0.displayId, name: $0.title, isActive: $0.key == selected?.key)
            }
        case .connecting:
            guard let pending = pendingWorkspaceOpen else { return [] }
            return [Tab(id: UInt32.max, name: pending.config.name, isActive: true)]
        }
    }

    private func updatePresentedTabs(fallback: [Tab]? = nil) {
        let tabs: [Tab]
        if case .workspace = workspacePresentation {
            tabs = fallback ?? lastSnapshot.tabs
        } else {
            tabs = presentedTabs()
        }
        content.updateTabs(tabs)
        content.statusBar.setTabActivities(presentedTabActivities())
        switch workspacePresentation {
        case .workspace:
            content.statusBar.setWorkspacePresentation(.workspace)
            content.statusBar.allowsTabCreation = true
            content.statusBar.allowsTabRenaming = true
            content.statusBar.allowsTabClosing = true
            content.statusBar.allowsTabReordering = terminalManager.usesClientResize
            content.paneLayout.allowsPaneBreak = terminalManager.supportsMultiTab
        case .shells:
            content.statusBar.setWorkspacePresentation(.shells)
            content.statusBar.allowsTabCreation = true
            content.statusBar.allowsTabRenaming = false
            content.statusBar.allowsTabClosing = true
            content.statusBar.allowsTabReordering = false
            content.paneLayout.allowsPaneBreak = false
        case .agents:
            content.statusBar.setWorkspacePresentation(.agents)
            content.statusBar.allowsTabCreation = false
            content.statusBar.allowsTabRenaming = false
            content.statusBar.allowsTabClosing = true
            content.statusBar.allowsTabReordering = false
            content.paneLayout.allowsPaneBreak = false
        case .connecting:
            content.statusBar.setWorkspacePresentation(.opening)
            content.statusBar.allowsTabCreation = false
            content.statusBar.allowsTabRenaming = false
            content.statusBar.allowsTabClosing = false
            content.statusBar.allowsTabReordering = false
            content.paneLayout.allowsPaneBreak = false
        }
    }

    private func reconcileAggregatePresentation(agents: [AgentSidebarItem]) {
        switch workspacePresentation {
        case .workspace:
            return
        case .connecting:
            return
        case .shells(let workspaceId, let tabId):
            let tabs = shellAggregateTabs()
            guard let target = tabs.first(where: {
                $0.workspaceId == workspaceId && $0.sourceTabId == tabId
            }) ?? tabs.first else { return }
            let changed = target.workspaceId != workspaceId || target.sourceTabId != tabId
            workspacePresentation = .shells(
                workspaceId: target.workspaceId,
                tabId: target.sourceTabId
            )
            updatePresentedTabs()
            if changed {
                DispatchQueue.main.async { [weak self] in
                    self?.activateShells(
                        selectFirstLocal: false,
                        preferredWorkspaceId: target.workspaceId,
                        preferredTabId: target.sourceTabId
                    )
                }
            }
        case .agents(let selectedKey):
            let tabs = agentAggregateTabs(agents: agents)
            guard let target = tabs.first(where: { $0.key == selectedKey })
                ?? tabs.first(where: { $0.key == lastAgentAggregateKey })
                ?? tabs.first else {
                workspacePresentation = .agents(nil)
                lastAgentAggregateKey = nil
                presentEmptyAgents()
                return
            }
            lastAgentAggregateKey = target.key
            if target.key != selectedKey {
                workspacePresentation = .agents(target.key)
                DispatchQueue.main.async { [weak self] in
                    self?.activateAgents(preferred: target.key)
                }
            }
            updatePresentedTabs()
        }
    }

    func refreshWorkspaceSidebar(force: Bool = false) {
        let attention = attentionSnapshotForPanel()
        let agents = WorkspaceSidebarProjection.agents(
            workspaces: runtimeSidebarItems(),
            attention: attention
        ).filter { !hiddenAttentionKeys.contains(AttentionVisibilityKey(
            workspaceId: $0.workspaceId,
            paneId: $0.paneId
        )) }
        reconcileAggregatePresentation(agents: agents)
        guard force || isWorkspaceSidebarOpen else { return }
        let workspaces = sidebarItems()
        workspaceSidebar.setWorkspaces(workspaces)
        workspaceSidebar.setAgents(agents)
        workspaceSidebar.setCommands(WorkspaceSidebarProjection.commands(
            workspaces: runtimeSidebarItems(),
            attention: attention
        ).filter { !hiddenAttentionKeys.contains(AttentionVisibilityKey(
            workspaceId: $0.workspaceId,
            paneId: $0.paneId
        )) })
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: lastSnapshot.activeTab,
            paneId: activePaneID,
            workspaceSelectionId: presentedWorkspaceSelectionID
        )
    }

    private func attentionSnapshotForPanel(refreshActive: Bool = false) -> AttentionSnapshot? {
        if refreshActive, let snapshot = attentionSnapshot(from: bridge) {
            lastPoolAttentionSnapshot = snapshot
            return snapshot
        }
        if let snapshot = lastPoolAttentionSnapshot, !snapshot.workspaces.isEmpty {
            return snapshot
        }
        var workspaces: [WorkspaceAttention] = []
        var seen = Set<String>()
        func append(_ snapshot: AttentionSnapshot) {
            for workspace in snapshot.workspaces where seen.insert(workspace.workspaceId).inserted {
                workspaces.append(workspace)
            }
        }
        if let snapshot = sceneStack.activeKey
            .flatMap({ sceneStack.scenes[$0]?.cachedAttentionSnapshot })
        {
            append(snapshot)
        }
        for slot in sceneStack.scenes.values where slot.visibility != .closed {
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

    /// 测试用：按 Workspace 读取 Core 的 attention 快照。
    func testAttentionBlockedCount(workspaceId: String) -> Int {
        var blockedCount = -1
        guard let slot = sceneStack.scenes.values.first(where: {
            scene($0, matchesWorkspaceId: workspaceId)
        }), let snapshot = slot.cachedAttentionSnapshot else {
            return blockedCount
        }
        blockedCount = snapshot.blockedCount
        return blockedCount
    }

    /// E2E 用：同步排空 shared Workspace 的主线程 event pump，避免测试
    /// 依赖后台 QoS 队列的调度时序。
    func testPollBackgroundWorkspaces() {
        guard !isClosing else { return }
        for _ in 0..<32 {
            pollOnce()
            var hasPending = false
            for slot in sceneStack.scenes.values where slot.visibility == .hidden {
                while slot.hasPendingSurfaceWork {
                    hasPending = true
                    _ = slot.applyPendingSurfaceEvents(
                        maxEvents: Int.max,
                        timeBudget: .infinity
                    )
                }
            }
            if !hasPending { break }
        }
    }

    /// E2E 仍可传入一个独立 bridge 来准备远端 session，但真正的
    /// Workspace 会重新通过当前 Core handle 打开，测试路径与生产事件泵一致。
    func testActivateWorkspaceBridge(_ nextBridge: CoreBridge, session: String) {
        let transport: TargetTransport
        if let alias = nextBridge.sshAlias {
            transport = .ssh(name: alias)
        } else {
            transport = .local
        }
        let target = TargetConfig(
            name: session,
            runtime: .tmux,
            transport: transport,
            path: "",
            session: session,
            socket: nextBridge.socket
        )
        do {
            let opened = try bridge.openWorkspace(
                target: target,
                intent: .attachOnly,
                initialClientSize: initialTmuxClientSizeHint()
            )
            nextBridge.shutdown()
            activate(slot: makeCatalogWorkspaceScene(opened: opened, sharedBridge: bridge))
        } catch {
            nextBridge.shutdown()
            showError(error)
        }
    }

    /// Close a hidden workspace from the sidebar. The visible scene falls forward
    /// to the next scene; closing the last one closes the session window.
    func closeWorkspace(_ workspaceId: String) {
        guard let slot = sceneStack.scenes.values.first(where: { candidate in
            candidate.visibility != .closed && workspaceReplicaID(for: candidate) == workspaceId
        }) else { return }

        let wasActive = slot.visibility == .visible
        let ordered = workspaceSidebarScenes()
        let index = ordered.firstIndex(where: { $0 === slot }) ?? 0
        let fallback = wasActive
            ? ordered.dropFirst(index + 1).first
                ?? ordered.prefix(index).last
            : nil

        // Keep the UI switch local, but close the corresponding Core-owned
        // workspace at the next serialized event-pump boundary.  The last
        // visible workspace is handled by the window shutdown path instead.
        if let sceneWorkspaceID = slot.workspaceID,
           !wasActive || fallback != nil
        {
            _ = enqueueCoreCommand(.closeWorkspace(
                workspaceID: sceneWorkspaceID,
                failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
            ))
        }
        if let sceneWorkspaceID = slot.workspaceID {
            commandMarksCache = commandMarksCache.filter {
                $0.key.workspaceID != sceneWorkspaceID
            }
        }
        sceneStack.close(key: slot.key)
        content.paneLayout.dropParked(except: Array(sceneStack.scenes.values.map(\.terminalManager)))
        quickConnectStore.replaceAllRecents(sceneStack.allRecentTargetConfigs())
        if wasActive {
            if let fallback {
                activate(slot: fallback)
            } else {
                closeSessionWindow()
            }
        }
        refreshWorkspaceSidebar(force: true)
        unifiedPanel.refreshData()
    }

    /// 容量是 soft limit：超过阈值时只提醒用户，绝不静默移除后台 Workspace。
    /// 列出的都是最久未使用的后台 slot，当前活动 Workspace 永远不会出现在
    /// 选择框中；用户选择后才调用同一条 sidebar close/evict 资源路径。
    private func presentWorkspaceCapacityWarningIfNeeded() {
        guard !isClosing else { return }
        if !sceneStack.isOverCapacity {
            capacityWarningPresentedForSceneCount = nil
            return
        }
        guard capacityWarningPresentedForSceneCount != sceneStack.sceneCount,
              let ownerWindow = window
        else { return }

        let sceneCount = sceneStack.sceneCount
        let overflow = max(1, sceneCount - sceneStack.maxScenes)
        let candidates = sceneStack.oldestHiddenCandidates(
            limit: min(8, overflow)
        )
        guard !candidates.isEmpty else { return }
        capacityWarningPresentedForSceneCount = sceneCount

        let alert = NSAlert()
        alert.messageText = MuxtermI18n.shared.tr(.workspaceCapacityTitle)
        alert.informativeText = MuxtermI18n.shared.tr(
            .workspaceCapacityMessage,
            arguments: [
                "count": "\(sceneCount)",
                "limit": "\(sceneStack.maxScenes)",
            ]
        )
        alert.alertStyle = .warning

        let choices = candidates.map { candidate -> (SceneCapacityCandidate, NSButton) in
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
                    _ = self.sceneStack.close(key: candidate.key)
                }
                self.content.paneLayout.dropParked(
                    except: Array(self.sceneStack.scenes.values.map(\.terminalManager))
                )
                self.quickConnectStore.replaceAllRecents(
                    self.sceneStack.allRecentTargetConfigs()
                )
                self.refreshWorkspaceSidebar(force: true)
                self.unifiedPanel.refreshData()
            }
            // 关闭了部分候选后仍然超限时，不在同一轮连续弹窗；下一次新增
            // slot（或手动关闭使数量变化）再重新提醒。
            self.capacityWarningPresentedForSceneCount = self.sceneStack.isOverCapacity
                ? self.sceneStack.sceneCount
                : nil
        }
    }

    @discardableResult
    private func activateWorkspaceIfAvailable(_ workspaceId: String) -> Bool {
        if let activeKey = sceneStack.activeKey,
           let activeScene = sceneStack.scenes[activeKey],
           scene(activeScene, matchesWorkspaceId: workspaceId)
        {
            if case .workspace = workspacePresentation {
                return true
            }
            // Agents / Shells 只是当前真实 scene 的前端投影；回到同一个
            // Workspace 时只退出投影，不重新激活或重挂底层布局。
            workspacePresentation = .workspace
            content.setConnectProgress(stage: nil)
            updatePresentedTabs()
            refreshWorkspaceSidebar(force: true)
            return true
        }
        guard let targetScene = scene(forWorkspaceId: workspaceId) else { return false }
        activate(slot: targetScene)
        return true
    }

    func activateSidebarWorkspace(_ workspaceId: String) {
        if pendingWorkspaceOpen?.id == workspaceId {
            presentPendingWorkspaceOpen()
            return
        }
        switch workspaceId {
        case AggregateWorkspaceIdentity.shells:
            activateShells(selectFirstLocal: false)
        case AggregateWorkspaceIdentity.agents:
            activateAgents()
        default:
            _ = activateWorkspaceIfAvailable(workspaceId)
        }
    }

    /// 进入 Shells 时只激活真实 shell Workspace scene；Tab 条由前端合并显示。
    func activateShells(
        selectFirstLocal: Bool,
        preferredWorkspaceId: String? = nil,
        preferredTabId: UInt32? = nil
    ) {
        let tabs = shellAggregateTabs()
        let current: ShellAggregateTab? = {
            if selectFirstLocal { return tabs.first(where: \.isLocal) ?? tabs.first }
            if let preferredWorkspaceId, let preferredTabId {
                return tabs.first {
                    $0.workspaceId == preferredWorkspaceId && $0.sourceTabId == preferredTabId
                }
            }
            if case .shells(let workspaceId, let tabId) = workspacePresentation {
                return tabs.first {
                    $0.workspaceId == workspaceId && $0.sourceTabId == tabId
                }
            }
            return tabs.first(where: \.isLocal) ?? tabs.first
        }()
        guard let target = current, let slot = scene(forWorkspaceId: target.workspaceId) else {
            openLocalShellWorkspace()
            return
        }
        workspacePresentation = .shells(
            workspaceId: target.workspaceId,
            tabId: target.sourceTabId
        )
        activateBackingSlot(slot, force: true)
        if slot.lastSnapshot.activeTab != target.sourceTabId {
            requestSourceTab(target.sourceTabId)
        }
        updatePresentedTabs()
        refreshWorkspaceSidebar(force: true)
    }

    private func activateShellTab(displayId: UInt32) {
        guard let target = shellAggregateTabs().first(where: { $0.displayId == displayId }) else {
            return
        }
        activateShells(
            selectFirstLocal: false,
            preferredWorkspaceId: target.workspaceId,
            preferredTabId: target.sourceTabId
        )
    }

    private func openLocalShellWorkspace() {
        let directory = FileManager.default.homeDirectoryForCurrentUser.path
        let target = TargetConfig(
            name: "local",
            runtime: .shell,
            transport: .local,
            path: directory
        )
        guard beginPendingWorkspaceOpen(config: target, stage: .resolving) else { return }
        connectCatalogTarget(config: target, intent: .createIfMissing) { [weak self] result in
            self?.finishCatalogConnect(result)
        }
    }

    /// Shells 中的 “+” 和 Cmd-T 总是在本机 shell Workspace 新建真实 tab。
    private func createLocalShellTab() {
        guard let local = shellAggregateTabs().first(where: \.isLocal),
              let slot = scene(forWorkspaceId: local.workspaceId)
        else {
            openLocalShellWorkspace()
            return
        }
        workspacePresentation = .shells(
            workspaceId: local.workspaceId,
            tabId: local.sourceTabId
        )
        activateBackingSlot(slot, force: true)
        _ = enqueueCoreTask(
            MuxTask.newTab(),
            failureMessage: MuxtermI18n.shared.tr(.errorNewTab)
        )
    }

    func activateAgents(preferred: AgentAggregateKey? = nil) {
        if let preferred {
            // 侧栏中的 agent 行是显式重新进入；允许把先前关掉的投影页找回。
            hiddenAgentTabs.remove(preferred)
        }
        let tabs = agentAggregateTabs()
        let target = preferred.flatMap { key in tabs.first(where: { $0.key == key }) }
            ?? {
                guard case .agents(let selectedKey) = workspacePresentation else { return nil }
                return selectedKey.flatMap { key in tabs.first(where: { $0.key == key }) }
            }()
            ?? lastAgentAggregateKey.flatMap { key in tabs.first(where: { $0.key == key }) }
            ?? tabs.first
        guard let target, let slot = scene(forWorkspaceId: target.workspaceId) else {
            workspacePresentation = .agents(nil)
            lastAgentAggregateKey = nil
            presentEmptyAgents()
            refreshWorkspaceSidebar(force: true)
            return
        }
        lastAgentAggregateKey = target.key
        workspacePresentation = .agents(target.key)
        activateBackingSlot(slot, force: false)
        if slot.lastSnapshot.activeTab != target.sourceTabId {
            requestSourceTab(target.sourceTabId)
        }
        updatePresentedTabs()
        refreshWorkspaceSidebar(force: true)
    }

    private func activateAgentTab(displayId: UInt32) {
        guard let target = agentAggregateTabs().first(where: { $0.displayId == displayId }) else {
            return
        }
        activateAgents(preferred: target.key)
    }

    /// 固定 Agents 槽没有可投影页时仍可进入；空布局不会制造 Core 拓扑。
    private func presentEmptyAgents() {
        guard case .agents = workspacePresentation else { return }
        _ = content.paneLayout.apply(
            layout: nil,
            panes: [],
            tabId: UInt32.max
        )
        updatePresentedTabs()
    }

    private func closeActiveAggregateAgentTab() {
        guard case .agents(let selectedKey) = workspacePresentation else { return }
        let tabs = agentAggregateTabs()
        guard let current = tabs.first(where: { $0.key == selectedKey }) ?? tabs.first else {
            return
        }
        hiddenAgentTabs.insert(current.key)
        let remaining = agentAggregateTabs()
        if let next = remaining.first {
            activateAgents(preferred: next.key)
        } else if let source = scene(forWorkspaceId: current.workspaceId) {
            workspacePresentation = .workspace
            activateBackingSlot(source, force: false)
            updatePresentedTabs()
        }
        refreshWorkspaceSidebar(force: true)
    }

    /// in-process e2e 只注入 Core 已归一化后的 agent 事实，验证聚合投影本身。
    func cacheStructuredAgentForTesting(
        paneId: UInt32,
        name: String,
        title: String?
    ) {
        guard let activeKey = sceneStack.activeKey,
              let slot = sceneStack.scenes[activeKey]
        else { return }
        let agent = StructuredPaneAgent(
            paneId: paneId,
            displayName: name,
            title: title,
            name: name.lowercased(),
            kind: name.lowercased(),
            status: .working,
            stateChangeSeq: 1,
            revision: 1
        )
        slot.cacheStructuredAgents([agent])
        structuredAgentTestOverrides[workspaceReplicaID(for: slot)] = [agent]
        let tabId = slot.cachedTabIdsByPane?[paneId]
            ?? slot.lastSnapshot.activeTab
        slot.cacheTabTargets(
            tabIdsByPane: [paneId: tabId],
            tabNumbersByPane: [paneId: 1]
        )
        refreshWorkspaceSidebar(force: true)
    }

    /// 面板跳转的统一入口。目标 scene 常驻，由主线程 event pump 持续更新。
    private func routePanelJump(
        workspaceId: String?,
        tabId: UInt32?,
        paneId: UInt32,
        seq: UInt64,
        query: String
    ) {
        if let workspaceId {
            guard let targetScene = scene(forWorkspaceId: workspaceId) else {
                pendingPanelJump = PendingPanelJump(
                    workspaceId: workspaceId,
                    tabId: tabId,
                    paneId: paneId,
                    seq: seq,
                    query: query
                )
                return
            }
            if sceneStack.activeKey != targetScene.key {
                activate(slot: targetScene)
            }
            // paneId 只在目标 Workspace 内有意义，不能拿其它 tmux server
            // 中相同的 pane 数字作为跨 Workspace 跳转的回退。
            let targetPaneIDs = paneIDsForScene(targetScene)
            if !targetPaneIDs.isEmpty, !targetPaneIDs.contains(paneId) {
                pendingPanelJump = PendingPanelJump(
                    workspaceId: workspaceId,
                    tabId: tabId,
                    paneId: paneId,
                    seq: seq,
                    query: query
                )
                return
            }
        }
        pendingPanelJump = nil
        performIfWindowOpen { [weak self] in
            self?.jumpToPane(tabId: tabId, paneId: paneId, seq: seq, query: query)
        }
    }

    private func retryPendingPanelJump() {
        guard let jump = pendingPanelJump else { return }
        if let workspaceId = jump.workspaceId {
            guard let targetScene = scene(forWorkspaceId: workspaceId) else { return }
            let targetPaneIDs = paneIDsForScene(targetScene)
            guard targetPaneIDs.isEmpty || targetPaneIDs.contains(jump.paneId) else { return }
            if sceneStack.activeKey != targetScene.key {
                activate(slot: targetScene)
            }
        }
        pendingPanelJump = nil
        performIfWindowOpen { [weak self] in
            self?.jumpToPane(
                tabId: jump.tabId,
                paneId: jump.paneId,
                seq: jump.seq,
                query: jump.query
            )
        }
    }

    private func acknowledgeWorkspacePane(workspaceId: String, paneId: UInt32) {
        performIfWindowOpen { [weak self] in
            guard let self,
                  let targetScene = self.scene(forWorkspaceId: workspaceId),
                  let sceneWorkspaceID = targetScene.workspaceID
            else {
                return
            }
            _ = self.enqueueCoreAttention(
                workspaceID: sceneWorkspaceID,
                .acknowledge(paneID: paneId)
            )
        }
    }

    private func muteWorkspacePane(
        workspaceId: String,
        paneId: UInt32,
        seconds: UInt64
    ) {
        performIfWindowOpen { [weak self] in
            guard let self,
                  let targetScene = self.scene(forWorkspaceId: workspaceId),
                  let sceneWorkspaceID = targetScene.workspaceID
            else {
                return
            }
            _ = self.enqueueCoreAttention(
                workspaceID: sceneWorkspaceID,
                .mute(paneID: paneId, seconds: seconds)
            )
        }
    }

    /// 跳转到指定 tab + pane（搜索命中 / 注意力行）。
    ///
    /// `tabId` 为 nil 时按 pane 反查（注意力行没有 tab）。tmux window 0
    /// 是真实 tab，不能当哨兵跳过。`seq>0` 时把历史滚到命中行。
    func jumpToPane(tabId: UInt32?, paneId: UInt32, seq: UInt64 = 0, query: String = "") {
        let resolvedTab = tabId ?? cachedTabID(containingPane: paneId)
        if let resolvedTab {
            requestSwitchTab(resolvedTab)
        }
        _ = enqueueCoreTask(
            MuxTask.switchPane(paneId),
            failureMessage: MuxtermI18n.shared.tr(
                .errorSwitchPane,
                arguments: ["id": "\(paneId)"]
            )
        )
        // select-pane 的状态事件可能被 Surface catch-up 推迟；先乐观
        // 更新快照和焦点，与 nextPane 同语义。Core snapshot 在下一轮事件泵对齐。
        activatePaneLocally(paneId, tabId: resolvedTab)
        needsLayoutReload = true
        restoreTerminalFocusIfAllowed()
        if seq > 0 || !query.isEmpty {
            pendingSearchJump = PendingSearchJump(paneId: paneId, seq: seq, query: query)
        }
    }

    /// 注意力面板 Cmd-Enter：打开/关闭独立 replica overlay（W19-E）。
    /// overlay 用选中 pane 的 snapshot 渲染，I/O 走 overlay，不改主布局。
    func toggleReplyOverlay(paneId: UInt32? = nil, workspaceId: String? = nil) {
        if let overlay = replyOverlayView, !content.replyOverlayContainer.isHidden {
            overlay.removeFromSuperview()
            replyOverlayView = nil
            replyOverlayPaneId = nil
            replyOverlayWorkspaceID = nil
            content.replyOverlayContainer.isHidden = true
            content.replyOverlayContainer.setAccessibilityValue("0")
            return
        }
        guard unifiedPanel?.modelTab == .attention else { return }
        let selectedRow = unifiedPanel?.testSelectedAttentionRow()
        let targetPaneId = paneId ?? selectedRow?.pane.paneId
        guard let targetPaneId else { return }
        let targetWorkspaceId = workspaceId ?? selectedRow?.workspaceId
        let targetScene = targetWorkspaceId.flatMap { scene(forWorkspaceId: $0) }
        guard targetWorkspaceId == nil || targetScene != nil else { return }
        let workspaceID = targetScene?.workspaceID ?? activeSceneWorkspaceID
        // Scene 的只读 ViewStore 可能正等待本轮布局提交；可见场景必须以
        // MainWindow 已应用的 lastSnapshot 为准。隐藏 workspace 则只读
        // 自己的 ViewStore，不能用可能碰撞的活动 workspace pane id。
        let windowPane = lastSnapshot.panes.first(where: { $0.id == targetPaneId })
        let scenePane = targetScene?.lastSnapshot.panes.first(where: { $0.id == targetPaneId })
        let targetsActiveScene: Bool
        if let targetWorkspaceId,
           let activeKey = sceneStack.activeKey,
           let activeScene = sceneStack.scenes[activeKey]
        {
            targetsActiveScene = targetScene?.key == activeKey
                || scene(activeScene, matchesWorkspaceId: targetWorkspaceId)
        } else {
            targetsActiveScene = targetWorkspaceId == nil
        }
        let sourcePane: Pane?
        if targetsActiveScene {
            sourcePane = windowPane ?? scenePane
        } else {
            sourcePane = scenePane
        }
        let sourceGrid = sourcePane
            .map { (cols: Int($0.cols), rows: Int($0.rows)) }
        let cachedLastLine = selectedRow.flatMap { row -> String? in
            guard row.pane.paneId == targetPaneId,
                  targetWorkspaceId == nil || row.workspaceId == targetWorkspaceId
            else {
                return nil
            }
            let line = row.pane.lastLine.trimmingCharacters(in: .newlines)
            return line.isEmpty ? nil : line
        }
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
        replyOverlayWorkspaceID = workspaceID
        content.replyOverlayContainer.isHidden = false
        // 手动布局（不依赖容器 Auto Layout，headless 下容器高度可能为 0）。
        window?.layoutIfNeeded()
        content.layoutSubtreeIfNeeded()
        let overlayFrame = content.bounds.insetBy(dx: 24, dy: 24)
        content.replyOverlayContainer.frame = overlayFrame
        overlay.frame = content.replyOverlayContainer.bounds
        overlay.layoutSubtreeIfNeeded()
        // Pane snapshot 是在源 pane 字符格上生成的完整 VT 流。replica 若按
        // overlay 的像素尺寸解析，窄屏会因行数不同把顶部内容滚出可见区。
        _ = overlay.syncSizeToPty(notifyResize: false)
        // AttentionSnapshot 已经由 event pump 缓存在前端值类型模型中。
        // 先同步显示最新稳定行，不能把兜底放进异步 pane-output 回调；
        // Core command queue 繁忙时 overlay 也必须立即可用。这里不追加换行，
        // 避免初始极小网格只有一行时把唯一内容滚出可见区。
        if let cachedLastLine {
            overlay.feedOutput(
                Data(cachedLastLine.utf8),
                isSnapshot: true,
                snapshotGrid: sourceGrid
            )
        }
        seedReplyOverlay(
            overlay,
            workspaceID: workspaceID,
            paneID: targetPaneId,
            attemptsRemaining: 50,
            requestedSnapshot: false,
            sourceGrid: sourceGrid
        )
        content.replyOverlayContainer.setAccessibilityValue("1")
    }

    /// Core 可能还没为后台 pane 完成 attach seed。第一次读到空缓存时显式
    /// 请求 runtime-neutral snapshot；后续读取与请求都经过统一命令队列，
    /// overlay 身份守卫则阻止旧请求串到新 pane。
    private func seedReplyOverlay(
        _ overlay: MuxTerminalView,
        workspaceID: String?,
        paneID: UInt32,
        attemptsRemaining: Int,
        requestedSnapshot: Bool,
        sourceGrid: (cols: Int, rows: Int)?
    ) {
        _ = enqueuePaneOutput(
            workspaceID: workspaceID,
            paneID: paneID
        ) { [weak self, weak overlay] raw in
            guard let self,
                  let overlay,
                  self.replyOverlayView === overlay,
                  self.replyOverlayPaneId == paneID,
                  self.replyOverlayWorkspaceID == workspaceID,
                  !self.content.replyOverlayContainer.isHidden
            else {
                return
            }
            if !raw.isEmpty {
                // pane output 是一个完整 VT 字节流；尾部的光标定位/退出
                // alternate-screen 并不代表新帧，不能据此裁掉前面的正文。
                overlay.feedOutput(raw, isSnapshot: true, snapshotGrid: sourceGrid)
                return
            }
            guard attemptsRemaining > 1 else { return }
            if !requestedSnapshot {
                _ = self.enqueueCoreTask(
                    workspaceID: workspaceID,
                    MuxTask.requestPaneSnapshot(paneID),
                    failureMessage: ""
                )
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(100)) { [weak self, weak overlay] in
                guard let self, let overlay else { return }
                self.seedReplyOverlay(
                    overlay,
                    workspaceID: workspaceID,
                    paneID: paneID,
                    attemptsRemaining: attemptsRemaining - 1,
                    requestedSnapshot: true,
                    sourceGrid: sourceGrid
                )
            }
        }
    }

    /// 回底：把当前 pane 的 viewport 重置到最新（W16a jump-latest）。
    @objc func jumpToLatest() {
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        else {
            return
        }
        terminalManager.scrollToLatest(paneId: pane)
        viewportOffsets[pane] = 0
        content.setJumpLatestVisible(false, unseenLines: 0)
    }

    @objc func jumpToLastSeen() {
        guard let target = lastSeenJump else { return }
        applyPaneViewport(paneId: target.paneId, offset: target.offset)
        // 点击后消费这次离开提示；下一次完整的离开→返回才建立新 marker。
        lastSeenLineSeq.removeValue(forKey: target.paneId)
        lastSeenOffsetFailureSince.removeValue(forKey: target.paneId)
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
        commandMarksCache[CommandMarksKey(
            workspaceID: activeSceneWorkspaceID,
            paneID: paneId
        )] ?? []
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
        viewportOffsets[paneId] = offset
        terminalManager.applyViewport(paneId: paneId, offset: offset)
        content.setJumpLatestVisible(
            offset > 0,
            unseenLines: terminalManager.unseenLineCount(paneId: paneId)
        )
    }

    private func wireTerminalManagerCallbacks() {
        terminalManager.onError = { [weak self] message in
            self?.reportStatusError(message)
        }
        terminalManager.enqueueCoreCommand = { [weak self] command in
            self?.enqueueCoreCommand(command) ?? false
        }
        terminalManager.onViewportChanged = { [weak self, weak manager = terminalManager] paneId, offset in
            guard let self, let manager, manager === self.terminalManager else { return }
            self.viewportOffsets[paneId] = offset
            // 用户滚轮/触控板改变视口时，下一次命令导航应从当前状态重新开始；
            // 程序化 command jump 只保留刚设置的游标一次。
            if self.commandNavigationPanes.remove(paneId) == nil {
                self.commandTimelineCursor.removeValue(forKey: paneId)
            }
            // 隐藏 tab/pane 的输出仍会滚动，只更新它自己的位置，不能动当前按钮。
            guard paneId == self.activePaneID else { return }
            self.content.setJumpLatestVisible(
                offset > 0,
                unseenLines: manager.unseenLineCount(paneId: paneId)
            )
            if self.lastSeenVisiblePane == paneId {
                self.dismissLastSeenOffer(for: paneId)
            }
            self.refreshHistoryChrome(for: paneId)
        }
        terminalManager.onUnseenLinesChanged = { [weak self, weak manager = terminalManager] paneId, count in
            guard let self, let manager, manager === self.terminalManager,
                  paneId == self.activePaneID
            else { return }
            let offset = self.viewportOffsets[paneId] ?? 0
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
            guard beginPendingWorkspaceOpen(config: choice.config, stage: .attach) else { return }
            connectCatalogTarget(config: choice.config, intent: .attachOnly) { [weak self] result in
                self?.finishCatalogConnect(result)
            }
        case .shell:
            // Shell 没有 Discover；防御未知/未来候选时仍走严格 attach-only。
            guard beginPendingWorkspaceOpen(config: choice.config, stage: .attach) else { return }
            connectCatalogTarget(config: choice.config, intent: .attachOnly) { [weak self] result in
                self?.finishCatalogConnect(result)
            }
        }
    }

    /// Immediately show a selectable Workspace placeholder while Core opens
    /// the real item. A second open request selects the existing placeholder
    /// instead of racing the shared Core handle.
    @discardableResult
    private func beginPendingWorkspaceOpen(
        config: TargetConfig,
        stage: ConnectProgressStage
    ) -> Bool {
        if pendingWorkspaceOpen != nil {
            presentPendingWorkspaceOpen()
            return false
        }
        pendingWorkspaceOpen = PendingWorkspaceOpen(
            id: "__muxterm_opening__.\(UUID().uuidString)",
            config: config,
            stage: stage
        )
        presentPendingWorkspaceOpen()
        refreshWorkspaceSidebar(force: true)
        unifiedPanel.refreshData()
        return true
    }

    private func updatePendingWorkspaceOpen(stage: ConnectProgressStage) {
        guard var pending = pendingWorkspaceOpen else { return }
        pending.stage = stage
        pendingWorkspaceOpen = pending
        if workspacePresentation == .connecting(pending.id) {
            content.setConnectProgress(stage: stage, title: pending.config.name)
        }
        refreshWorkspaceSidebar(force: true)
        unifiedPanel.refreshData()
    }

    private func presentPendingWorkspaceOpen() {
        guard let pending = pendingWorkspaceOpen else { return }
        workspacePresentation = .connecting(pending.id)
        content.setConnectProgress(stage: pending.stage, title: pending.config.name)
        updatePresentedTabs()
        refreshWorkspaceSidebar(force: true)
    }

    private func finishPendingWorkspaceOpen(error: Error? = nil, prefix: String? = nil) {
        let pendingID = pendingWorkspaceOpen?.id
        pendingWorkspaceOpen = nil
        if let pendingID, workspacePresentation == .connecting(pendingID) {
            workspacePresentation = .workspace
            content.setConnectProgress(stage: nil)
            updatePresentedTabs()
        }
        refreshWorkspaceSidebar(force: true)
        unifiedPanel.refreshData()
        if let error {
            showError(error, prefix: prefix)
        }
    }

    /// 按 QuickConnect 目标连接：tmux 有 name → attach，无 name → 创建；
    /// shell runtime → 本地/远程 shell 在 path 启动。
    func connect(config: TargetConfig) {
        unifiedPanel.dismiss()
        guard beginPendingWorkspaceOpen(config: config, stage: .resolving) else { return }
        // recents 由 SceneStack 派生：连接成功后 sceneStack.activate 会更新列表。
        switch config.runtime {
        case .tmux:
            connectProject(config: config)
        case .herdr:
            connectHerdrProject(config: config)
        case .shell:
            startShell(config: config)
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
                    self.finishPendingWorkspaceOpen()
                    // attachTmux 内部已通过 sceneStack 激活 slot 并切换渲染。
                case .failure(let error):
                    box.flow.attachExistingFailed(message: error.localizedDescription)
                    self.updatePendingWorkspaceOpen(stage: .resolving)
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
                        self.finishPendingWorkspaceOpen(
                            error: error,
                            prefix: "create session failed"
                        )
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
                    self.finishPendingWorkspaceOpen()
                    // attachTmux 内部已通过 sceneStack 激活 slot 并切换渲染。
                case .failure(let error):
                    box.flow.attachCreatedFailed(message: error.localizedDescription)
                    self.activeProjectFlow = nil
                    self.finishPendingWorkspaceOpen(
                        error: error,
                        prefix: "attach created session failed"
                    )
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
        var target = config
        target.session = session
        let initialClientSize = initialTmuxClientSizeHint()
        connectCatalogTarget(
            config: target,
            intent: .attachOnly,
            initialClientSize: initialClientSize
        ) { result in
            switch result {
            case .success(let connection):
                completion(.success(connection.bridge))
            case .failure(let error):
                completion(.failure(error))
            }
        }
    }

    private func startShell(config: TargetConfig) {
        // Shells 每台机器只有一个真实 shell Workspace；同一 transport 的后续
        // 入口复用已有 scene，需要更多本机 shell 时在槽内新建 tab。
        if let existing = sceneStack.scenes.values.first(where: {
            $0.visibility != .closed
                && $0.targetConfig.runtime == .shell
                && $0.targetConfig.transport == config.transport
        }) {
            activate(slot: existing)
            finishPendingWorkspaceOpen()
            return
        }
        connectCatalogTarget(config: config, intent: .createIfMissing) { [weak self] result in
            guard let self else { return }
            self.finishCatalogConnect(result)
        }
    }

    /// Herdr Project 先 AttachOnly；无匹配再 CreateIfMissing。
    /// 未填 session 时 Core 补 default；SSH 通过远端 session list 解析 socket，
    /// 不在这里偷选或 start server。
    private func connectHerdrProject(config: TargetConfig) {
        updatePendingWorkspaceOpen(stage: .attach)
        connectCatalogTarget(
            config: config,
            intent: .attachOnly,
            initialClientSize: initialTmuxClientSizeHint()
        ) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let connection):
                self.quickConnectStore.upsertProject(connection.target)
                self.finishCatalogConnect(.success(connection))
            case .failure:
                self.connectCatalogTarget(
                    config: config,
                    intent: .createIfMissing,
                    initialClientSize: self.initialTmuxClientSizeHint()
                ) { [weak self] createResult in
                    guard let self else { return }
                    if case .success(let connection) = createResult {
                        self.quickConnectStore.upsertProject(connection.target)
                    }
                    self.finishCatalogConnect(createResult)
                }
            }
        }
    }

    /// descriptor-aware Runtime 建连；Catalog resolver 返回的 canonical target
    /// 用于 canonical scene key 与 Recent，不能从 WorkspaceId 五段字符串反推。
    private func connectCatalogTarget(
        config: TargetConfig,
        intent: CoreTargetOpenIntent,
        initialClientSize: (UInt16, UInt16)? = nil,
        completion: @escaping (Result<CatalogConnection, Error>) -> Void
    ) {
        let pendingID = pendingWorkspaceOpen?.id
        let requestedKey = Self.connectionKey(config: config, session: config.session)
        if let slot = sceneStack.scenes[requestedKey], slot.visibility != .closed {
            let canonical = QuickConnect.mergingProjectMetadata(
                resolved: slot.targetConfig,
                requested: config
            )
            slot.targetConfig = canonical
            if pendingID == nil || workspacePresentation == .connecting(pendingID!) {
                activate(slot: slot)
            }
            completion(.success(CatalogConnection(bridge: slot.bridge, target: canonical)))
            return
        }

        let sharedBridge = bridge
        sharedCoreOperationInFlight = true
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                let opened = try sharedBridge.openWorkspace(
                    target: config,
                    intent: intent,
                    initialClientSize: initialClientSize
                )
                let resolved = opened.target
                let key = Self.connectionKey(config: resolved, session: resolved.session)
                DispatchQueue.main.async {
                    guard let self else {
                        return
                    }
                    self.sharedCoreOperationInFlight = false
                    let shouldActivate = pendingID == nil
                        || self.workspacePresentation == .connecting(pendingID!)
                    if let existing = self.sceneStack.scenes[key],
                       existing.visibility != .closed
                    {
                        let canonical = QuickConnect.mergingProjectMetadata(
                            resolved: existing.targetConfig,
                            requested: resolved
                        )
                        existing.targetConfig = canonical
                        if shouldActivate {
                            self.activate(slot: existing)
                        }
                        completion(.success(CatalogConnection(
                            bridge: existing.bridge,
                            target: canonical
                        )))
                        return
                    }
                    let slot = self.makeCatalogWorkspaceScene(
                        opened: opened,
                        sharedBridge: sharedBridge
                    )
                    if shouldActivate {
                        self.activate(slot: slot)
                    } else {
                        self.insertHidden(slot: slot)
                    }
                    completion(.success(CatalogConnection(bridge: sharedBridge, target: resolved)))
                }
            } catch {
                DispatchQueue.main.async {
                    self?.sharedCoreOperationInFlight = false
                    completion(.failure(error))
                }
            }
        }
    }

    /// An open that completes in the background joins the local SceneStack
    /// without stealing focus from the Workspace selected while it loaded.
    private func insertHidden(slot: WorkspaceScene) {
        if slot.openedOrder == 0 {
            slot.openedOrder = nextWorkspaceOpenedOrder
            nextWorkspaceOpenedOrder += 1
        }
        let (_, created) = sceneStack.insertHidden(key: slot.key) { _ in slot }
        guard created else { return }
        quickConnectStore.replaceAllRecents(sceneStack.allRecentTargetConfigs())
        refreshWorkspaceSidebar(force: true)
        unifiedPanel.refreshData()
        presentWorkspaceCapacityWarningIfNeeded()
    }

    private func finishCatalogConnect(_ result: Result<CatalogConnection, Error>) {
        switch result {
        case .success:
            finishPendingWorkspaceOpen()
        case .failure(let error):
            finishPendingWorkspaceOpen(error: error)
        }
    }

    /// Catalog 打开后立刻种下拓扑快照，首帧不会短暂画出上一个 Workspace。
    private func makeCatalogWorkspaceScene(
        opened: CoreWorkspaceOpenResult,
        sharedBridge: CoreBridge
    ) -> WorkspaceScene {
        let resolved = opened.target
        let key = Self.connectionKey(config: resolved, session: resolved.session)
        let slot = WorkspaceScene(
            key: key,
            bridge: sharedBridge,
            workspaceID: opened.id,
            terminalManager: TerminalManager(
                bridge: sharedBridge,
                workspaceID: opened.id,
                runtimeID: resolved.runtime.rawValue,
                fontFamily: terminalFontSettings.family,
                fontSize: terminalFontSettings.size
            ),
            targetConfig: resolved,
            now: 0
        )
        if let size = opened.appliedClientSize {
            slot.terminalManager.noteClientSize(size)
        }
        slot.cacheSnapshot(sharedBridge.snapshot(workspaceID: opened.id))
        return slot
    }

    /// 可见 scene 才是当前 Workspace；共享 handle 的 `session` 只是遗留字段。
    var activeWorkspaceSession: String? {
        if let session = sceneStack.currentTargetConfig?.session, !session.isEmpty {
            return session
        }
        if let key = sceneStack.activeKey,
           let session = sceneStack.scenes[key]?.key.session,
           !session.isEmpty
        {
            return session
        }
        return bridge.session
    }

    /// 切 scene 时把遗留 identity 字段跟上可见 Workspace，避免测试和状态栏读到旧 session。
    private func applyVisibleWorkspaceIdentity(_ slot: WorkspaceScene) {
        if let session = slot.targetConfig.session, !session.isEmpty {
            bridge.session = session
        } else if !slot.key.session.isEmpty {
            bridge.session = slot.key.session
        }
        if let socket = slot.targetConfig.socket ?? slot.key.socket, !socket.isEmpty {
            bridge.socket = socket
        }
    }

    /// 从 TargetConfig + session 构造 pool key（连接身份）。
    private static func connectionKey(
        config: TargetConfig,
        session: String?
    ) -> SceneKey {
        let alias: String?
        if case .ssh(let name) = config.transport {
            alias = name
        } else {
            alias = nil
        }
        return SceneKey(
            transport: config.transport.isSSH ? "ssh" : "local",
            alias: alias,
            session: session ?? (config.runtime == .tmux ? config.name : ""),
            runtime: config.runtime.rawValue,
            path: config.path,
            socket: config.socket,
            workspaceID: config.workspaceID
        )
    }

    /// 激活常驻 scene：立即切换渲染源，Core 前台意图排到事件泵提交。
    /// Core event pump 已经持续维护所有 scene 的快照，因此不存在前台校准。
    func activate(slot: WorkspaceScene) {
        if slot.targetConfig.runtime == .shell {
            let activeTab = slot.lastSnapshot.tabs.first(where: \.isActive)?.id
                ?? slot.lastSnapshot.tabs.first?.id
            activateShells(
                selectFirstLocal: false,
                preferredWorkspaceId: workspaceReplicaID(for: slot),
                preferredTabId: activeTab
            )
            return
        }
        workspacePresentation = .workspace
        content.setConnectProgress(stage: nil)
        activateBackingSlot(slot, force: true)
        refreshWorkspaceSidebar(force: true)
    }

    /// 聚合槽和普通 Workspace 共用的真实 scene 激活路径。
    private func activateBackingSlot(_ slot: WorkspaceScene, force: Bool) {
        guard !isClosing else { return }
        content.setConnectProgress(stage: nil)
        if !force,
           sceneStack.activeKey == slot.key,
           bridge === slot.bridge,
           slot.visibility == .visible
        {
            return
        }

        let hasPendingSurfaceCatchUp = slot.hasPendingSurfaceWork
        if slot.openedOrder == 0 {
            slot.openedOrder = nextWorkspaceOpenedOrder
            nextWorkspaceOpenedOrder += 1
        }
        let (_, created) = sceneStack.activate(key: slot.key) { _ in slot }
        quickConnectStore.replaceAllRecents(sceneStack.allRecentTargetConfigs())
        bridge = slot.bridge
        bridge.selectWorkspace(slot.workspaceID)
        applyVisibleWorkspaceIdentity(slot)
        terminalManager = slot.terminalManager
        terminalManager.setBridgeQueriesEnabled(false)
        trafficRateSampler.reset()
        lastSeenLineSeq.removeAll()
        pendingLastSeenPanes.removeAll()
        lastSeenJump = nil
        lastSeenVisiblePane = nil
        lastSeenVisibleUntil = nil
        lastSeenOffsetFailureSince.removeAll()
        content.setLastSeenVisible(false)
        commandTimelineCursor.removeAll()
        commandNavigationPanes.removeAll()
        viewportOffsets.removeAll()
        paneLatestLineSeqCache.removeAll()
        paneViewportOffsetForSeqCache.removeAll()
        wireTerminalManagerCallbacks()
        let restoredParkedTree = content.paneLayout.replaceTerminalManager(slot.terminalManager)
        content.paneLayout.dropParked(
            except: Array(sceneStack.scenes.values.map(\.terminalManager))
        )
        content.statusBar.allowsTabReordering = terminalManager.usesClientResize
        content.paneLayout.allowsPaneBreak = terminalManager.supportsMultiTab
        // scene 的 TerminalManager 各自保存字体状态：切回时沿用当前字号，
        // 避免旧 slot 还是切换前的小字体。字号没变就不要 resetFont，那会清选区。
        terminalManager.setFont(
            family: terminalFontSettings.family,
            size: terminalFontSettings.size,
            container: content.paneLayout
        )
        // scene 的视图沿用当前主题 palette（终端跟随主题）。
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
        CoreBridge.log(
            "workspace activation target=\(slot.targetConfig.name) restored=\(restoredParkedTree)"
        )
        paintCachedForegroundActivation(slot, restoredParkedTree: restoredParkedTree)
        completeWorkspaceActivation(
            slot: slot,
            hasPendingSurfaceCatchUp: hasPendingSurfaceCatchUp,
            created: created
        )
    }

    /// 用 scene 的 ViewStore 完成最小视觉切换；切换不调用 Core。
    private func paintCachedForegroundActivation(
        _ slot: WorkspaceScene,
        restoredParkedTree: Bool
    ) {
        let snapshot = slot.lastSnapshot
        guard !snapshot.tabs.isEmpty || !snapshot.panes.isEmpty else {
            refreshWorkspaceSidebar()
            return
        }

        lastSnapshot = snapshot
        tabSwitchGate.onSnapshot(tabs: snapshot.tabs.map(\.id))
        updatePresentedTabs(fallback: snapshot.tabs)
        terminalManager.updatePaneSizes(snapshot.panes)
        let revealed = restoredParkedTree
            && content.paneLayout.revealCachedTab(snapshot.activeTab) != nil
        if !revealed,
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

    private func completeWorkspaceActivation(
        slot: WorkspaceScene,
        hasPendingSurfaceCatchUp: Bool,
        created: Bool
    ) {
        guard !isClosing,
              slot.visibility == .visible,
              bridge === slot.bridge
        else { return }
        CoreBridge.log("workspace activation ready target=\(slot.targetConfig.name)")

        // Cached painting is complete; re-enable bridge-backed geometry only
        // at the next event-pump boundary.
        terminalManager.setBridgeQueriesEnabled(false)
        content.paneLayout.resumeGeometrySync()
        focusActiveTerminal()
        deferredBridgeWorkScene = slot
        statusBarNeedsRefresh = true
        // 切连接后立即更新 SSH 状态 + 流量监控显示。
        updateTrafficMonitor()
        // 这一拍只读后台缓存；下一个正常 poll 再做 active bridge 的
        // attention acknowledge/snapshot，避免再次把远端查询放回点击栈。
        refreshAttentionChrome(allowBridgeQueries: false)
        // 后台 metadata 可能在 visibility 切换为 active 的竞态窗口内完成。
        // 这些通知直接由 event pump 交给当前 scene 的 ViewStore 消费。
        postAttentionNotifications(slot.takePendingAttentionNotifications())
        refreshWorkspaceSidebar()
        unifiedPanel.refreshData()
        if hasPendingSurfaceCatchUp || slot.hasPendingSurfaceWork {
            enqueueSurfaceCatchUp(slot)
        }
        if created {
            presentWorkspaceCapacityWarningIfNeeded()
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
        let snap = lastSnapshot
        guard !snap.panes.isEmpty else { return }
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
        switch workspacePresentation {
        case .workspace:
            requestSourceTab(tabId)
        case .shells:
            activateShellTab(displayId: tabId)
        case .agents:
            activateAgentTab(displayId: tabId)
        case .connecting:
            return
        }
    }

    /// 对当前真实 Runtime 发切 Tab；聚合 display id 不得越过此边界。
    private func requestSourceTab(_ tabId: UInt32) {
        guard tabId != lastSnapshot.activeTab else { return }
        // AppKit can deliver a button action twice before the next poll updates
        // lastSnapshot. The gate is the single in-flight command for a target;
        // coalesce only the same pending target (different targets remain valid
        // rapid navigation and replace the pending request).
        if tabSwitchGate.pendingTab == tabId, !tabSwitchGate.isReleased() {
            return
        }
        let departingPane = activePaneID
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
        guard enqueueCoreTask(
            MuxTask.switchTab(tabId),
            failureMessage: MuxtermI18n.shared.tr(
                .errorSwitchTab,
                arguments: ["id": "\(tabId)"]
            )
        ) else {
            tabSwitchGate = TabSwitchGate()
            return
        }
        // A tab switch with no cached tree has no pane to select yet. Clear
        // stale Agent/Command highlights now; the next authoritative snapshot
        // will select the target row if it is a known sidebar item.
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: tabId,
            paneId: cachedTargetPane,
            workspaceSelectionId: presentedWorkspaceSelectionID
        )
        if let departingPane {
            markLastSeenPending(for: departingPane)
        }
        // 等 STATE_ACTIVE_TAB_CHANGED 到达后再用权威 snapshot 对齐；
        // 缓存命中时画面已经切过去了。
    }

    /// 已缓存的 tab：只挂树、对一下 snapshot，不重建、不 refresh-client -C。
    /// 返回 true 表示第一次进入且本地 layout 还没齐，需要走全量 refreshUI。
    @discardableResult
    private func applyCachedTabSwitch(_ tabId: UInt32) -> Bool {
        // requestSwitchTab 已在命令入队时登记待解析的离开基线；同一个
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
            updatePresentedTabs(fallback: lastSnapshot.tabs)
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
        updatePresentedTabs(fallback: lastSnapshot.tabs)
        terminalManager.updatePaneSizes(panes)
        terminalManager.flushSeedsNow(paneIds: Set(panes.map(\.id)))
        focusVisibleTab(lastSnapshot)
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
        guard let pane = lastSnapshot.panes.first(where: \.isActive)?.id ?? lastSnapshot.panes.first?.id else {
            return
        }
        splitPane(pane, horizontal: horizontal)
    }

    private func splitPane(_ pane: UInt32, horizontal: Bool) {
        _ = enqueueCoreTask(
            MuxTask.splitPane(targetPane: pane, horizontal: horizontal),
            failureMessage: MuxtermI18n.shared.tr(
                .errorSplitPane,
                arguments: ["id": "\(pane)"]
            )
        )
        needsLayoutReload = true
    }

    /// 在当前 tab 的布局叶子中循环切换 pane。
    ///
    /// 这里显式发送目标 pane，而不是发送 NextPane/PrevPane 让核心再次
    /// 从全局 active 状态推断。这样 tab 切换后 Cmd+[ / Cmd+] 的行为只
    /// 依赖当前 tab 快照，不会因为旧 staticlib 或焦点事件顺序回到首 tab。
    private func movePane(offset: Int) {
        // `lastSnapshot` is maintained by the active scene ViewStore and the
        // single event pump. Reading CoreBridge here would make a keyboard
        // shortcut wait for a remote tmux pause/capture round-trip.
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

        _ = enqueueCoreTask(
            MuxTask.switchPane(target),
            failureMessage: MuxtermI18n.shared.tr(
                .errorSwitchPane,
                arguments: ["id": "\(target)"]
            )
        )

        // tmux 选择另一个 pane 会自动退出 zoom；重新对目标 pane 执行
        // `resize-pane -Z`，让“切换的是另一个 pane 的全屏状态”成立。
        if terminalManager.usesClientResize {
            let wasZoomed = PaneFullscreenPolicy.zoomedPaneID(
                layoutPaneIDs: snap.layout?.leafPaneIDs() ?? [],
                paneIDs: snapshotPaneIDs
            ) != nil
            if wasZoomed {
                _ = enqueueCoreTask(
                    MuxTask.togglePaneFullscreen(target),
                    failureMessage: MuxtermI18n.shared.tr(.errorCommandFailed)
                )
            }
            if wasZoomed {
                lastSnapshot.layout = .leaf(paneId: target)
                _ = content.paneLayout.apply(
                    layout: lastSnapshot.layout,
                    panes: lastSnapshot.panes,
                    tabId: lastSnapshot.activeTab
                )
                content.paneLayout.refreshSurfaceGeometry(paneId: target)
            }
        } else if content.paneLayout.testFullscreenPaneID != nil {
            content.paneLayout.setFullscreenPane(paneId: target)
            content.paneLayout.refreshSurfaceGeometry(paneId: target)
        }

        // select-pane 的状态事件稍后才会到达；先乐观更新焦点、tab pane
        // 高亮和快照，连续 Cmd/Alt+[ ] 因而不依赖下一次远端 poll。
        activatePaneLocally(target)
        restoreTerminalFocusIfAllowed()
    }

    /// Core 的 active-pane 事件可能落后一轮远端 poll。所有本地选择入口先
    /// 同步同一份产品状态，确保高亮、键盘输入和 Cmd-Enter 缩放目标一致。
    private func activatePaneLocally(_ paneId: UInt32, tabId: UInt32? = nil) {
        guard lastSnapshot.panes.contains(where: { $0.id == paneId }) else { return }
        lastSnapshot.activePane = paneId
        lastSnapshot.panes = lastSnapshot.panes.map { pane in
            Pane(
                id: pane.id,
                cols: pane.cols,
                rows: pane.rows,
                isActive: pane.id == paneId,
                title: pane.title
            )
        }
        content.paneLayout.markActivePane(paneId)
        terminalManager.focusTarget = terminalManager.view(for: paneId)
        workspaceSidebar.setActiveTarget(
            workspaceId: activeWorkspaceReplicaID,
            tabId: tabId ?? lastSnapshot.activeTab,
            paneId: paneId,
            workspaceSelectionId: presentedWorkspaceSelectionID
        )
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
                title: i18n.tr(.cycleLayout),
                detail: i18n.tr(.cycleLayoutDetail),
                keywords: "layout cycle rotate next-layout 布局 切换 上下 左右",
                kind: .command(.cycleLayout)
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
        // 多 Workspace 时只关闭当前 Scene/Core Workspace；最后一个仍走显式
        // Task::Detach 关闭窗口并保留 tmux session。
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
        case .command(.cycleLayout):
            commandPalette.dismiss()
            cycleActiveLayout()
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
            detachCurrentWorkspace()
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
        let transport: TargetTransport
        switch target {
        case .local:
            transport = .local
        case .ssh(let host):
            transport = .ssh(name: host.alias)
        }
        let config = TargetConfig(
            name: target.displayName,
            runtime: .tmux,
            transport: transport,
            path: "",
            session: session,
            socket: resolvedSocket
        )
        guard beginPendingWorkspaceOpen(config: config, stage: .attach) else { return }
        let initialClientSize = initialTmuxClientSizeHint()
        connectCatalogTarget(
            config: config,
            intent: .attachOnly,
            initialClientSize: initialClientSize
        ) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success:
                self.finishPendingWorkspaceOpen()
            case .failure(let error):
                self.lastPaletteError = error.localizedDescription
                self.finishPendingWorkspaceOpen(error: error)
            }
        }
    }

    private func closeActiveTab() {
        guard let tab = presentedTabs().first(where: \.isActive) ?? presentedTabs().first else {
            return
        }
        closeTab(tab.id)
    }

    func closeTab(_ tabId: UInt32) {
        switch workspacePresentation {
        case .agents:
            guard let target = agentAggregateTabs().first(where: { $0.displayId == tabId }) else {
                return
            }
            workspacePresentation = .agents(target.key)
            closeActiveAggregateAgentTab()
            return
        case .shells:
            guard let target = shellAggregateTabs().first(where: { $0.displayId == tabId }),
                  let slot = scene(forWorkspaceId: target.workspaceId)
            else { return }
            if target.isLocal {
                let localTabs = shellAggregateTabs().filter(\.isLocal)
                guard localTabs.count > 1 else { return }
                workspacePresentation = .shells(
                    workspaceId: target.workspaceId,
                    tabId: target.sourceTabId
                )
                activateBackingSlot(slot, force: true)
                _ = enqueueCoreTask(
                    MuxTask.closeTab(target.sourceTabId),
                    failureMessage: MuxtermI18n.shared.tr(
                        .errorCloseTab,
                        arguments: ["id": "\(target.sourceTabId)"]
                    )
                )
            } else {
                let next = shellAggregateTabs().first { $0.displayId != tabId }
                closeWorkspace(target.workspaceId)
                if let next {
                    activateShells(
                        selectFirstLocal: false,
                        preferredWorkspaceId: next.workspaceId,
                        preferredTabId: next.sourceTabId
                    )
                }
            }
            return
        case .connecting:
            return
        case .workspace:
            break
        }
        _ = enqueueCoreTask(
            MuxTask.closeTab(tabId),
            failureMessage: MuxtermI18n.shared.tr(
                .errorCloseTab,
                arguments: ["id": "\(tabId)"]
            )
        )
        // 等 TabClosed / ActiveTabChanged。点击当拍不要拆当前树。
    }

    private func renamePresentedTab(_ tabId: UInt32) {
        guard case .workspace = workspacePresentation else { return }
        promptRenameTab(tabId)
    }

    @discardableResult
    private func movePresentedTab(from: UInt32, target: UInt32, before: Bool) -> Bool {
        guard case .workspace = workspacePresentation else { return false }
        return moveTab(from: from, target: target, before: before)
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

    /// 可见 Workspace：每个 pane 的 PTY 都进 Surface，不按活动 tab 过滤。
    private func shouldHandleSurfaceEvent(paneId: UInt32) -> Bool {
        SurfaceEventPolicy.shouldDeliver(
            viewCreationEnabled: true,
            hasView: terminalManager.hasView(for: paneId)
        )
    }

    /// Drain the Core handle once and route its owned events by WorkspaceId.
    /// Every Workspace scene is fed by this one main-thread event pump.
    private func pollSharedWorkspaceEvents(
        activeSlot: WorkspaceScene?
    ) -> [StateChange] {
        var sharedSlots = Array(sceneStack.scenes.values)
        guard !sharedSlots.isEmpty else { return [] }

        var activeEvents: [StateChange] = []
        var polledBridges = Set<ObjectIdentifier>()
        while let slot = sharedSlots.popLast() {
            let bridgeID = ObjectIdentifier(slot.bridge)
            guard polledBridges.insert(bridgeID).inserted else { continue }
            let bridgeSlots = sceneStack.scenes.values.filter {
                $0.bridge === slot.bridge
            }
            let events = slot.bridge.pollWorkspaceEvents()
            var byWorkspace: [String: [StateChange]] = [:]
            for event in events {
                byWorkspace[event.workspaceID, default: []].append(event.event)
            }

            for candidate in bridgeSlots {
                guard let workspaceID = candidate.workspaceID,
                      let workspaceEvents = byWorkspace[workspaceID],
                      !workspaceEvents.isEmpty
                else {
                    continue
                }
                candidate.enqueueCoreCommand = { [weak self] command in
                    self?.enqueueCoreCommand(command) ?? false
                }
                if candidate === activeSlot {
                    if candidate.hasPendingSurfaceWork {
                        // 前台仍有积压时，新的 Surface 必须排到旧队列后面，
                        // 不能在 pollOnce 里直接 feed（会越过尚未应用的 Cursor
                        // redraw）。控制/拓扑事件仍立刻返回给 UI。
                        var surface: [StateChange] = []
                        var rest: [StateChange] = []
                        surface.reserveCapacity(workspaceEvents.count)
                        rest.reserveCapacity(workspaceEvents.count)
                        for event in workspaceEvents {
                            if event.isPaneOutput
                                || event.isPaneFrame
                                || event.isPaneSnapshot
                                || event.isPaneHistory
                                || event.isPaneClosed
                                || event.type == STATE_PANE_RESIZED
                            {
                                surface.append(event)
                            } else {
                                rest.append(event)
                            }
                        }
                        if !surface.isEmpty {
                            candidate.ingestSharedEvents(surface)
                            enqueueSurfaceCatchUp(candidate)
                        }
                        activeEvents.append(contentsOf: rest)
                    } else {
                        activeEvents.append(contentsOf: workspaceEvents)
                    }
                } else {
                    candidate.ingestSharedEvents(workspaceEvents)
                    if candidate.hasPendingSurfaceWork {
                        enqueueSurfaceCatchUp(candidate)
                    }
                }
            }
            if let error = slot.bridge.takeError() {
                reportStatusError(error)
            }
        }
        return activeEvents
    }

    /// Resume bridge-backed work after the cached scene is already visible.
    /// This runs at the serialized event-pump boundary after local painting.
    private func flushDeferredSceneBridgeWork() {
        guard let slot = deferredBridgeWorkScene else { return }
        deferredBridgeWorkScene = nil
        guard !isClosing,
              slot.visibility == .visible,
              bridge === slot.bridge
        else {
            return
        }
        terminalManager.setBridgeQueriesEnabled(true)
        content.paneLayout.resumeGeometrySync()
        reportPaneColoursIfNeeded(lastSnapshot.panes)
    }

    func pollOnce() {
        guard !isClosing else { return }
        guard !sharedCoreOperationInFlight else { return }
        // UI actions only enqueue.  This is the single command boundary next
        // to the single workspace-event drain, so Core never races a click
        // handler or needs a frontend lock.
        bridge.flushWorkspaceSelection()
        flushDeferredSceneBridgeWork()
        // Resuming a scene may enqueue deferred input/resize work collected
        // while bridge queries were paused, so flush after the resume as well.
        flushCoreCommandQueue()
        pollImagePaste()
        pollUpdateStatus()
        // 后台排空的事件必须先于 active bridge 的新事件交付。否则切回
        // Workspace 后，新的 PaneOutput 可能越过尚未应用的旧队列。
        if flushActiveSurfaceCatchUpBeforePoll() {
            // 积压 Surface 只推迟新的 pollEvents，不能把已经生效的
            // SwitchPane/面板跳转一起饿死。
            if needsLayoutReload {
                refreshUI()
            }
            retryPendingPanelJump()
            return
        }
        resolvePendingLastSeen()
        let activeSlot = sceneStack.activeKey.flatMap { sceneStack.scenes[$0] }
        let events = pollSharedWorkspaceEvents(activeSlot: activeSlot)
        applyPolledEvents(events)
    }

    /// 更新提醒：Core 推进状态机，这里只取快照并渲染 banner。
    /// 未变化时 `apply` 直接返回，不做布局。
    private func pollUpdateStatus() {
        guard let status = bridge.updateStatus() else { return }
        content.updateBanner.apply(status)
    }

    /// 一键更新/重试：按 Core 当前阶段触发安装或检查。
    func performUpdateAction() {
        guard let status = bridge.updateStatus() else { return }
        let updated: CoreBridge.UpdateStatus?
        switch status.phase {
        case "available":
            updated = bridge.updateInstall()
        case "failed", "up_to_date", "idle":
            updated = bridge.updateCheck()
        default:
            updated = status
        }
        if let updated {
            content.updateBanner.apply(updated)
        }
    }

    /// 菜单/命令面板的「检查更新」入口。
    @objc func checkForUpdates() {
        if let status = bridge.updateCheck() {
            content.updateBanner.apply(status)
        }
    }

    /// 生产事件批次入口；回归测试直接重放同一条路径。
    func applyPolledEvents(_ events: [StateChange]) {
        terminalManager.beginEventBatch()
        defer { terminalManager.endEventBatch() }
        if events.contains(where: { Self.tabNumberTopologyEvents.contains($0.type) }),
           let activeSlot = sceneStack.activeKey.flatMap({ sceneStack.scenes[$0] })
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
        // 同批 full/snapshot 和 diff 必须保持 wire 顺序，不能按类型重排。
        var pendingSurfaceEvents: [StateChange] = []
        for ev in events {
            if ev.isPaneClosed {
                // pane 真正关闭才销毁视图；切 tab / 布局变化保留视图状态。
                terminalManager.removePane(ev.paneId)
                commandMarksCache.removeValue(forKey: CommandMarksKey(
                    workspaceID: activeSceneWorkspaceID,
                    paneID: ev.paneId
                ))
            } else if ev.isPaneSnapshot {
                guard shouldHandleSurfaceEvent(paneId: ev.paneId) else {
                    continue
                }
                // 结构/尺寸事件先同步模型和布局，再 reset + feed snapshot，
                // 否则 Cursor/htop 的 CUP 会按旧网格重放。
                if deferSurfaceEvents {
                    pendingSurfaceEvents.append(ev)
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
                    pendingSurfaceEvents.append(ev)
                } else {
                    terminalManager.handleFrame(paneId: ev.paneId, data: ev.data)
                }
                outputSeen = true
            } else if ev.isPaneHistory {
                guard shouldHandleSurfaceEvent(paneId: ev.paneId) else {
                    continue
                }
                if deferSurfaceEvents {
                    pendingSurfaceEvents.append(ev)
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
                if deferSurfaceEvents {
                    pendingSurfaceEvents.append(ev)
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
                    _ = enqueueCoreAttention(
                        workspaceID: activeSceneWorkspaceID,
                        .becameVisible(paneID: ev.paneId),
                        refreshPanel: false
                    )
                    focusPaneTerminal(ev.paneId)
                }
            } else if ev.isBackendStatus {
                uiStateChanged = true
            } else if ev.type == STATE_STATUS_SUBSCRIPTION {
                // status-left/right 订阅推送（文档 §B+）：零轮询更新原生条。
                if ev.name == "muxterm.status-left" || ev.name == "muxterm.status-right",
                   let value = String(data: ev.data, encoding: .utf8) {
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
            } else if ev.isPaneTitleChanged {
                lastSnapshot.panes = lastSnapshot.panes.map { pane in
                    guard pane.id == ev.paneId else { return pane }
                    return Pane(
                        id: pane.id,
                        cols: pane.cols,
                        rows: pane.rows,
                        isActive: pane.isActive,
                        title: ev.name
                    )
                }
                content.paneLayout.updatePaneTitle(paneId: ev.paneId, title: ev.name)
            } else if ev.type == STATE_PANE_RESIZED {
                // 格子与 Surface 字节一同按源顺序交付，不能先套用批次末尺寸。
                pendingSurfaceEvents.append(ev)
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
                        // Shells 是固定聚合槽；具体 shell 退出只让当前真实
                        // Workspace 进入空拓扑，下面统一决定补 local 或关闭 remote。
                        content.setDisconnected(true)
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
            updatePresentedTabs(fallback: tabs)
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
            refreshStatusBar(force: true)
        }
        // 布局/尺寸同步完成后再喂输出，避免 resize 竞态。
        for event in pendingSurfaceEvents where shouldHandleSurfaceEvent(paneId: event.paneId) {
            if event.type == STATE_PANE_RESIZED,
               let grid = PaneGridSyncPolicy.grid(fromResizeEvent: event.data) {
                terminalManager.handleResize(paneId: event.paneId, cols: grid.cols, rows: grid.rows)
            } else if event.isPaneSnapshot {
                terminalManager.handleSnapshot(paneId: event.paneId, data: event.data)
            } else if event.isPaneFrame {
                terminalManager.handleFrame(paneId: event.paneId, data: event.data)
            } else if event.isPaneHistory {
                terminalManager.handleHistory(paneId: event.paneId, data: event.data)
            } else if event.isPaneOutput {
                terminalManager.handleOutput(paneId: event.paneId, data: event.data)
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
        retryPendingPanelJump()
        let surfaceBusy = sceneStack.activeKey
            .flatMap({ sceneStack.scenes[$0] })?
            .hasPendingSurfaceWork == true
            || !surfaceCatchUpScenes.isEmpty
        refreshAttentionChrome(force: !surfaceBusy)
        if let activePane = activePaneID {
            refreshHistoryChromeFromCore(for: activePane)
        }
        applyPendingSearchJumpIfReady()
    }

    /// 激活后先处理当前 Workspace 的旧 Surface 队列，再 poll 新事件，保持
    /// Runtime 输出顺序。返回值保留兼容：始终 false，积压改由 catch-up
    /// 异步排空，避免跳过 poll 饿死回显/其它 Workspace，并防止 1ms 忙等打满 CPU。
    private func flushActiveSurfaceCatchUpBeforePoll() -> Bool {
        guard let activeKey = sceneStack.activeKey,
              let slot = sceneStack.scenes[activeKey],
              slot.visibility == .visible,
              slot.hasPendingSurfaceWork
        else {
            return false
        }
        let hasPending = slot.applyPendingSurfaceEvents(
            maxEvents: SurfaceEventBatchPolicy.activeMaxEventsPerPass,
            timeBudget: SurfaceEventBatchPolicy.activeTimeBudget
        )
        if hasPending {
            enqueueSurfaceCatchUp(slot)
        }
        return false
    }

    private func enqueueSurfaceCatchUp(_ slot: WorkspaceScene) {
        enqueueSurfaceCatchUp([slot])
    }

    private func enqueueSurfaceCatchUp(_ slots: [WorkspaceScene]) {
        guard !isClosing else { return }
        for slot in slots where slot.visibility != .closed {
            if !surfaceCatchUpScenes.contains(where: { $0 === slot }) {
                surfaceCatchUpScenes.append(slot)
            }
        }
        scheduleSurfaceCatchUp()
    }

    private func scheduleSurfaceCatchUp() {
        guard !isClosing,
              surfaceCatchUpWorkItem == nil,
              !surfaceCatchUpScenes.isEmpty
        else {
            return
        }
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.surfaceCatchUpWorkItem = nil
            self.flushSurfaceCatchUpPass()
        }
        surfaceCatchUpWorkItem = work
        // 与 EventPump 同频，避免 1ms 忙等把主线程打满（Cursor 积压时 ~100% CPU）。
        DispatchQueue.main.asyncAfter(
            deadline: .now() + SurfaceEventBatchPolicy.catchUpInterval,
            execute: work
        )
    }

    /// 在一个全局主线程预算内轮转所有待处理的 Workspace scene，active 优先。
    private func flushSurfaceCatchUpPass() {
        guard !isClosing else {
            surfaceCatchUpScenes.removeAll()
            return
        }

        let activeKey = sceneStack.activeKey
        let slots = surfaceCatchUpScenes.sorted { lhs, rhs in
            let lhsActive = lhs.key == activeKey
            let rhsActive = rhs.key == activeKey
            if lhsActive != rhsActive {
                return lhsActive
            }
            return lhs.openedOrder < rhs.openedOrder
        }
        surfaceCatchUpScenes.removeAll()

        let started = ProcessInfo.processInfo.systemUptime
        let passBudget = slots.contains(where: { $0.key == activeKey })
            ? SurfaceEventBatchPolicy.activeTimeBudget
            : SurfaceEventBatchPolicy.timeBudget
        for slot in slots where slot.visibility != .closed {
            guard slot.hasPendingSurfaceWork else { continue }
            let elapsed = ProcessInfo.processInfo.systemUptime - started
            let remainingBudget = passBudget - elapsed
            guard remainingBudget > 0 else {
                surfaceCatchUpScenes.append(slot)
                continue
            }
            let slotBudget = slot.key == activeKey
                ? min(remainingBudget, SurfaceEventBatchPolicy.activeTimeBudget)
                : remainingBudget
            let slotMaxEvents = slot.key == activeKey
                ? SurfaceEventBatchPolicy.activeMaxEventsPerPass
                : SurfaceEventBatchPolicy.maxEventsPerPass
            if slot.applyPendingSurfaceEvents(
                maxEvents: slotMaxEvents,
                timeBudget: slotBudget
            ) {
                surfaceCatchUpScenes.append(slot)
            }
        }
        scheduleSurfaceCatchUp()
    }

    /// 注意力引擎：更新状态栏红点 + 弹出 blocked/done 通知。
    ///
    /// `force == false` 时在 Surface 积压路径上按间隔节流：Cursor 洪峰下
    /// 每拍都 decode 全池 attention JSON + 重建 sidebar 会把主线程打满。
    private func refreshAttentionChrome(
        allowBridgeQueries: Bool = true,
        force: Bool = true
    ) {
        guard !isClosing else { return }
        let now = ProcessInfo.processInfo.systemUptime
        if !force,
           now - lastAttentionChromeRefreshAt
           < SurfaceEventBatchPolicy.attentionRefreshMinInterval
        {
            return
        }
        lastAttentionChromeRefreshAt = now
        // 前台 pane 输出视为已看见：CommandDone 清成 Idle（Linux 同款），
        // 前台 `sleep && echo` 不弹完成通知。
        let activePane = lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
        if allowBridgeQueries, let activePane {
            _ = enqueueCoreAttention(
                workspaceID: activeSceneWorkspaceID,
                .becameVisible(paneID: activePane),
                refreshPanel: false
            )
        }
        // Core 返回的是整个 WorkspacePool 的 attention 快照；EventPump 每拍
        // 把它按 WorkspaceId 分发给各 scene 的 ViewStore，隐藏 scene 也持续更新。
        if allowBridgeQueries, let snapshot = attentionSnapshot(from: bridge) {
            lastPoolAttentionSnapshot = snapshot
            updateSceneAttentionStores(snapshot: snapshot)
        }
        if allowBridgeQueries {
            drainAttentionNotifications(from: bridge)
        }
        for slot in sceneStack.scenes.values
            where slot.visibility != .closed
        {
            postAttentionNotifications(slot.takePendingAttentionNotifications())
        }
        let visibleRows = attentionSnapshotForPanel().map {
            AttentionList.rows(from: $0, workspaces: runtimeSidebarItems(), query: "")
        }?.filter { !hiddenAttentionKeys.contains($0.visibilityKey) } ?? []
        content.statusBar.setAttention(StatusBarAttention(
            indicators: visibleRows.map(\.indicator)
        ))
        content.statusBar.setTabActivities(presentedTabActivities())
        refreshWorkspaceSidebar()
    }

    private func updateSceneAttentionStores(snapshot: AttentionSnapshot) {
        for scene in sceneStack.scenes.values where scene.visibility != .closed {
            let workspaces = snapshot.workspaces.filter {
                self.scene(scene, matchesWorkspaceId: $0.workspaceId)
            }
            let workspaceID = workspaces.first?.workspaceId ?? workspaceReplicaID(for: scene)
            let blockedCount = workspaces.reduce(into: 0) { count, workspace in
                count += workspace.blocked
            }
            scene.cacheAttentionSnapshot(
                AttentionSnapshot(blockedCount: blockedCount, workspaces: workspaces)
            )
            scene.cacheStructuredAgents(
                scene.bridge.structuredAgentSnapshot(workspaceId: workspaceID)
            )
        }
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
        updatePresentedTabs(fallback: snap.tabs)
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
            // 滚轮回调已经持有最新的 native viewport。Core 写入通过 event
            // pump 排队，不能用尚未应用的旧 offset 把按钮反复闪回去。
            let viewport: UInt32
            if let localViewport = viewportOffsets[activePane] {
                viewport = localViewport
            } else {
                viewport = UInt32(max(0, bridge.paneViewport(paneId: activePane)))
                viewportOffsets[activePane] = viewport
            }
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
        cacheActiveSlotSnapshot(
            tabIdsByPane: snap.tabs.isEmpty ? nil : tabIdsByPane,
            tabNumbersByPane: snap.tabs.isEmpty ? nil : tabNumbersByPane
        )
    }

    private func cacheActiveSlotSnapshot(
        tabIdsByPane: [UInt32: UInt32]? = nil,
        tabNumbersByPane: [UInt32: Int]? = nil
    ) {
        guard let activeKey = sceneStack.activeKey,
              let slot = sceneStack.scenes[activeKey],
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

    /// 切 tab/pane 完成后：按 seq 喂历史帧，并用 SwiftTerm findNext 高亮 query。
    private func applyPendingSearchJumpIfReady() {
        guard let jump = pendingSearchJump
        else { return }
        guard tabSwitchGate.isReleased() else { return }
        guard lastSnapshot.panes.contains(where: { $0.id == jump.paneId }) else { return }
        pendingSearchJump = nil
        if jump.seq > 0 {
            let offset = max(0, bridge.paneViewportOffsetForSeq(paneId: jump.paneId, seq: jump.seq))
            if offset > 0 {
                let uoff = UInt32(offset)
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
        needsLayoutReload = true
    }

    /// Mark a departing pane without querying Core from the tab click path.
    /// The event pump resolves the sequence at the next safe drain boundary.
    private func markLastSeenPending(for paneId: UInt32) {
        guard lastSeenLineSeq[paneId] == nil else { return }
        pendingLastSeenPanes.insert(paneId)
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
        let latest = latest ?? paneLatestLineSeqCache[PaneHistoryKey(
            workspaceID: activeSceneWorkspaceID,
            paneID: paneId
        )] ?? -1
        guard let seq = LastSeenNavigation.baselineSequence(latest: latest) else {
            pendingLastSeenPanes.insert(paneId)
            return
        }
        pendingLastSeenPanes.remove(paneId)
        lastSeenLineSeq[paneId] = seq
        lastSeenOffsetFailureSince.removeValue(forKey: paneId)
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
            let key = PaneHistoryKey(
                workspaceID: activeSceneWorkspaceID,
                paneID: paneId
            )
            let latest = bridge.paneLatestLineSeq(paneId: paneId)
            paneLatestLineSeqCache[key] = latest
            recordLastSeen(for: paneId, latest: latest)
        }
    }

    func lastSeenJumpOffsetForTest() -> UInt32? {
        lastSeenJump?.offset
    }

    /// 测试用：last-seen 状态机的三个输入，便于 E2E 失败定位。
    func testLastSeenDiagnostics(paneId: UInt32) -> String {
        let workspaceID = activeSceneWorkspaceID
        let latest = paneLatestLineSeqCache[PaneHistoryKey(
            workspaceID: workspaceID,
            paneID: paneId
        )] ?? -1
        let seen = lastSeenLineSeq[paneId]
        let rawOffset = seen
            .map { seq in
                paneViewportOffsetForSeqCache[PaneHistoryOffsetKey(
                    workspaceID: workspaceID,
                    paneID: paneId,
                    seq: seq
                )] ?? -1
            }
            ?? -1
        return "latest=\(latest) seen=\(seen.map(String.init) ?? "nil") rawOffset=\(rawOffset)"
    }

    private func refreshHistoryChrome(for paneId: UInt32) {
        guard paneId == activePaneID else { return }
        expireLastSeenOfferIfNeeded(for: paneId)
        let workspaceID = activeSceneWorkspaceID
        let latest = paneLatestLineSeqCache[PaneHistoryKey(
            workspaceID: workspaceID,
            paneID: paneId
        )] ?? -1
        let seen = lastSeenLineSeq[paneId]
        let rawOffset = seen.map {
            paneViewportOffsetForSeqCache[PaneHistoryOffsetKey(
                workspaceID: workspaceID,
                paneID: paneId,
                seq: $0
            )] ?? -1
        } ?? -1
        if let offset = LastSeenNavigation.targetOffset(
            latest: latest,
            seen: seen,
            rawOffset: rawOffset
        ) {
            lastSeenOffsetFailureSince.removeValue(forKey: paneId)
            lastSeenJump = (paneId, offset)
            setLastSeenVisible(true, paneId: paneId)
        } else if let seen,
                  latest > 0,
                  UInt64(latest) > seen,
                  rawOffset < 0,
                  let jump = lastSeenJump,
                  jump.paneId == paneId
        {
            let now = ProcessInfo.processInfo.systemUptime
            let firstFailure = lastSeenOffsetFailureSince[paneId] ?? now
            lastSeenOffsetFailureSince[paneId] = firstFailure
            if now - firstFailure < Self.lastSeenOffsetFailureGrace {
                // Keep the last known target until Core either resolves it or
                // confirms that the stable line has been evicted.
                setLastSeenVisible(true, paneId: paneId)
            } else {
                lastSeenOffsetFailureSince.removeValue(forKey: paneId)
                lastSeenJump = nil
                setLastSeenVisible(false, paneId: paneId)
            }
        } else {
            // latest 没有前进或 seq 已 stale 时，清掉旧目标；没有可复用
            // marker 时的瞬时查询失败也不应凭空显示按钮。
            lastSeenOffsetFailureSince.removeValue(forKey: paneId)
            lastSeenJump = nil
            setLastSeenVisible(false, paneId: paneId)
        }

        var ticks: [CommandMarkTick] = []
        for mark in commandMarksCache[CommandMarksKey(
            workspaceID: workspaceID,
            paneID: paneId
        )] ?? [] {
            // Core 返回 nil history_offset 时表示 seq 已淘汰；绝不能
            // 回退成 0，否则点击红/绿刻度会错误跳到 live 底部。
            guard let offset = mark.historyOffset else { continue }
            ticks.append(
                CommandMarkTick(
                    seq: mark.seq,
                    command: mark.command,
                    exitCode: mark.exitCode,
                    offset: offset
                )
            )
        }
        let rows = UInt32(max(1, Int(lastSnapshot.panes.first(where: { $0.id == paneId })?.rows ?? 24)))
        let rawMax = bridge.paneHistoryMaxOffset(paneId: paneId, rows: rows)
        let maxOffset = rawMax < 0 ? (ticks.map(\.offset).max() ?? 0) : UInt32(rawMax)
        content.setCommandMarks(ticks, maxOffset: maxOffset)
    }

    /// Refresh history/index values at the EventPump boundary, then render
    /// from the owned caches so AppKit callbacks remain Core-free.
    private func refreshHistoryChromeFromCore(for paneId: UInt32) {
        guard paneId == activePaneID else { return }
        let workspaceID = activeSceneWorkspaceID
        let historyKey = PaneHistoryKey(workspaceID: workspaceID, paneID: paneId)
        let latest = bridge.paneLatestLineSeq(paneId: paneId)
        paneLatestLineSeqCache[historyKey] = latest
        if let seen = lastSeenLineSeq[paneId] {
            paneViewportOffsetForSeqCache[PaneHistoryOffsetKey(
                workspaceID: workspaceID,
                paneID: paneId,
                seq: seen
            )] = bridge.paneViewportOffsetForSeq(paneId: paneId, seq: seen)
        }
        let marks = bridge.paneCommandMarks(paneId: paneId)
            .filter { $0.exitCode != nil && $0.historyOffset != nil }
        commandMarksCache[CommandMarksKey(
            workspaceID: workspaceID,
            paneID: paneId
        )] = marks
        refreshHistoryChrome(for: paneId)
    }

    private func setLastSeenVisible(_ visible: Bool, paneId: UInt32) {
        if visible {
            guard lastSeenVisiblePane != paneId else { return }
            lastSeenVisiblePane = paneId
            lastSeenVisibleUntil = ProcessInfo.processInfo.systemUptime
                + Self.lastSeenPresentationDuration
        } else {
            // 清除来自旧 active pane 的 marker 也必须是幂等的。切 tab/pane
            // 时刷新函数传入的是新 pane，不能因为 paneId 不同而把旧按钮留在
            // 左上角，造成所有页面闪烁或残留。
            guard lastSeenVisiblePane != nil else { return }
            lastSeenVisiblePane = nil
            lastSeenVisibleUntil = nil
        }
        content.setLastSeenVisible(visible)
    }

    private func expireLastSeenOfferIfNeeded(for paneId: UInt32) {
        guard lastSeenVisiblePane == paneId,
              let deadline = lastSeenVisibleUntil,
              ProcessInfo.processInfo.systemUptime >= deadline
        else { return }
        dismissLastSeenOffer(for: paneId)
    }

    private func dismissLastSeenOffer(for paneId: UInt32) {
        lastSeenLineSeq.removeValue(forKey: paneId)
        pendingLastSeenPanes.remove(paneId)
        lastSeenOffsetFailureSince.removeValue(forKey: paneId)
        if lastSeenJump?.paneId == paneId {
            lastSeenJump = nil
        }
        setLastSeenVisible(false, paneId: paneId)
    }

    /// 当前 Workspace 已空时关掉这一格，切到邻近 Workspace。
    /// 只有再也没有打开的 Workspace 才关整个窗口。
    private func maybeCloseIfSessionEnded() {
        let snap = lastSnapshot
        guard snap.tabs.isEmpty && snap.panes.isEmpty else { return }
        let current = sceneStack.activeKey.flatMap { sceneStack.scenes[$0] }
        if let current,
           current.targetConfig.runtime == .shell,
           current.targetConfig.transport == .local
        {
            _ = enqueueCoreTask(
                MuxTask.newTab(),
                failureMessage: MuxtermI18n.shared.tr(.errorNewTab)
            )
            return
        }
        let others = sceneStack.scenes.values.filter { slot in
            slot.visibility != .closed
                && slot !== current
                && !(slot.lastSnapshot.tabs.isEmpty && slot.lastSnapshot.panes.isEmpty)
        }
        if let workspaceId = activeWorkspaceReplicaID, !others.isEmpty {
            closeWorkspace(workspaceId)
            return
        }
        closeSessionWindow()
    }

    /// 通过 Core SettingsService 事务写配置；失败只提示，不直接改文件。
    private func persistConfig(_ operations: [[String: Any]]) {
        _ = enqueueConfigTransaction(operations) { [weak self] result in
            if case .failure = result {
                self?.reportStatusError(MuxtermI18n.shared.tr(.errorCommandFailed))
            }
        }
    }

    private func reportStatusError(_ message: String) {
        content.statusBar.showError(message)
    }

    /// 新出现的 pane 需要把客户端主题色上报给 tmux，否则 tmux 代答
    /// OSC 10/11 颜色查询时用的是自己的默认色板（codex 黑底黑字/白底白字）。
    private func reportPaneColoursIfNeeded(_ panes: [Pane]) {
        guard terminalManager.usesClientResize else { return }
        let osc = ColorContrast.oscColors(
            fg: MuxtermTerminalColors.activePalette.fg,
            bg: MuxtermTerminalColors.activePalette.bg
        )
        let workspaceID = activeSceneWorkspaceID
        for id in Set(panes.map(\.id)) {
            let key = ColourPaneKey(workspaceID: workspaceID, paneID: id)
            guard !reportedColourPanes.contains(key) else { continue }
            if enqueueCoreColours(
                workspaceID: workspaceID,
                .pane(paneID: id, fgHex: osc.fg, bgHex: osc.bg)
            ) {
                reportedColourPanes.insert(key)
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

    /// 抓取并应用 tmux status bar 快照。所有 Core 访问都在主线程事件泵
    /// 上顺序执行，不再与 poll 竞争同一个 handle。
    private func refreshStatusBar(force: Bool) {
        guard terminalManager.usesClientResize
        else { return }
        if !force, Date().timeIntervalSince(lastStatusFetchAt) < 2 {
            return
        }
        lastStatusFetchAt = Date()
        guard let json = bridge.statusBarSnapshotJSON(),
              let data = json.data(using: .utf8),
              let response = try? JSONDecoder().decode(StatusBarResponse.self, from: data),
              response.ok,
              let snapshot = response.status
        else { return }
        statusBarSnapshot = snapshot
        content.applyStatusBar(snapshot)
        // 文档 §B+：tmux ≥3.2 用 refresh-client -B 订阅推送（零轮询）；
        // 只有老版本才保留 status-interval 轮询定时器。
        if !bridge.statusSubscriptionActive() {
            scheduleStatusRefresh(snapshot)
        }
    }

    /// 结构事件（切 tab / 建删窗口 / 布局变化）后标记 status bar 待刷新。
    /// 实际查询由下一轮 event pump 完成，不从 UI 回调另起 Core 访问。
    private func scheduleStatusBarRefresh() {
        guard terminalManager.usesClientResize else { return }
        statusRefreshWorkItem?.cancel()
        statusRefreshWorkItem = nil
        statusBarNeedsRefresh = true
    }

    /// 按 tmux `status-interval` 周期刷新（时钟/时间类 right 段需要）。
    private func scheduleStatusRefresh(_ snapshot: StatusBarSnapshot) {
        statusRefreshTimer?.invalidate()
        guard snapshot.enabled else { return }
        let interval = TimeInterval(max(5, Int(snapshot.interval)))
        let timer = Timer(timeInterval: interval, repeats: true) { [weak self] _ in
            self?.statusBarNeedsRefresh = true
        }
        RunLoop.main.add(timer, forMode: .common)
        statusRefreshTimer = timer
    }

    func testQueuedCommandCount() -> Int { commandQueue.count }

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
        sceneStack.shutdownAll()
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
        sceneStack.shutdownAll()
        bridge.shutdown()
        window?.close()
    }

    /// 通过 core 的独立 detach FFI 关闭控制 client，保留 tmux session。
    private func detachCurrentWorkspace() {
        guard terminalManager.usesClientResize else { return }
        if runtimeSidebarItems().count > 1,
           let workspaceID = activeWorkspaceReplicaID
        {
            closeWorkspace(workspaceID)
        } else {
            detachSessionWindow()
        }
    }

    /// 通过 core 的独立 detach FFI 关闭最后一个控制 client，保留 tmux session。
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
            if eventWindow === unifiedPanel?.window,
               let action = workspaceNavigationAction(for: event)
            {
                unifiedPanel.dismiss()
                performWorkspaceNavigation(action)
                return nil
            }
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
        // Cmd-P 统一面板可见时，Tab/Shift+Tab/Esc/Enter/↑↓ 走面板。
        // headless e2e 经 testDispatchKeyEvent 调用 handleKey，事件挂在主
        // 窗口上，不会进面板自己的 local monitor。
        if unifiedPanel?.window?.isVisible == true {
            if let offset = CompactPanelKeyNavigation.selectionOffset(for: event) {
                unifiedPanel.moveSelection(offset: offset)
                return true
            }
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
        if performWorkspaceNavigation(action) {
            if unifiedPanel?.window?.isVisible == true {
                unifiedPanel.dismiss()
            }
            return true
        }
        switch action {
        case .newTab:
            newTab()
        case .splitHorizontal:
            splitActivePane(horizontal: true)
        case .splitVertical:
            splitActivePane(horizontal: false)
        case .splitAuto:
            splitActivePaneAuto()
        case .closeWindow:
            closeActiveWindow()
        case .closePane:
            closeFocusedSurface()
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
        case .cycleLayout:
            cycleActiveLayout()
        case .toggleSidebar:
            toggleWorkspaceSidebar()
        case .openShells, .openAgents, .switchWorkspace:
            break
        }
        return true
    }

    /// 独立 NSPanel 的本地事件不会进入主窗口 `handleKey`；这里只识别
    /// Workspace 导航键，其余编辑和面板导航仍交给面板自己的 responder。
    private func workspaceNavigationAction(for event: NSEvent) -> KeyAction? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard let raw = event.charactersIgnoringModifiers, let first = raw.first else {
            return nil
        }
        let chord = KeyChord(
            command: flags.contains(.command),
            shift: flags.contains(.shift),
            option: flags.contains(.option),
            control: flags.contains(.control),
            key: String(first)
        )
        guard let action = KeyBindings.action(for: chord, custom: customKeybindings) else {
            return nil
        }
        switch action {
        case .openShells, .openAgents, .switchWorkspace:
            return action
        default:
            return nil
        }
    }

    @discardableResult
    private func performWorkspaceNavigation(_ action: KeyAction) -> Bool {
        switch action {
        case .openShells:
            activateShells(selectFirstLocal: true)
        case .openAgents:
            activateAgents()
        case .switchWorkspace(let index):
            switchToWorkspaceAtFixedIndex(index)
        default:
            return false
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
            sceneStack.shutdownAll()
            bridge.shutdown()
        }
    }

    private func cancelSurfaceCatchUp() {
        surfaceCatchUpWorkItem?.cancel()
        surfaceCatchUpWorkItem = nil
        surfaceCatchUpScenes.removeAll()
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
        guard replyOverlayView === view else { return }
        let paneId = replyOverlayPaneId ?? view.paneId
        let workspaceID = replyOverlayWorkspaceID
        let payload = Data(data)
        // W19-E：overlay 快速回复不清 Blocked（注意力行保留，Enter 仍可跳转）。
        performIfWindowOpen { [weak self] in
            _ = self?.enqueueCoreInput(
                workspaceID: workspaceID,
                paneId: paneId,
                data: payload,
                quiet: true
            )
        }
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {
        // overlay 不写回 tmux 尺寸（不改主布局 PTY）。
    }
}
