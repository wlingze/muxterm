import AppKit
import MuxtermChrome

/// Native main-window sidebar mirroring Linux's two persistent sections.
final class WorkspaceSidebarView: NSView, NSTableViewDataSource, NSTableViewDelegate {
    var onWorkspaceActivate: ((String) -> Void)?
    var onAgentActivate: ((String, UInt32) -> Void)?

    private let sections = NSSplitView()
    private let workspaceTable = NSTableView()
    private let agentTable = NSTableView()
    private let workspaceScroll = NSScrollView()
    private let agentScroll = NSScrollView()
    private let workspaceHeader = NSButton()
    private let agentHeader = NSButton()
    private var workspaces: [WorkspaceSidebarItem] = []
    private var agents: [AgentSidebarItem] = []
    private var isReloadingSelection = false

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setAccessibilityIdentifier("muxterm.sidebar")
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor

        configureTable(
            workspaceTable,
            scroll: workspaceScroll,
            identifier: "muxterm.sidebar.workspaces"
        )
        configureTable(
            agentTable,
            scroll: agentScroll,
            identifier: "muxterm.sidebar.agents"
        )
        configureHeader(
            workspaceHeader,
            title: "WORKSPACES",
            action: #selector(toggleWorkspaceSection)
        )
        configureHeader(
            agentHeader,
            title: "AGENTS",
            action: #selector(toggleAgentSection)
        )

        let workspaceSection = section(header: workspaceHeader, scroll: workspaceScroll)
        workspaceSection.setAccessibilityIdentifier("muxterm.sidebar.workspaces.section")
        let agentSection = section(header: agentHeader, scroll: agentScroll)
        agentSection.setAccessibilityIdentifier("muxterm.sidebar.agents.section")

        sections.translatesAutoresizingMaskIntoConstraints = false
        sections.isVertical = false
        sections.dividerStyle = .thin
        sections.addArrangedSubview(workspaceSection)
        sections.addArrangedSubview(agentSection)
        sections.setAccessibilityIdentifier("muxterm.sidebar.sections")
        addSubview(sections)
        NSLayoutConstraint.activate([
            sections.leadingAnchor.constraint(equalTo: leadingAnchor),
            sections.trailingAnchor.constraint(equalTo: trailingAnchor),
            sections.topAnchor.constraint(equalTo: topAnchor),
            sections.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func layout() {
        super.layout()
        if sections.subviews.count == 2, sections.subviews[0].frame.height == 0 {
            sections.setPosition(max(120, bounds.height * 0.5), ofDividerAt: 0)
        }
    }

    func setWorkspaces(_ items: [WorkspaceSidebarItem]) {
        guard workspaces != items else { return }
        workspaces = items
        workspaceTable.reloadData()
        isReloadingSelection = true
        defer { isReloadingSelection = false }
        if let active = items.firstIndex(where: \.isActive) {
            workspaceTable.selectRowIndexes(IndexSet(integer: active), byExtendingSelection: false)
        } else {
            workspaceTable.deselectAll(nil)
        }
    }

    func setAgents(_ items: [AgentSidebarItem]) {
        guard agents != items else { return }
        agents = items
        agentTable.reloadData()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === workspaceTable ? workspaces.count : agents.count
    }

    func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        if tableView === workspaceTable {
            guard workspaces.indices.contains(row) else { return nil }
            let item = workspaces[row]
            let cell = sidebarCell(in: tableView, identifier: "WorkspaceSidebarCell")
            cell.set(
                marker: item.isActive ? "●" : "○",
                markerColor: item.isActive ? .controlAccentColor : .tertiaryLabelColor,
                title: item.name,
                detail: "\(item.runtime) @ \(item.transport)"
            )
            cell.setAccessibilityIdentifier("muxterm.sidebar.workspace.\(safeID(item.workspaceId))")
            return cell
        }

        guard agents.indices.contains(row) else { return nil }
        let item = agents[row]
        let cell = sidebarCell(in: tableView, identifier: "AgentSidebarCell")
        let color: NSColor
        switch item.indicator {
        case .running:
            color = .systemGreen
        case .done:
            color = .systemOrange
        case .read:
            color = .tertiaryLabelColor
        }
        cell.set(marker: "●", markerColor: color, title: item.title, detail: item.detail)
        cell.setAccessibilityIdentifier(
            "muxterm.sidebar.agent.\(safeID(item.workspaceId)).\(item.paneId)"
        )
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !isReloadingSelection else { return }
        guard let table = notification.object as? NSTableView else { return }
        if table === workspaceTable {
            let row = table.selectedRow
            guard workspaces.indices.contains(row) else { return }
            onWorkspaceActivate?(workspaces[row].workspaceId)
        } else if table === agentTable {
            let row = table.selectedRow
            guard agents.indices.contains(row) else { return }
            let agent = agents[row]
            onAgentActivate?(agent.workspaceId, agent.paneId)
        }
    }

    @objc private func toggleWorkspaceSection() {
        setSection(workspaceScroll, expanded: workspaceHeader.state == .on)
    }

    @objc private func toggleAgentSection() {
        setSection(agentScroll, expanded: agentHeader.state == .on)
    }

    private func configureTable(
        _ table: NSTableView,
        scroll: NSScrollView,
        identifier: String
    ) {
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("main"))
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 48
        table.style = .sourceList
        table.intercellSpacing = NSSize(width: 0, height: 2)
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityIdentifier(identifier + ".list")
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.documentView = table
        scroll.setAccessibilityIdentifier(identifier)
    }

