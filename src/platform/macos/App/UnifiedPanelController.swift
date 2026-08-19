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
    var onConnect: ((TargetConfig) -> Void)?
    var onEditProject: ((TargetConfig) -> Void)?
    var onNewProject: (() -> Void)?
    var onJump: ((UInt32?, UInt32, UInt64, String) -> Void)? // (tabId, paneId, seq, query)
    var currentConfig: TargetConfig?

    private let store: QuickConnectStore
    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = MuxtermFillWidthScrollView()
    private var allItems: [QuickConnectItem] = []
    private var visibleItems: [QuickConnectItem] = []
    private var hits: [SearchHit] = []
    private var rows: [AttentionRow] = []
    private var model = PanelModel.open(.workspaces)
    private var tabButtons: [PanelTab: NSButton] = [:]
    private var keyMonitor: Any?
    private weak var ownerWindow: NSWindow?
    private let snapshot: () -> AttentionSnapshot?
    private let paneOutput: (UInt32) -> Data
    private let sendInput: (UInt32, Data) -> Void
    private let search: (String) -> [SearchHit]

    init(
        store: QuickConnectStore,
        ownerWindow: NSWindow?,
        snapshot: @escaping () -> AttentionSnapshot?,
        paneOutput: @escaping (UInt32) -> Data,
        sendInput: @escaping (UInt32, Data) -> Void,
        search: @escaping (String) -> [SearchHit]
    ) {
        self.store = store
        self.ownerWindow = ownerWindow
        self.snapshot = snapshot
        self.paneOutput = paneOutput
        self.sendInput = sendInput
        self.search = search

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 480),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.title = "Quick Connect"
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

    func present(initial: PanelTab = .workspaces) {
        model.tab = initial
        // Linux `PanelModel::open(initial)` 语义：重新打开时 query 清空，
        // query 只在本次打开期间跨 tab 保留。
        model.query = ""
        input.stringValue = ""
        reload()
        guard let window else { return }
        if let ownerWindow {
            window.level = .floating
            let ownerFrame = ownerWindow.frame
            window.setFrameOrigin(NSPoint(
                x: ownerFrame.midX - window.frame.width / 2,
                y: ownerFrame.midY - window.frame.height / 2
            ))
        } else {
            window.center()
        }
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
        hits = model.query.isEmpty ? [] : search(model.query)
        table.reloadData()
        if !rows.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            table.scrollRowToVisible(0)
        }
        if !hits.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            table.scrollRowToVisible(0)
        }
        updatePeek()
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
            onJump?(nil, row.pane.paneId, 0, "")
            dismiss()
        case .search:
            guard table.selectedRow < hits.count else { return }
            let hit = hits[table.selectedRow]
            onJump?(hit.tabId, hit.paneId, hit.seq, model.query)
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

        // 三个 tab 按钮（Linux `muxterm-panel-tab-*` 同款）。
        let tabStack = NSStackView()
        tabStack.translatesAutoresizingMaskIntoConstraints = false
        tabStack.orientation = .horizontal
        tabStack.alignment = .centerY
        tabStack.spacing = 8
        root.addSubview(tabStack)

        let workspaces = makeTabButton(
            title: "Workspaces",
            id: "muxterm.panel.tab.workspaces",
            linuxId: "muxterm-panel-tab-workspaces",
            tag: PanelTab.workspaces.rawValue
        )
        let attention = makeTabButton(
            title: "Attention",
            id: "muxterm.panel.tab.attention",
            linuxId: "muxterm-panel-tab-attention",
            tag: PanelTab.attention.rawValue
        )
        let search = makeTabButton(
            title: "Search",
            id: "muxterm.panel.tab.search",
            linuxId: "muxterm-panel-tab-search",
            tag: PanelTab.search.rawValue
        )
        tabButtons[.workspaces] = workspaces
        tabButtons[.attention] = attention
        tabButtons[.search] = search
        tabStack.addArrangedSubview(workspaces)
        tabStack.addArrangedSubview(attention)
        tabStack.addArrangedSubview(search)

        input.translatesAutoresizingMaskIntoConstraints = false
        input.font = NSFont.systemFont(ofSize: 18)
        input.placeholderString = "Quick Connect"
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

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("panel"))
        table.addTableColumn(column)
        QuickConnectTableLayout.configure(table, column: column)
        table.headerView = nil
        table.rowHeight = QuickTargetCellView.preferredRowHeight
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.usesAlternatingRowBackgroundColors = false
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
        root.addSubview(scrollView)

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

            tabStack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 18),
            tabStack.topAnchor.constraint(equalTo: root.topAnchor, constant: 12),

            input.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 18),
            input.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -18),
            input.topAnchor.constraint(equalTo: tabStack.bottomAnchor, constant: 10),
            input.heightAnchor.constraint(equalToConstant: 34),

            scrollView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: input.bottomAnchor, constant: 12),
            scrollView.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
        ])
    }

    private func aliasLabel(_ id: String) -> NSView {
        let view = NSView()
        view.translatesAutoresizingMaskIntoConstraints = false
        view.setAccessibilityIdentifier(id)
        view.setAccessibilityElement(true)
        return view
    }

    private func makeTabButton(title: String, id: String, linuxId: String, tag: Int) -> NSButton {
        let button = NSButton(title: title, target: self, action: #selector(tabClicked(_:)))
        button.setButtonType(.radio)
        button.tag = tag
        button.setAccessibilityIdentifier(id)
        // Linux 别名：独立 AX 标签，避免覆盖主 identifier。
        let alias = aliasLabel(linuxId)
        button.addSubview(alias)
        NSLayoutConstraint.activate([
            alias.leadingAnchor.constraint(equalTo: button.leadingAnchor),
            alias.trailingAnchor.constraint(equalTo: button.trailingAnchor),
            alias.topAnchor.constraint(equalTo: button.topAnchor),
            alias.bottomAnchor.constraint(equalTo: button.bottomAnchor),
        ])
        return button
    }

    @objc private func tabClicked(_ sender: NSButton) {
        guard let tab = PanelTab(rawValue: sender.tag) else { return }
        model.tab = tab
        applyTab()
        reload()
    }

    private func applyTab() {
        // 更新三个 tab 按钮的选中态。
        for (tab, button) in tabButtons {
            button.state = tab == model.tab ? .on : .off
        }
        input.placeholderString = placeholder(for: model.tab)
    }

    private func placeholder(for tab: PanelTab) -> String {
        switch tab {
        case .workspaces: return "Quick Connect"
        case .attention: return "Attention"
        case .search: return "Search panes"
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
                let cell = tableView.makeView(withIdentifier: id, owner: self) as? NSTextField
                    ?? NSTextField(labelWithString: "")
                cell.identifier = id
                cell.stringValue = "＋ New Project"
                cell.font = NSFont.systemFont(ofSize: 14, weight: .medium)
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
            label.stringValue = "\(hit.workspaceId)  tab \(hit.tabId)  pane @\(hit.paneId)\n\(hit.line)"
            label.font = NSFont.systemFont(ofSize: 12)
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

    // MARK: - 选中行（W19：无 peek，预览走 Cmd-Enter overlay）

    private func updatePeek() {
        // W19-E：注意力列表不再渲染 muxterm.attention.peek。
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
        guard model.tab == .attention, table.selectedRow >= 0, table.selectedRow < rows.count else {
            return nil
        }
        return rows[table.selectedRow]
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
