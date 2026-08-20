import AppKit
import MuxtermChrome

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
    var onEditProject: ((TargetConfig) -> Void)?
    var onNewProject: (() -> Void)?
    var onJump: ((String?, UInt32?, UInt32, UInt64, String) -> Void)?
    // (workspaceId, tabId, paneId, seq, query)
    var onPreview: ((String, UInt32) -> Void)? // (workspaceId, paneId)
    var onMute: ((String, UInt32, UInt64) -> Void)? // (workspaceId, paneId, seconds)
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
    private var allItems: [QuickConnectItem] = []
    private var visibleItems: [QuickConnectItem] = []
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

    init(
        store: QuickConnectStore,
        ownerWindow: NSWindow?,
        snapshot: @escaping () -> AttentionSnapshot?,
        paneOutput: @escaping (UInt32) -> Data,
        sendInput: @escaping (UInt32, Data) -> Void,
        search: @escaping (String, SearchScope) -> [SearchHit]
    ) {
        self.store = store
        self.ownerWindow = ownerWindow
        self.snapshot = snapshot
        self.paneOutput = paneOutput
        self.sendInput = sendInput
        self.search = search

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
        reload()
        guard let window else { return }
        window.level = .floating
        CompactPanelLayout.prepare(window, owner: ownerWindow, preferred: Self.preferredContentSize)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        applyTab()
        window.layoutIfNeeded()
        QuickConnectTableLayout.fit(table)
        window.makeFirstResponder(input)
    }

    func dismiss() {
        window?.orderOut(nil)
        ownerWindow?.makeKeyAndOrderFront(nil)
        if let ownerWindow, let first = ownerWindow.contentView {
            ownerWindow.makeFirstResponder(first)
        }
    }

    // MARK: - 数据

    private func reload() {
        let currentId = currentConfig.map { QuickConnect.uniqueID(for: $0) }
        allItems = QuickConnect.entries(
            recents: store.recents,
            projects: store.projects
        ).map { entry in
            .target(
                entry.config,
                badges: entry.badges,
                isCurrent: currentId == QuickConnect.uniqueID(for: entry.config)
            )
        }
        allItems.append(.newProject)
        applyFilter()
        rows = snapshot().map { AttentionList.rows(from: $0, query: model.query) } ?? []
        hits = model.query.isEmpty ? [] : search(model.query, model.scope)
        table.reloadData()
        let rowCount = numberOfRows(in: table)
        if rowCount > 0 {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            table.scrollRowToVisible(0)
        } else {
            table.deselectAll(nil)
        }
        updatePeek()
        updateEmptyState()
        QuickConnectTableLayout.fit(table)
    }

    private func applyFilter() {
        let query = model.query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        visibleItems = query.isEmpty
            ? allItems
            : allItems.filter { item in
                switch item {
                case .target(let c, _, _): return QuickConnect.searchText(for: c).contains(query)
                case .newProject: return "new project 新建".contains(query)
                }
            }
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
            case .target(let config, _, _):
                onConnect?(config)
            case .newProject:
                onNewProject?()
            }
        case .attention:
            guard table.selectedRow < rows.count else { return }
            let row = rows[table.selectedRow]
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
                self.dismiss()
                return nil
            case 48: // Tab
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
        reload()
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
            case .target(let config, let badges, let isCurrent):
                let id = NSUserInterfaceItemIdentifier("QuickTarget")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickTargetCellView
                    ?? QuickTargetCellView(identifier: id)
                cell.config = config
                cell.badges = badges
                cell.isCurrent = isCurrent
                return cell
            case .newProject:
                let id = NSUserInterfaceItemIdentifier("QuickNew")
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickActionCellView
                    ?? QuickActionCellView(identifier: id)
                cell.title = "＋ " + MuxtermI18n.shared.tr(.panelNewProject)
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
            let status = row.pane.status == .blocked ? "● " : "✓ "
            label.stringValue = status + row.title
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
        case .target(let config, _, _):
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
        onJump?(row.workspaceId, nil, row.pane.paneId, 0, "")
        dismiss()
    }

    @objc private func openSelectedAttention() {
        guard let row = selectedAttentionRow() else { return }
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