    private func configureHeader(_ button: NSButton, title: String, action: Selector) {
        button.title = "▾  \(title)"
        button.font = .systemFont(ofSize: 11, weight: .semibold)
        button.alignment = .left
        button.isBordered = false
        button.setButtonType(.toggle)
        button.state = .on
        button.target = self
        button.action = action
        button.translatesAutoresizingMaskIntoConstraints = false
        button.setAccessibilityIdentifier("muxterm.sidebar.\(title.lowercased()).toggle")
    }

    private func section(header: NSButton, scroll: NSScrollView) -> NSView {
        let view = NSView()
        header.translatesAutoresizingMaskIntoConstraints = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(header)
        view.addSubview(scroll)
        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 8),
            header.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -8),
            header.topAnchor.constraint(equalTo: view.topAnchor),
            header.heightAnchor.constraint(equalToConstant: 28),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: header.bottomAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        return view
    }

    private func setSection(_ scroll: NSScrollView, expanded: Bool) {
        scroll.isHidden = !expanded
        let header = scroll === workspaceScroll ? workspaceHeader : agentHeader
        let title = scroll === workspaceScroll ? "WORKSPACES" : "AGENTS"
        header.title = "\(expanded ? "▾" : "▸")  \(title)"
    }

    private func sidebarCell(
        in table: NSTableView,
        identifier value: String
    ) -> WorkspaceSidebarCellView {
        let identifier = NSUserInterfaceItemIdentifier(value)
        return table.makeView(withIdentifier: identifier, owner: self) as? WorkspaceSidebarCellView
            ?? WorkspaceSidebarCellView(identifier: identifier)
    }

    private func safeID(_ value: String) -> String {
        value.map { character in
            character.isLetter || character.isNumber || character == "-" || character == "_"
                ? character
                : "-"
        }.reduce(into: "") { $0.append($1) }
    }

    // MARK: - Tests

    func testWorkspaceCount() -> Int { workspaces.count }
    func testAgentCount() -> Int { agents.count }
    func testAgentIndicators() -> [AgentSidebarIndicator] { agents.map(\.indicator) }
    func testWorkspaceNames() -> [String] { workspaces.map(\.name) }
}

private final class WorkspaceSidebarCellView: NSTableCellView {
    private let marker = NSTextField(labelWithString: "")
    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        marker.translatesAutoresizingMaskIntoConstraints = false
        marker.font = .systemFont(ofSize: 10)
        marker.alignment = .center
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = .systemFont(ofSize: 12.5, weight: .medium)
        titleLabel.lineBreakMode = .byTruncatingTail
        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = .systemFont(ofSize: 10.5)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.lineBreakMode = .byTruncatingTail
        addSubview(marker)
        addSubview(titleLabel)
        addSubview(detailLabel)
        textField = titleLabel
        NSLayoutConstraint.activate([
            marker.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            marker.centerYAnchor.constraint(equalTo: centerYAnchor),
            marker.widthAnchor.constraint(equalToConstant: 12),
            titleLabel.leadingAnchor.constraint(equalTo: marker.trailingAnchor, constant: 7),
            titleLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 6),
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 2),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    func set(marker: String, markerColor: NSColor, title: String, detail: String) {
        self.marker.stringValue = marker
        self.marker.textColor = markerColor
        titleLabel.stringValue = title
        detailLabel.stringValue = detail
    }
}
