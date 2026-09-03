import AppKit
import MuxtermChrome

private enum UnifiedWorkspaceNavigation: Equatable {
    case root
    case existingConnections
}

private enum UnifiedWorkspaceItem {
    case existingConnections
    case target(
        TargetConfig,
        badges: [QuickBadge],
        isCurrent: Bool,
        workspaceIndex: Int?
    )
    case newProject
    case back
    case existing(ExistingConnectionChoice)
    case loading
    case empty

    var title: String {
        switch self {
        case .existingConnections:
            return MuxtermI18n.shared.tr(.existingConnections)
        case .target(let config, _, _, _):
            return config.name
        case .newProject:
            return MuxtermI18n.shared.tr(.panelNewProject)
        case .back:
            return MuxtermI18n.shared.tr(.existingBack)
        case .existing(let choice):
            return choice.config.name
        case .loading:
            return MuxtermI18n.shared.tr(.existingLoading)
        case .empty:
            return MuxtermI18n.shared.tr(.existingEmpty)
        }
    }

    func matches(_ query: String) -> Bool {
        score(for: query) != nil
    }

    /// Search score keeps the same workspace entry order stable while putting
    /// exact/strong target matches before generic action rows.
    func score(for query: String) -> Int? {
        let normalizedQuery = query.lowercased()
        switch self {
        case .existingConnections, .newProject, .loading, .empty:
            return title.lowercased().contains(normalizedQuery) ? 500 : nil
        case .target(let config, _, _, _):
            return QuickConnect.searchScore(query, config: config)
        case .back:
            return 0
        case .existing(let choice):
            return QuickConnect.searchScore(query, config: choice.config)
        }
    }
}

/// Cmd-P 统一三 tab 面板（对标 Linux `quickconnect_panel.rs` + `panel_model.rs`）。
///
/// 一个 NSPanel：Workspaces / Attention / Search，共享 input，Tab / Shift+Tab
/// 循环，query 跨 tab 保留。旧 QuickConnect / Search / Attention 三个独立
/// 面板的 identifier 在这里做别名，让旧测试继续绿。
final class UnifiedPanelController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    private static let preferredContentSize = NSSize(width: 640, height: 420)

    var onConnect: ((TargetConfig) -> Void)?
    var onLoadExistingConnections: ((@escaping (Result<[ExistingConnectionChoice], Error>) -> Void) -> Void)?
    var onLoadSSHAliases: ((@escaping (Result<[String], Error>) -> Void) -> Void)?
    var onAttachExistingConnection: ((ExistingConnectionChoice) -> Void)?
    var onExistingConnectionsError: ((Error) -> Void)?
    var onEditProject: ((TargetConfig) -> Void)?
    var onNewProject: (() -> Void)?
    var onJump: ((String?, UInt32?, UInt32, UInt64, String) -> Void)?
    // (workspaceId, tabId, paneId, seq, query)
    var onPreview: ((String, UInt32) -> Void)? // (workspaceId, paneId)
    var onMute: ((String, UInt32, UInt64) -> Void)? // (workspaceId, paneId, seconds)
    /// 明确打开 Attention 条目时确认已读；仅切到列表或查看状态不触发。
    var onAcknowledge: ((String, UInt32) -> Void)? // (workspaceId, paneId)
    var onDismissed: (() -> Void)?
    var currentConfig: TargetConfig?

    private let store: QuickConnectStore
    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = MuxtermFillWidthScrollView()
    private let emptyLabel = NSTextField(labelWithString: "")
    private let tabControl = NSSegmentedControl()
    private let accessoryContainer = NSView()
    private let scopeBar = NSStackView()
    private let scopeControl = NSSegmentedControl()
    private let attentionActions = NSStackView()
    private let attentionJumpButton = NSButton(title: "", target: nil, action: nil)
    private let attentionOpenButton = NSButton(title: "", target: nil, action: nil)
    private let attentionMuteButton = NSPopUpButton(frame: .zero, pullsDown: true)
    private var selectionAliases: [String: PanelAccessibilityAliasButton] = [:]
    private var allItems: [UnifiedWorkspaceItem] = []
    private var visibleItems: [UnifiedWorkspaceItem] = []
    private var existingItems: [UnifiedWorkspaceItem] = []
    /// 根目录搜索使用的全部 Existing workspace；空 query 不展示它们，
    /// 但用户开始输入后必须与 Project/Recent 一起参与过滤。
    private var rootExistingChoices: [ExistingConnectionChoice] = []
    /// Discovery 成功返回空数组也是一个完成状态；否则每次 reload 都会
    /// 重新发起请求，形成空结果下的 discovery 循环。
    private var rootExistingLoaded = false
    private var rootExistingLoading = false
    /// `@alias` 补全使用全部 SSH config alias，而不仅是当前有 workspace 的 host。
    private var sshAliases: [String] = []
    private var sshAliasesLoaded = false
    private var sshAliasesLoading = false
    private var workspaceNavigation = UnifiedWorkspaceNavigation.root
    private var existingRequestGeneration: UInt64 = 0
    private var hits: [SearchHit] = []
    private var rows: [AttentionRow] = []
    private var model = PanelModel.open(.workspaces)
    private var accessoryHeightConstraint: NSLayoutConstraint?
    private var keyMonitor: Any?
    private weak var ownerWindow: NSWindow?
    private let snapshot: () -> AttentionSnapshot?
    private let paneOutput: (UInt32) -> Data
    private let sendInput: (UInt32, Data) -> Void
    private let search: (String, SearchScope) -> [SearchHit]
    private let workspaceIndex: (TargetConfig) -> Int?
    /// MainWindow supplies every currently pooled Workspace so direct panel
    /// search does not depend on a separate Existing discovery round trip.
    private let connectedWorkspaces: (() -> [TargetConfig])?

    init(
        store: QuickConnectStore,
        ownerWindow: NSWindow?,
        snapshot: @escaping () -> AttentionSnapshot?,
        paneOutput: @escaping (UInt32) -> Data,
        sendInput: @escaping (UInt32, Data) -> Void,
        search: @escaping (String, SearchScope) -> [SearchHit],
        workspaceIndex: @escaping (TargetConfig) -> Int? = { _ in nil },
        connectedWorkspaces: (() -> [TargetConfig])? = nil
    ) {
        self.store = store
        self.ownerWindow = ownerWindow
        self.snapshot = snapshot
        self.paneOutput = paneOutput
        self.sendInput = sendInput
        self.search = search
        self.workspaceIndex = workspaceIndex
        self.connectedWorkspaces = connectedWorkspaces

        let panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: Self.preferredContentSize),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.title = MuxtermI18n.shared.tr(.quickConnect)
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.hasShadow = true

        super.init(window: panel)
        buildView()
        installKeyMonitor()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    deinit {
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
    }

    // MARK: - 打开 / 关闭

    func present(initial: PanelTab = .workspaces, scope: SearchScope = .workspace) {
        model.tab = initial
        // Linux `PanelModel::open(initial)` 语义：重新打开时 query 清空，
        // query 只在本次打开期间跨 tab 保留。
        model.query = ""
        model.scope = scope
        input.stringValue = ""
        workspaceNavigation = .root
        existingItems = []
        rootExistingChoices = []
        rootExistingLoaded = false
        rootExistingLoading = false
        existingRequestGeneration &+= 1
        reload()
        loadSSHAliasesIfNeeded()
        guard let window else { return }
        window.level = .floating
        CompactPanelLayout.prepare(window, owner: ownerWindow, preferred: Self.preferredContentSize)
        CompactPanelLayout.bringForward(window)
        applyTab()
        window.layoutIfNeeded()
        QuickConnectTableLayout.fit(table)
        window.makeFirstResponder(input)
    }

    /// 显示指定 tab。面板已打开时只切换内容，不把 Cmd-P/R/F 解释成 toggle。
    /// 这样连续按快捷键始终是可预测的导航动作；query 在 tab 之间保留。
    func show(tab: PanelTab, scope: SearchScope? = nil) {
        if window?.isVisible == true {
            if tab == .workspaces, workspaceNavigation != .root {
                workspaceNavigation = .root
                existingItems = []
                rootExistingChoices = []
                rootExistingLoaded = false
                rootExistingLoading = false
                existingRequestGeneration &+= 1
            }
            model.tab = tab
            if let scope {
                model.scope = scope
            }
            applyTab()
            reload()
            window?.makeKeyAndOrderFront(nil)
            window?.makeFirstResponder(input)
        } else {
            present(initial: tab, scope: scope ?? model.scope)
        }
    }

    /// 重新读取面板数据但不改变当前 tab、query 或 scope。
    func refreshData() {
        guard window?.isVisible == true else { return }
        reload()
    }

    func dismiss() {
        existingRequestGeneration &+= 1
        window?.orderOut(nil)
        ownerWindow?.makeKeyAndOrderFront(nil)
        onDismissed?()
    }

    // MARK: - 数据

    private func reload() {
        if model.tab == .workspaces, workspaceNavigation == .root {
            ensureRootExistingChoicesIfNeeded()
        }
        if model.tab == .workspaces || allItems.isEmpty {
            loadWorkspaceItems()
        }
        if PanelReloadPolicy.needsAttentionSnapshot(model.tab) {
            rows = snapshot().map { AttentionList.rows(from: $0, query: model.query) } ?? []
        } else {
            rows = []
        }
        if PanelReloadPolicy.needsSearch(
            model.tab,
            queryIsEmpty: model.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        ) {
            hits = search(model.query, model.scope)
        } else {
            hits = []
        }
        if model.tab == .workspaces {
            applyFilter()
        }
        table.reloadData()
        let rowCount = numberOfRows(in: table)
        if rowCount > 0 {
            let row = defaultSelectedRow()
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            table.scrollRowToVisible(row)
        } else {
            table.deselectAll(nil)
        }
        updatePeek()
        updateEmptyState()
        QuickConnectTableLayout.fit(table)
    }

    private func loadWorkspaceItems() {
        if workspaceNavigation == .existingConnections {
            allItems = existingItems
            return
        }
        if let connectedWorkspaces {
            store.replaceAllRecents(connectedWorkspaces())
        }
        let currentId = currentConfig.map { QuickConnect.uniqueID(for: $0) }
        allItems = [.existingConnections]
        let queryIsNonEmpty = !model.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let entries = QuickConnect.entries(
            recents: store.recents,
            projects: store.projects,
            recentLimit: queryIsNonEmpty ? QuickConnectStore.maxRecent : 5
        )
        allItems.append(contentsOf: entries.map { entry in
            .target(
                entry.config,
                badges: entry.badges,
                isCurrent: currentId == QuickConnect.uniqueID(for: entry.config),
                workspaceIndex: workspaceIndex(entry.config)
            )
        })
        if queryIsNonEmpty {
            var seen = Set(entries.map { QuickConnect.uniqueID(for: $0.config) })
            for choice in rootExistingChoices {
                guard seen.insert(QuickConnect.uniqueID(for: choice.config)).inserted else {
                    continue
                }
                allItems.append(.existing(choice))
            }
            if rootExistingLoading, rootExistingChoices.isEmpty {
                allItems.append(.loading)
            }
        }
        allItems.append(.newProject)
    }

    private func applyFilter() {
        let query = model.query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else {
            visibleItems = allItems
            return
        }
        visibleItems = allItems.enumerated()
            .compactMap { index, item in
                item.score(for: query).map { (index, $0, item) }
            }
            .sorted {
                if $0.1 != $1.1 { return $0.1 > $1.1 }
                return $0.0 < $1.0
            }
            .map { $0.2 }
    }

    private func defaultSelectedRow() -> Int {
        guard model.tab == .workspaces else { return 0 }
        return visibleItems.firstIndex { item in
            if case .target(_, _, let isCurrent, _) = item {
                return isCurrent
            }
            return false
        } ?? 0
    }

    /// 对 Existing 结果统一排序并按 attach identity 去重。
    private func sortedExistingChoices(
        _ choices: [ExistingConnectionChoice]
    ) -> [ExistingConnectionChoice] {
        var seen = Set<String>()
        return choices.sorted {
            if $0.target.displayName != $1.target.displayName {
                return $0.target.displayName.localizedCaseInsensitiveCompare(
                    $1.target.displayName
                ) == .orderedAscending
            }
            if $0.config.runtime != $1.config.runtime {
                return $0.config.runtime.rawValue.localizedCaseInsensitiveCompare(
                    $1.config.runtime.rawValue
                ) == .orderedAscending
            }
            return $0.config.name.localizedCaseInsensitiveCompare($1.config.name)
                == .orderedAscending
        }.filter { seen.insert(QuickConnect.uniqueID(for: $0.config)).inserted }
    }

    /// 根目录只在真正需要搜索时启动 Existing discovery；结果回来后仍留在
    /// 根目录时立即刷新，因而慢 SSH 不会阻塞输入，也不会漏掉新结果。
    private func ensureRootExistingChoicesIfNeeded() {
        let queryIsNonEmpty = !model.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        guard model.tab == .workspaces,
              workspaceNavigation == .root,
              queryIsNonEmpty,
              !rootExistingLoaded,
              !rootExistingLoading
        else { return }

        rootExistingLoading = true
        existingRequestGeneration &+= 1
        let generation = existingRequestGeneration
        guard let loader = onLoadExistingConnections else {
            rootExistingLoading = false
            rootExistingLoaded = true
            return
        }
        loader { [weak self] result in
            let apply = { [weak self] in
                guard let self, self.existingRequestGeneration == generation else { return }
                self.rootExistingLoading = false
                self.rootExistingLoaded = true
                switch result {
                case .success(let choices):
                    self.rootExistingChoices = self.sortedExistingChoices(choices)
                case .failure(let error):
                    self.rootExistingChoices = []
                    self.onExistingConnectionsError?(error)
                }
                guard self.window?.isVisible == true,
                      self.workspaceNavigation == .root,
                      self.model.tab == .workspaces
                else { return }
                self.reload()
            }
            if Thread.isMainThread {
                apply()
            } else {
                DispatchQueue.main.async(execute: apply)
            }
        }
    }

    /// 预加载 SSH alias，确保输入单个 `@` 时也能提示“当前没有 workspace
    /// 的 host”。失败时静默保留运行时补全，不弹打扰式错误。
    private func loadSSHAliasesIfNeeded() {
        guard !sshAliasesLoaded, !sshAliasesLoading else { return }
        sshAliasesLoading = true
        guard let loader = onLoadSSHAliases else {
            sshAliasesLoading = false
            sshAliasesLoaded = true
            return
        }
        loader { [weak self] result in
            let apply = { [weak self] in
                guard let self else { return }
                self.sshAliasesLoading = false
                self.sshAliasesLoaded = true
                if case .success(let aliases) = result {
                    self.sshAliases = aliases
                        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                        .filter { !$0.isEmpty }
                        .reduce(into: []) { values, alias in
                            if !values.contains(where: {
                                $0.caseInsensitiveCompare(alias) == .orderedSame
                            }) {
                                values.append(alias)
                            }
                        }
                }
            }
            if Thread.isMainThread {
                apply()
            } else {
                DispatchQueue.main.async(execute: apply)
            }
        }
    }

    private func openExistingConnections() {
        workspaceNavigation = .existingConnections
        model.query = ""
        input.stringValue = ""
        rootExistingLoading = false
        existingRequestGeneration &+= 1
        let generation = existingRequestGeneration
        existingItems = [.back, .loading]
        reload()

        guard let loader = onLoadExistingConnections else {
            existingItems = [.back, .empty]
            reload()
            return
        }
        loader { [weak self] result in
            let apply = { [weak self] in
                guard let self,
                      self.existingRequestGeneration == generation,
                      self.workspaceNavigation == .existingConnections,
                      self.window?.isVisible == true
                else { return }
                switch result {
                case .success(let choices):
                    let sorted = self.sortedExistingChoices(choices)
                    self.rootExistingChoices = sorted
                    self.rootExistingLoaded = true
                    self.existingItems = [.back]
                    self.existingItems.append(contentsOf: sorted.map(UnifiedWorkspaceItem.existing))
                    if sorted.isEmpty {
                        self.existingItems.append(.empty)
                    }
                case .failure(let error):
                    self.rootExistingChoices = []
                    self.rootExistingLoaded = true
                    self.existingItems = [.back, .empty]
                    self.onExistingConnectionsError?(error)
                }
                self.reload()
            }
            if Thread.isMainThread {
                apply()
            } else {
                DispatchQueue.main.async(execute: apply)
            }
        }
    }

    private func returnToWorkspaceRoot() {
        existingRequestGeneration &+= 1
        rootExistingLoading = false
        rootExistingChoices = []
        rootExistingLoaded = false
        workspaceNavigation = .root
        model.query = ""
        input.stringValue = ""
        reload()
    }

    private func selectRow(offset: Int) {
        let count = table.numberOfRows
        guard count > 0 else { return }
        let current = table.selectedRow >= 0 ? table.selectedRow : 0
        let next = ((current + offset) % count + count) % count
        table.selectRowIndexes(IndexSet(integer: next), byExtendingSelection: false)
        table.scrollRowToVisible(next)
    }

    private func activateSelected() {
        guard table.selectedRow >= 0 else { return }
        switch model.tab {
        case .workspaces:
            guard table.selectedRow < visibleItems.count else { return }
            switch visibleItems[table.selectedRow] {
            case .existingConnections:
                openExistingConnections()
            case .target(let config, _, _, _):
                onConnect?(config)
            case .newProject:
                onNewProject?()
            case .back:
                returnToWorkspaceRoot()
            case .existing(let choice):
                onAttachExistingConnection?(choice)
            case .loading, .empty:
                break
            }
        case .attention:
            guard table.selectedRow < rows.count else { return }
            let row = rows[table.selectedRow]
            onAcknowledge?(row.workspaceId, row.pane.paneId)
            onJump?(row.workspaceId, nil, row.pane.paneId, 0, "")
            dismiss()
        case .search:
            guard table.selectedRow < hits.count else { return }
            let hit = hits[table.selectedRow]
            onJump?(hit.workspaceId, hit.tabId, hit.paneId, hit.seq, model.query)
            dismiss()
        }
    }

    // MARK: - 视图

    private func buildView() {
        guard let window, let content = window.contentView else { return }

        let root = NSView()
        root.translatesAutoresizingMaskIntoConstraints = false
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        content.addSubview(root)

        buildTabControl()
        root.addSubview(tabControl)

        input.translatesAutoresizingMaskIntoConstraints = false
        input.font = NSFont.systemFont(ofSize: 14)
        input.controlSize = .large
        input.placeholderString = MuxtermI18n.shared.tr(.quickConnect)
        input.focusRingType = .none
        input.delegate = self
        // 统一 identifier + 旧面板别名（AX 子视图标签，旧测试按名字查找）。
        input.setAccessibilityIdentifier("muxterm.panel.input")
        let qcAlias = aliasLabel("muxterm.quickConnect.input")
        let searchAlias = aliasLabel("muxterm.search.input")
        let attentionAlias = aliasLabel("muxterm.attention.input")
        root.addSubview(input)
        root.addSubview(qcAlias)
        root.addSubview(searchAlias)
        root.addSubview(attentionAlias)
        NSLayoutConstraint.activate([
            qcAlias.leadingAnchor.constraint(equalTo: input.leadingAnchor),
            qcAlias.trailingAnchor.constraint(equalTo: input.trailingAnchor),
            qcAlias.topAnchor.constraint(equalTo: input.topAnchor),
            qcAlias.bottomAnchor.constraint(equalTo: input.bottomAnchor),
            searchAlias.leadingAnchor.constraint(equalTo: input.leadingAnchor),
            searchAlias.trailingAnchor.constraint(equalTo: input.trailingAnchor),
            searchAlias.topAnchor.constraint(equalTo: input.topAnchor),
            searchAlias.bottomAnchor.constraint(equalTo: input.bottomAnchor),
            attentionAlias.leadingAnchor.constraint(equalTo: input.leadingAnchor),
            attentionAlias.trailingAnchor.constraint(equalTo: input.trailingAnchor),
            attentionAlias.topAnchor.constraint(equalTo: input.topAnchor),
            attentionAlias.bottomAnchor.constraint(equalTo: input.bottomAnchor),
        ])

        buildSearchScopeBar()
        buildAttentionActions()
        accessoryContainer.translatesAutoresizingMaskIntoConstraints = false
        accessoryContainer.addSubview(scopeBar)
        accessoryContainer.addSubview(attentionActions)
        root.addSubview(accessoryContainer)
        NSLayoutConstraint.activate([
            scopeBar.leadingAnchor.constraint(equalTo: accessoryContainer.leadingAnchor),
            scopeBar.centerYAnchor.constraint(equalTo: accessoryContainer.centerYAnchor),
            attentionActions.leadingAnchor.constraint(equalTo: accessoryContainer.leadingAnchor),
            attentionActions.centerYAnchor.constraint(equalTo: accessoryContainer.centerYAnchor),
        ])

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("panel"))
        table.addTableColumn(column)
        QuickConnectTableLayout.configure(table, column: column)
        table.headerView = nil
        table.rowHeight = QuickTargetCellView.preferredRowHeight
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.usesAlternatingRowBackgroundColors = false
        table.backgroundColor = .clear
        table.selectionHighlightStyle = .regular
        table.style = .plain
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(tableActivated)
        table.doubleAction = #selector(tableDoubleActivated)
        table.setAccessibilityIdentifier("muxterm.panel.list")

        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.documentView = table
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = false
        scrollView.borderType = .noBorder
        root.addSubview(scrollView)

        emptyLabel.translatesAutoresizingMaskIntoConstraints = false
        emptyLabel.font = NSFont.systemFont(ofSize: 13)
        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.alignment = .center
        emptyLabel.setAccessibilityIdentifier("muxterm.panel.empty")
        emptyLabel.isHidden = true
        root.addSubview(emptyLabel)

        // 旧面板 identifier 别名：放在 scrollView 内部，与 scrollView 同祖先。
        let listAliases = [
            "muxterm.quickConnect.list",
            "muxterm.search.list",
            "muxterm.attention.list",
        ]
        for (i, alias) in listAliases.enumerated() {
            let label = aliasLabel(alias)
            scrollView.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: scrollView.leadingAnchor),
                label.trailingAnchor.constraint(equalTo: scrollView.trailingAnchor),
                label.topAnchor.constraint(equalTo: scrollView.topAnchor),
                label.heightAnchor.constraint(equalToConstant: 8 + CGFloat(i)),
            ])
        }

        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),

            tabControl.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 14),
            tabControl.topAnchor.constraint(equalTo: root.topAnchor, constant: 10),

            input.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 14),
            input.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -14),
            input.topAnchor.constraint(equalTo: tabControl.bottomAnchor, constant: 8),
            input.heightAnchor.constraint(equalToConstant: 28),

            scrollView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            accessoryContainer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 14),
            accessoryContainer.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -14),
            accessoryContainer.topAnchor.constraint(equalTo: input.bottomAnchor, constant: 4),

            scrollView.topAnchor.constraint(equalTo: accessoryContainer.bottomAnchor, constant: 4),
            scrollView.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -6),

            emptyLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            emptyLabel.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            emptyLabel.centerYAnchor.constraint(equalTo: scrollView.centerYAnchor),
        ])
        let accessoryHeight = accessoryContainer.heightAnchor.constraint(equalToConstant: 0)
        accessoryHeight.isActive = true
        accessoryHeightConstraint = accessoryHeight
    }

    private func buildTabControl() {
        tabControl.translatesAutoresizingMaskIntoConstraints = false
        tabControl.segmentCount = PanelTab.allCases.count
        tabControl.trackingMode = .selectOne
        tabControl.segmentStyle = .rounded
        tabControl.controlSize = .small
        tabControl.target = self
        tabControl.action = #selector(tabClicked(_:))
        tabControl.setAccessibilityIdentifier("muxterm.panel.tabs")
        tabControl.setLabel(MuxtermI18n.shared.tr(.panelWorkspaces), forSegment: 0)
        tabControl.setLabel(MuxtermI18n.shared.tr(.panelAttention), forSegment: 1)
        tabControl.setLabel(MuxtermI18n.shared.tr(.panelSearch), forSegment: 2)
        for id in [
            "muxterm.panel.tab.workspaces",
            "muxterm-panel-tab-workspaces",
            "muxterm.panel.tab.attention",
            "muxterm-panel-tab-attention",
            "muxterm.panel.tab.search",
            "muxterm-panel-tab-search",
        ] {
            addAlias(id, to: tabControl)
        }
    }

    private func buildSearchScopeBar() {
        scopeBar.translatesAutoresizingMaskIntoConstraints = false
        scopeBar.orientation = .horizontal
        scopeBar.alignment = .centerY
        scopeBar.spacing = 0
        scopeControl.segmentCount = SearchScope.allCases.count
        scopeControl.trackingMode = .selectOne
        scopeControl.segmentStyle = .rounded
        scopeControl.controlSize = .small
        scopeControl.target = self
        scopeControl.action = #selector(searchScopeClicked(_:))
        scopeControl.setAccessibilityIdentifier("muxterm.search.scope")
        scopeControl.setLabel(MuxtermI18n.shared.tr(.searchScopePane), forSegment: 0)
        scopeControl.setLabel(MuxtermI18n.shared.tr(.searchScopeWorkspace), forSegment: 1)
        scopeControl.setLabel(MuxtermI18n.shared.tr(.searchScopeAll), forSegment: 2)
        scopeBar.addArrangedSubview(scopeControl)
        for id in [
            "muxterm.search.scope.pane",
            "muxterm.search.scope.workspace",
            "muxterm.search.scope.all",
        ] {
            addAlias(id, to: scopeControl)
        }
    }

    private func buildAttentionActions() {
        attentionActions.translatesAutoresizingMaskIntoConstraints = false
        attentionActions.orientation = .horizontal
        attentionActions.alignment = .centerY
        attentionActions.spacing = 8

        attentionJumpButton.target = self
        attentionJumpButton.action = #selector(jumpSelectedAttention)
        attentionJumpButton.controlSize = .small
        attentionJumpButton.bezelStyle = .rounded
        attentionJumpButton.title = MuxtermI18n.shared.tr(.attentionJump)
        attentionJumpButton.setAccessibilityIdentifier("muxterm.attention.jump")

        attentionOpenButton.target = self
        attentionOpenButton.action = #selector(openSelectedAttention)
        attentionOpenButton.controlSize = .small
        attentionOpenButton.bezelStyle = .rounded
        attentionOpenButton.title = MuxtermI18n.shared.tr(.attentionOpen)
        attentionOpenButton.setAccessibilityIdentifier("muxterm.attention.open")

        attentionMuteButton.controlSize = .small
        attentionMuteButton.setAccessibilityIdentifier("muxterm.attention.mute")
        attentionMuteButton.addItem(withTitle: MuxtermI18n.shared.tr(.attentionMute))
        for (title, seconds) in [
            ("5m", 300),
            ("10m", 600),
            ("30m", 1_800),
            ("1h", 3_600),
            ("4h", 14_400),
            ("24h", 86_400),
        ] {
            let item = NSMenuItem(
                title: title,
                action: #selector(muteMenuItemSelected(_:)),
                keyEquivalent: ""
            )
            item.target = self
            item.tag = seconds
            attentionMuteButton.menu?.addItem(item)
        }

        attentionActions.addArrangedSubview(attentionJumpButton)
        attentionActions.addArrangedSubview(attentionOpenButton)
        attentionActions.addArrangedSubview(attentionMuteButton)
    }

    private func aliasLabel(_ id: String) -> NSView {
        let view = PanelAccessibilityAliasView()
        view.translatesAutoresizingMaskIntoConstraints = false
        view.setAccessibilityIdentifier(id)
        view.setAccessibilityElement(true)
        return view
    }

    private func addAlias(_ id: String, to control: NSView) {
        let alias = PanelAccessibilityAliasButton()
        alias.translatesAutoresizingMaskIntoConstraints = false
        alias.setAccessibilityIdentifier(id)
        alias.setAccessibilityElement(true)
        selectionAliases[id] = alias
        control.addSubview(alias)
        NSLayoutConstraint.activate([
            alias.leadingAnchor.constraint(equalTo: control.leadingAnchor),
            alias.trailingAnchor.constraint(equalTo: control.trailingAnchor),
            alias.topAnchor.constraint(equalTo: control.topAnchor),
            alias.bottomAnchor.constraint(equalTo: control.bottomAnchor),
        ])
    }

    @objc private func tabClicked(_ sender: NSSegmentedControl) {
        guard let tab = PanelTab(rawValue: sender.selectedSegment) else { return }
        model.tab = tab
        applyTab()
        reload()
    }

    private func applyTab() {
        tabControl.selectedSegment = model.tab.rawValue
        scopeControl.selectedSegment = scopeSegment(model.scope)
        updateSelectionAliases()
        let showsScope = model.tab == .search
        let showsAttentionActions = model.tab == .attention
        scopeBar.isHidden = !showsScope
        attentionActions.isHidden = !showsAttentionActions
        accessoryHeightConstraint?.constant = (showsScope || showsAttentionActions) ? 24 : 0
        input.placeholderString = placeholder(for: model.tab)
        updatePeek()
        updateEmptyState()
    }

    @objc private func searchScopeClicked(_ sender: NSSegmentedControl) {
        let scope: SearchScope
        switch sender.selectedSegment {
        case 0: scope = .pane
        case 1: scope = .workspace
        default: scope = .all
        }
        guard model.scope != scope else { return }
        model.scope = scope
        applyTab()
        reload()
    }

    private func scopeSegment(_ scope: SearchScope) -> Int {
        switch scope {
        case .pane: return 0
        case .workspace: return 1
        case .all: return 2
        }
    }

    private func placeholder(for tab: PanelTab) -> String {
        switch tab {
        case .workspaces: return MuxtermI18n.shared.tr(.quickConnect)
        case .attention: return MuxtermI18n.shared.tr(.panelAttentionPlaceholder)
        case .search: return MuxtermI18n.shared.tr(.panelSearchPlaceholder)
        }
    }

    func refreshLocalization() {
        window?.title = MuxtermI18n.shared.tr(.quickConnect)
        tabControl.setLabel(MuxtermI18n.shared.tr(.panelWorkspaces), forSegment: 0)
        tabControl.setLabel(MuxtermI18n.shared.tr(.panelAttention), forSegment: 1)
        tabControl.setLabel(MuxtermI18n.shared.tr(.panelSearch), forSegment: 2)
        scopeControl.setLabel(MuxtermI18n.shared.tr(.searchScopePane), forSegment: 0)
        scopeControl.setLabel(MuxtermI18n.shared.tr(.searchScopeWorkspace), forSegment: 1)
        scopeControl.setLabel(MuxtermI18n.shared.tr(.searchScopeAll), forSegment: 2)
        attentionJumpButton.title = MuxtermI18n.shared.tr(.attentionJump)
        attentionOpenButton.title = MuxtermI18n.shared.tr(.attentionOpen)
        attentionMuteButton.item(at: 0)?.title = MuxtermI18n.shared.tr(.attentionMute)
        applyTab()
        table.reloadData()
    }

    private func updateEmptyState() {
        let isEmpty = numberOfRows(in: table) == 0
        let key: MuxtermTextKey
        switch model.tab {
        case .workspaces:
            key = .panelNoWorkspaces
        case .attention:
            key = .panelNoAttention
        case .search:
            key = model.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? .panelSearchPrompt
                : .panelNoResults
        }
        emptyLabel.stringValue = MuxtermI18n.shared.tr(key)
        emptyLabel.isHidden = !isEmpty
        scrollView.isHidden = isEmpty
    }

    /// 旧版 E2E 和辅助功能脚本会按每个选项的 AX identifier 查询状态。
    /// 原生 NSSegmentedControl 只有一个控件节点，因此保留不可点击的轻量按钮别名。
    private func updateSelectionAliases() {
        let tabAliases: [[String]] = [
            ["muxterm.panel.tab.workspaces", "muxterm-panel-tab-workspaces"],
            ["muxterm.panel.tab.attention", "muxterm-panel-tab-attention"],
            ["muxterm.panel.tab.search", "muxterm-panel-tab-search"],
        ]
        for (index, ids) in tabAliases.enumerated() {
            for id in ids {
                selectionAliases[id]?.state = model.tab.rawValue == index ? .on : .off
            }
        }

        let selectedScope = scopeSegment(model.scope)
        for (index, id) in [
            "muxterm.search.scope.pane",
            "muxterm.search.scope.workspace",
            "muxterm.search.scope.all",
        ].enumerated() {
            selectionAliases[id]?.state = selectedScope == index ? .on : .off
        }
    }

    private func installKeyMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.window?.isKeyWindow == true else { return event }
            switch event.keyCode {
            case 53: // Escape
                if self.model.tab == .workspaces,
                   self.workspaceNavigation == .existingConnections
                {
                    self.returnToWorkspaceRoot()
                } else {
                    self.dismiss()
                }
                return nil
            case 48: // Tab
                if self.completeCurrentWorkspaceToken() {
                    return nil
                }
                self.model.cycleTab(back: event.modifierFlags.contains(.shift))
                self.applyTab()
                self.reload()
                return nil
            case 125: // Down
                self.selectRow(offset: 1)
                return nil
            case 126: // Up
                self.selectRow(offset: -1)
                return nil
            case 36, 76: // Return / keypad Enter
                self.activateSelected()
                return nil
            default:
                return event
            }
        }
    }

    // MARK: - 输入

    func controlTextDidChange(_ obj: Notification) {
        model.query = input.stringValue
        loadSSHAliasesIfNeeded()
        reload()
    }

    /// NSTextField 原生 completion；Tab 事件同时走同一条替换逻辑，避免
    /// 面板的 tab 循环吞掉 `@` 补全。
    func control(
        _ control: NSControl,
        textView: NSTextView,
        completions words: [String],
        forPartialWordRange charRange: NSRange,
        indexOfSelectedItem index: UnsafeMutablePointer<Int>
    ) -> [String] {
        guard control === input, model.tab == .workspaces else { return [] }
        index.pointee = 0
        return WorkspaceQuery.completionCandidates(
            for: input.stringValue,
            sshAliases: sshAliases
        )
    }

    private func completeCurrentWorkspaceToken() -> Bool {
        guard model.tab == .workspaces,
              let replacement = WorkspaceQuery.completionCandidates(
                  for: input.stringValue,
                  sshAliases: sshAliases
              ).first
        else { return false }
        let completed = WorkspaceQuery.replaceCurrentToken(
            in: input.stringValue,
            with: replacement
        )
        // 已经是完整候选时把 Tab 留给面板切换；否则 `@tmux` 会被
        // 每次按 Tab 原样替换，永远无法切到 Attention/Search。
        guard completed != input.stringValue else { return false }
        input.stringValue = completed
        model.query = completed
        if let editor = input.currentEditor() as? NSTextView {
            let end = (completed as NSString).length
            editor.setSelectedRange(NSRange(location: end, length: 0))
        }
        reload()
        return true
    }

    // MARK: - 表格

    func numberOfRows(in tableView: NSTableView) -> Int {
        switch model.tab {
        case .workspaces: return visibleItems.count
        case .attention: return rows.count
        case .search: return hits.count
        }
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        switch model.tab {
        case .workspaces:
            guard row < visibleItems.count else { return nil }
            let item = visibleItems[row]
            switch item {
            case .existingConnections:
                let id = NSUserInterfaceItemIdentifier("ExistingConnectionsFolder")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickActionCellView
                    ?? QuickActionCellView(identifier: id)
                cell.title = "› " + item.title
                cell.setAccessibilityIdentifier("muxterm.quickConnect.existingConnections")
                return cell
            case .target(let config, let badges, let isCurrent, let workspaceIndex):
                let id = NSUserInterfaceItemIdentifier("QuickTarget")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickTargetCellView
                    ?? QuickTargetCellView(identifier: id)
                cell.config = config
                cell.badges = badges
                cell.isCurrent = isCurrent
                cell.workspaceIndex = workspaceIndex
                return cell
            case .newProject:
                let id = NSUserInterfaceItemIdentifier("QuickNew")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickActionCellView
                    ?? QuickActionCellView(identifier: id)
                cell.title = "＋ " + MuxtermI18n.shared.tr(.panelNewProject)
                return cell
            case .back:
                let id = NSUserInterfaceItemIdentifier("ExistingConnectionsBack")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickActionCellView
                    ?? QuickActionCellView(identifier: id)
                cell.title = "‹ " + item.title
                cell.setAccessibilityIdentifier("muxterm.quickConnect.existingBack")
                return cell
            case .existing(let choice):
                let id = NSUserInterfaceItemIdentifier("ExistingConnection")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? ExistingConnectionCellView
                    ?? ExistingConnectionCellView(identifier: id)
                cell.title = choice.config.name
                var details = [choice.target.displayName, choice.config.runtime.rawValue]
                if let windowCount = choice.windowCount {
                    details.append(MuxtermI18n.shared.tr(
                        .tmuxWindows,
                        arguments: ["count": "\(windowCount)"]
                    ))
                    if choice.attached == true {
                        details.append(MuxtermI18n.shared.tr(.tmuxAttached))
                    }
                } else {
                    if let session = choice.config.session {
                        details.append(session)
                    }
                    if let workspaceID = choice.config.workspaceID {
                        details.append(workspaceID)
                    }
                }
                cell.detail = details.joined(separator: " · ")
                cell.setAccessibilityIdentifier(
                    "muxterm.quickConnect.existing.\(choice.target.displayName)."
                        + "\(choice.config.runtime.rawValue).\(choice.config.session ?? "")."
                        + "\(choice.config.workspaceID ?? choice.config.name)"
                )
                return cell
            case .loading, .empty:
                let id = NSUserInterfaceItemIdentifier("ExistingConnectionsStatus")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickActionCellView
                    ?? QuickActionCellView(identifier: id)
                cell.title = item.title
                return cell
            }
        case .attention:
            guard row < rows.count else { return nil }
            let row = rows[row]
            let id = NSUserInterfaceItemIdentifier("AttentionRow")
            let cell = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView
                ?? NSTableCellView()
            cell.identifier = id
            let label = cell.textField ?? {
                let l = NSTextField(labelWithString: "")
                l.translatesAutoresizingMaskIntoConstraints = false
                cell.addSubview(l)
                cell.textField = l
                NSLayoutConstraint.activate([
                    l.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 12),
                    l.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -12),
                    l.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                ])
                return l
            }()
            label.stringValue = "● " + row.title
            label.textColor = row.pane.status == .working ? .systemGreen : .systemOrange
            label.font = NSFont.systemFont(ofSize: 12)
            label.maximumNumberOfLines = 1
            cell.setAccessibilityIdentifier("muxterm.attention.hit-\(row)")
            return cell
        case .search:
            guard row < hits.count else { return nil }
            let hit = hits[row]
            let id = NSUserInterfaceItemIdentifier("SearchHit")
            let cell = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView
                ?? NSTableCellView()
            cell.identifier = id
            let label = cell.textField ?? {
                let l = NSTextField(labelWithString: "")
                l.translatesAutoresizingMaskIntoConstraints = false
                cell.addSubview(l)
                cell.textField = l
                NSLayoutConstraint.activate([
                    l.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 12),
                    l.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -12),
                    l.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                ])
                return l
            }()
            label.stringValue = "\(hit.workspaceId)  \(MuxtermI18n.shared.tr(.tab)) \(hit.tabId)  \(MuxtermI18n.shared.tr(.pane)) @\(hit.paneId)\n\(hit.line)"
            label.font = NSFont.systemFont(ofSize: 11.5)
            label.maximumNumberOfLines = 2
            cell.setAccessibilityIdentifier("muxterm.search.hit-\(row)")
            return cell
        }
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        updatePeek()
    }

    @objc private func tableActivated() {
        activateSelected()
    }

    @objc private func tableDoubleActivated() {
        let row = table.clickedRow
        guard row >= 0, model.tab == .workspaces, row < visibleItems.count else {
            activateSelected()
            return
        }
        switch visibleItems[row] {
        case .target(let config, _, _, _):
            onEditProject?(config)
        default:
            activateSelected()
        }
    }

    // MARK: - Attention 动作（W19：无内嵌 peek，预览走独立 overlay）

    private func updatePeek() {
        let hasSelection = model.tab == .attention
            && table.selectedRow >= 0
            && table.selectedRow < rows.count
        attentionJumpButton.isEnabled = hasSelection
        attentionOpenButton.isEnabled = hasSelection
        attentionMuteButton.isEnabled = hasSelection
    }

    @objc private func jumpSelectedAttention() {
        guard let row = selectedAttentionRow() else { return }
        onAcknowledge?(row.workspaceId, row.pane.paneId)
        onJump?(row.workspaceId, nil, row.pane.paneId, 0, "")
        dismiss()
    }

    @objc private func openSelectedAttention() {
        guard let row = selectedAttentionRow() else { return }
        onAcknowledge?(row.workspaceId, row.pane.paneId)
        onPreview?(row.workspaceId, row.pane.paneId)
    }

    @objc private func muteMenuItemSelected(_ sender: NSMenuItem) {
        muteSelected(seconds: UInt64(sender.tag))
    }

    private func muteSelected(seconds: UInt64) {
        guard let row = selectedAttentionRow(), seconds > 0 else { return }
        onMute?(row.workspaceId, row.pane.paneId, seconds)
        reload()
    }

    private func selectedAttentionRow() -> AttentionRow? {
        guard model.tab == .attention,
              table.selectedRow >= 0,
              table.selectedRow < rows.count
        else {
            return nil
        }
        return rows[table.selectedRow]
    }

    // MARK: - 测试钩子

    func testIsPresented() -> Bool {
        window?.isVisible == true
    }

    /// 供 MainWindow.handleKey 转发（面板 key window 时）。
    func cycleTabForTest(back: Bool) {
        model.cycleTab(back: back)
        applyTab()
        reload()
        // 让按钮状态立即反映到 AX（headless 下 state 变化可能延迟）。
        window?.contentView?.layoutSubtreeIfNeeded()
    }

    func activateForTest() {
        activateSelected()
    }

    func testSetQuery(_ query: String) {
        input.stringValue = query
        model.query = query
        reload()
    }

    func testActivateFirstHit() {
        guard !hits.isEmpty else { return }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        activateSelected()
    }

    func testHitCount() -> Int {
        hits.count
    }

    func testSearchHitPaneIDs() -> [UInt32] {
        hits.map(\.paneId)
    }

    func testSearchHitWorkspaceIDs() -> [String] {
        hits.map(\.workspaceId)
    }

    func testSetSearchScope(_ scope: SearchScope) {
        model.scope = scope
        applyTab()
        reload()
    }

    func testRowCount() -> Int {
        rows.count
    }

    func testWorkspaceRowCount() -> Int {
        visibleItems.count
    }

    func testWorkspaceTitles() -> [String] {
        visibleItems.map(\.title)
    }

    func testActivateWorkspaceItem(matching title: String) {
        guard model.tab == .workspaces,
              let row = visibleItems.firstIndex(where: { $0.title == title })
        else { return }
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        activateSelected()
    }

    func testWorkspaceShowsExistingConnections() -> Bool {
        workspaceNavigation == .existingConnections
    }

    func testSelectFirstRow() {
        guard !rows.isEmpty else { return }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        table.window?.makeFirstResponder(table)
    }

    func testPeekView() -> NSView? {
        // W19-E：peek 已删除，永远返回 nil。
        nil
    }

    func testPeekText() -> String {
        ""
    }

    var modelTab: PanelTab { model.tab }

    func testSelectedAttentionRow() -> AttentionRow? {
        selectedAttentionRow()
    }

    func testOpenSelectedAttention() {
        openSelectedAttention()
    }

    func testMuteSelected(seconds: UInt64) {
        muteSelected(seconds: seconds)
    }

    func testSelectRow(offset: Int) {
        selectRow(offset: offset)
    }

    func testSelectedRow() -> Int {
        table.selectedRow
    }

    func testAttentionRowTitle(_ index: Int) -> String {
        table.layoutSubtreeIfNeeded()
        guard let cell = table.view(atColumn: 0, row: index, makeIfNecessary: true) as? NSTableCellView else {
            return ""
        }
        return cell.textField?.stringValue ?? ""
    }

    func testTableColumnWidth() -> CGFloat {
        table.enclosingScrollView?.tile()
        return table.tableColumns.first?.width ?? 0
    }

    func testWorkspaceCell(at row: Int) -> QuickTargetCellView? {
        guard row >= 0, row < table.numberOfRows else { return nil }
        table.layoutSubtreeIfNeeded()
        return table.view(atColumn: 0, row: row, makeIfNecessary: true) as? QuickTargetCellView
    }

    func testWorkspaceIndex(matching title: String) -> Int? {
        guard let row = visibleItems.firstIndex(where: { $0.title == title }),
              case .target(_, _, _, let index) = visibleItems[row]
        else { return nil }
        return index
    }

    func testSelectedWorkspaceTitle() -> String? {
        guard model.tab == .workspaces,
              visibleItems.indices.contains(table.selectedRow)
        else { return nil }
        return visibleItems[table.selectedRow].title
    }

    func testContentSize() -> NSSize {
        window?.contentView?.bounds.size ?? .zero
    }

    func testSearchFontSize() -> CGFloat {
        input.font?.pointSize ?? 0
    }

    func testRowHeight() -> CGFloat {
        table.rowHeight
    }

    func testEmptyStateVisible() -> Bool {
        !emptyLabel.isHidden
    }

    func testEmptyStateText() -> String {
        emptyLabel.stringValue
    }

    func testUsesSegmentedNavigation() -> Bool {
        tabControl.segmentCount == 3 && scopeControl.segmentCount == 3
    }
}

private final class ExistingConnectionCellView: NSTableCellView {
    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")

    var title: String {
        get { titleLabel.stringValue }
        set { titleLabel.stringValue = newValue }
    }

    var detail: String {
        get { detailLabel.stringValue }
        set { detailLabel.stringValue = newValue }
    }

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        titleLabel.lineBreakMode = .byTruncatingTail
        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = NSFont.systemFont(ofSize: 11)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.lineBreakMode = .byTruncatingTail
        addSubview(titleLabel)
        addSubview(detailLabel)
        textField = titleLabel
        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            titleLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 1),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }
}

extension UnifiedPanelController: TerminalInputHandler {
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        sendInput(view.paneId, Data(data))
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {
        // peek 小终端不写回 tmux 尺寸。
    }
}

private extension NSView {
    func findSubview(_ pred: (NSView) -> Bool) -> NSView? {
        if pred(self) { return self }
        for child in subviews {
            if let found = child.findSubview(pred) { return found }
        }
        return nil
    }
}

private final class PanelAccessibilityAliasView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }
}

private final class PanelAccessibilityAliasButton: NSButton {
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        title = ""
        isBordered = false
        isTransparent = true
    }

    convenience init() {
        self.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }
}
