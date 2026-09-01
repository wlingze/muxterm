import AppKit
import MuxtermChrome

enum SidebarTestSection {
    case workspaces, agents, commands, hiddenCommands
}

/// Native main-window sidebar mirroring Linux's two persistent sections.
final class WorkspaceSidebarView: NSView, NSTableViewDataSource, NSTableViewDelegate {
    var onWorkspaceActivate: ((String) -> Void)?
    var onWorkspaceClose: ((String) -> Void)?
    var onAgentActivate: ((String, UInt32) -> Void)?
    var onCommandActivate: ((String, UInt32) -> Void)?

    private let sections = NSStackView()
    private let workspaceTable = NSTableView()
    private let agentTable = NSTableView()
    private let commandTable = NSTableView()
    private let hiddenCommandTable = NSTableView()
    private let workspaceScroll = NSScrollView()
    private let agentScroll = NSScrollView()
    private let commandScroll = NSScrollView()
    private let hiddenCommandScroll = NSScrollView()
    private let workspaceHeader = NSButton()
    private let agentHeader = NSButton()
    private let commandHeader = NSButton()
    private let hiddenCommandHeader = NSButton()
    private var hiddenCommandKeys = Set<CommandVisibilityKey>()
    private var sectionViews: [NSScrollView: NSView] = [:]
    private var sectionHeightConstraints: [NSScrollView: NSLayoutConstraint] = [:]
    private var workspaces: [WorkspaceSidebarItem] = []
    private var agents: [AgentSidebarItem] = []
    private var commands: [CommandSidebarItem] = []
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
        configureTable(
            commandTable,
            scroll: commandScroll,
            identifier: "muxterm.sidebar.commands"
        )
        configureTable(
            hiddenCommandTable,
            scroll: hiddenCommandScroll,
            identifier: "muxterm.sidebar.hiddenCommands"
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
        configureHeader(
            commandHeader,
            title: "COMMANDS",
            action: #selector(toggleCommandSection)
        )
        configureHeader(
            hiddenCommandHeader,
            title: "HIDDEN COMMANDS",
            action: #selector(toggleHiddenCommandSection)
        )
        hiddenCommandHeader.state = .off

        let workspaceSection = section(header: workspaceHeader, scroll: workspaceScroll)
        workspaceSection.setAccessibilityIdentifier("muxterm.sidebar.workspaces.section")
        let agentSection = section(header: agentHeader, scroll: agentScroll)
        agentSection.setAccessibilityIdentifier("muxterm.sidebar.agents.section")
        let commandSection = section(header: commandHeader, scroll: commandScroll)
        commandSection.setAccessibilityIdentifier("muxterm.sidebar.commands.section")
        let hiddenCommandSection = section(header: hiddenCommandHeader, scroll: hiddenCommandScroll)
        hiddenCommandSection.setAccessibilityIdentifier("muxterm.sidebar.hiddenCommands.section")
        sectionViews = [
            workspaceScroll: workspaceSection,
            agentScroll: agentSection,
            commandScroll: commandSection,
            hiddenCommandScroll: hiddenCommandSection,
        ]

        sections.translatesAutoresizingMaskIntoConstraints = false
        sections.orientation = .vertical
        sections.alignment = .leading
        sections.spacing = 0
        sections.distribution = .fill
        for view in [hiddenCommandSection, commandSection, agentSection, workspaceSection] {
            view.translatesAutoresizingMaskIntoConstraints = false
            sections.addArrangedSubview(view)
            sections.trailingAnchor.constraint(equalTo: view.trailingAnchor).isActive = true
        }
        setSection(workspaceScroll, expanded: true)
        setSection(agentScroll, expanded: true)
        setSection(commandScroll, expanded: true)
        setSection(hiddenCommandScroll, expanded: false)
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

    func setCommands(_ items: [CommandSidebarItem]) {
        let currentKeys = Set(items.map(CommandVisibilityKey.init))
        hiddenCommandKeys.formIntersection(currentKeys)
        guard commands != items else {
            hiddenCommandTable.reloadData()
            return
        }
        commands = items
        commandTable.reloadData()
        hiddenCommandTable.reloadData()
    }

    private func toggleCommandVisibility(_ key: CommandVisibilityKey) {
        if !hiddenCommandKeys.insert(key).inserted {
            hiddenCommandKeys.remove(key)
        }
        commandTable.reloadData()
        hiddenCommandTable.reloadData()
    }

    private var visibleCommands: [CommandSidebarItem] {
        commands.filter { !hiddenCommandKeys.contains(CommandVisibilityKey($0)) }
    }

    private var hiddenCommands: [CommandSidebarItem] {
        commands.filter { hiddenCommandKeys.contains(CommandVisibilityKey($0)) }
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        if tableView === workspaceTable { return workspaces.count }
        if tableView === agentTable { return agents.count }
        if tableView === commandTable { return visibleCommands.count }
        if tableView === hiddenCommandTable { return hiddenCommands.count }
        return 0
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
                detail: "\(item.runtime) @ \(item.transport)",
                shortcut: item.shortcut,
                trailingTooltip: "Close workspace",
                trailingAccessibilityID: "muxterm.sidebar.workspace.close.\(safeID(item.workspaceId))",
                trailingAction: { [weak self] in
                    self?.onWorkspaceClose?(item.workspaceId)
                }
            )
            cell.setAccessibilityIdentifier("muxterm.sidebar.workspace.\(safeID(item.workspaceId))")
            return cell
        }

        if tableView === commandTable || tableView === hiddenCommandTable {
            let visible = tableView === commandTable
            let source = visible ? visibleCommands : hiddenCommands
            guard source.indices.contains(row) else { return nil }
            let item = source[row]
            let key = CommandVisibilityKey(item)
            let cell = sidebarCell(
                in: tableView,
                identifier: visible ? "CommandSidebarCell" : "HiddenCommandSidebarCell"
            )
            cell.set(
                marker: "●",
                markerColor: indicatorColor(item.indicator),
                title: item.title,
                detail: item.detail,
                trailingSymbol: visible ? "eye.slash" : "eye",
                trailingTooltip: visible ? "Hide command" : "Show command",
                trailingAccessibilityID: "muxterm.sidebar.command.visibility.\(safeID(item.workspaceId)).\(item.paneId)",
                trailingShowsOnHover: false,
                trailingAction: { [weak self] in
                    self?.toggleCommandVisibility(key)
                }
            )
            cell.setAccessibilityIdentifier(
                "muxterm.sidebar.\(visible ? "command" : "hiddenCommand").\(safeID(item.workspaceId)).\(item.paneId)"
            )
            return cell
        }

        guard agents.indices.contains(row) else { return nil }
        let item = agents[row]
        let cell = sidebarCell(in: tableView, identifier: "AgentSidebarCell")
        cell.set(
            marker: "●",
            markerColor: indicatorColor(item.indicator),
            title: item.title,
            detail: item.detail
        )
        cell.setAccessibilityIdentifier(
            "muxterm.sidebar.agent.\(safeID(item.workspaceId)).\(item.paneId)"
        )
        return cell
    }

    private func indicatorColor(_ indicator: AgentSidebarIndicator) -> NSColor {
        switch indicator {
        case .running:
            .systemGreen
        case .done:
            .systemOrange
        case .read:
            .tertiaryLabelColor
        }
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
        } else if table === commandTable || table === hiddenCommandTable {
            let source = table === commandTable ? visibleCommands : hiddenCommands
            let row = table.selectedRow
            guard source.indices.contains(row) else { return }
            let command = source[row]
            onCommandActivate?(command.workspaceId, command.paneId)
        }
    }

    @objc private func toggleWorkspaceSection() {
        setSection(workspaceScroll, expanded: workspaceHeader.state == .on)
    }

    @objc private func toggleAgentSection() {
        setSection(agentScroll, expanded: agentHeader.state == .on)
    }

    @objc private func toggleCommandSection() {
        setSection(commandScroll, expanded: commandHeader.state == .on)
    }

    @objc private func toggleHiddenCommandSection() {
        setSection(hiddenCommandScroll, expanded: hiddenCommandHeader.state == .on)
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
        let header: NSButton
        let title: String
        if scroll === workspaceScroll {
            header = workspaceHeader
            title = "WORKSPACES"
        } else if scroll === agentScroll {
            header = agentHeader
            title = "AGENTS"
        } else if scroll === commandScroll {
            header = commandHeader
            title = "COMMANDS"
        } else {
            header = hiddenCommandHeader
            title = "HIDDEN COMMANDS"
        }
        header.title = "\(expanded ? "▾" : "▸")  \(title)"
        updateSectionConstraints()
    }

    /// VSCode/Cursor section packing.
    ///
    /// Expanded sections divide all remaining height. Collapsed sections keep
    /// only their 28pt header; those before the first expanded section pack to
    /// the top, those after the last expanded section pack to the bottom, and
    /// all-collapsed packs to the top.
    private func updateSectionConstraints() {
        for constraint in sectionHeightConstraints.values {
            constraint.isActive = false
        }
        sectionHeightConstraints.removeAll()

        let ordered: [(scroll: NSScrollView, expanded: Bool)] = [
            (workspaceScroll, workspaceHeader.state == .on),
            (agentScroll, agentHeader.state == .on),
            (commandScroll, commandHeader.state == .on),
            (hiddenCommandScroll, hiddenCommandHeader.state == .on),
        ]
        let expandedCount = ordered.filter(\.expanded).count
        for (scroll, expanded) in ordered {
            guard let view = sectionViews[scroll] else { continue }
            let constraint: NSLayoutConstraint
            if expanded {
                constraint = view.heightAnchor.constraint(
                    greaterThanOrEqualToConstant: 120
                )
                constraint.priority = .required
            } else {
                constraint = view.heightAnchor.constraint(equalToConstant: 28)
            }
            constraint.isActive = true
            sectionHeightConstraints[scroll] = constraint
        }
        sections.distribution = expandedCount == 0 ? .fill : .fillEqually
        // Equal distribution applies only to expanded sections; manually pinned
        // collapsed sections are excluded by their exact height constraints.
        needsLayout = true
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
    func testCommandCount() -> Int { visibleCommands.count }
    func testCommandTitles() -> [String] { visibleCommands.map(\.title) }
    func testHiddenCommandTitles() -> [String] { hiddenCommands.map(\.title) }
    func testToggleCommandVisibility(workspaceId: String, paneId: UInt32) {
        guard let item = commands.first(where: {
            $0.workspaceId == workspaceId && $0.paneId == paneId
        }) else { return }
        toggleCommandVisibility(CommandVisibilityKey(item))
    }

    func testSetSectionExpanded(_ section: SidebarTestSection, _ expanded: Bool) {
        let scroll: NSScrollView
        switch section {
        case .workspaces: scroll = workspaceScroll
        case .agents: scroll = agentScroll
        case .commands: scroll = commandScroll
        case .hiddenCommands: scroll = hiddenCommandScroll
        }
        let header: NSButton
        switch section {
        case .workspaces: header = workspaceHeader
        case .agents: header = agentHeader
        case .commands: header = commandHeader
        case .hiddenCommands: header = hiddenCommandHeader
        }
        header.state = expanded ? .on : .off
        setSection(scroll, expanded: expanded)
        needsLayout = true
        layoutSubtreeIfNeeded()
    }

    func testSectionFrames() -> [SidebarTestSection: NSRect] {
        [
            .workspaces: workspaceScroll.superview?.frame ?? .zero,
            .agents: agentScroll.superview?.frame ?? .zero,
            .commands: commandScroll.superview?.frame ?? .zero,
            .hiddenCommands: hiddenCommandScroll.superview?.frame ?? .zero,
        ]
    }
    func testAgentIndicators() -> [AgentSidebarIndicator] { agents.map(\.indicator) }
    func testWorkspaceNames() -> [String] { workspaces.map(\.name) }
    func testWorkspaceIDs() -> [String] { workspaces.map(\.workspaceId) }
}

private final class WorkspaceSidebarCellView: NSTableCellView {
    private let marker = NSTextField(labelWithString: "")
    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")
    private let shortcutLabel = NSTextField(labelWithString: "")
    private let trailingButton = NSButton()
    private var trailingAction: (() -> Void)?
    private var trailingShowsOnHover = true
    private var trailingButtonWidth: NSLayoutConstraint!
    private var titleLeadingToTrailing: NSLayoutConstraint!
    private var isHovered = false {
        didSet {
            guard isHovered != oldValue else { return }
            updateTrailingVisibility()
        }
    }

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
        shortcutLabel.translatesAutoresizingMaskIntoConstraints = false
        shortcutLabel.font = .systemFont(ofSize: 10, weight: .semibold)
        shortcutLabel.textColor = .tertiaryLabelColor
        shortcutLabel.alignment = .center
        trailingButton.translatesAutoresizingMaskIntoConstraints = false
        trailingButton.isBordered = false
        trailingButton.font = .systemFont(ofSize: 11, weight: .semibold)
        trailingButton.controlSize = .small
        trailingButton.setButtonType(.momentaryPushIn)
        trailingButton.action = #selector(trailingClicked)
        trailingButton.target = self
        addSubview(marker)
        addSubview(shortcutLabel)
        addSubview(titleLabel)
        addSubview(detailLabel)
        addSubview(trailingButton)
        textField = titleLabel
        updateTrackingAreas()

        trailingButtonWidth = trailingButton.widthAnchor.constraint(equalToConstant: 0)
        titleLeadingToTrailing = titleLabel.trailingAnchor.constraint(
            equalTo: trailingButton.leadingAnchor,
            constant: 0
        )
        NSLayoutConstraint.activate([
            marker.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            marker.centerYAnchor.constraint(equalTo: centerYAnchor),
            marker.widthAnchor.constraint(equalToConstant: 12),
            shortcutLabel.leadingAnchor.constraint(equalTo: marker.trailingAnchor, constant: 7),
            shortcutLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            shortcutLabel.widthAnchor.constraint(equalToConstant: 14),
            titleLabel.leadingAnchor.constraint(equalTo: shortcutLabel.trailingAnchor, constant: 4),
            titleLeadingToTrailing,
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 6),
            trailingButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            trailingButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            trailingButtonWidth,
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 2),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let area = trackingAreas.first(where: { $0.owner === self }) {
            removeTrackingArea(area)
        }
        addTrackingArea(
            NSTrackingArea(
                rect: bounds,
                options: [.mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect],
                owner: self,
                userInfo: nil
            )
        )
    }

    override func mouseEntered(with event: NSEvent) {
        isHovered = true
    }

    override func mouseExited(with event: NSEvent) {
        isHovered = false
    }

    private func updateTrailingVisibility() {
        let showsTrailing = trailingAction != nil
            && (!trailingShowsOnHover || isHovered)
        trailingButtonWidth.constant = showsTrailing ? 24 : 0
        titleLeadingToTrailing.constant = showsTrailing ? -4 : 0
        trailingButton.isHidden = !showsTrailing
    }

    @objc private func trailingClicked() {
        trailingAction?()
    }

    func set(
        marker: String,
        markerColor: NSColor,
        title: String,
        detail: String,
        shortcut: Int? = nil,
        closeAction: (() -> Void)? = nil,
        trailingSymbol: String? = nil,
        trailingTooltip: String? = nil,
        trailingAccessibilityID: String? = nil,
        trailingShowsOnHover: Bool = true,
        trailingAction: (() -> Void)? = nil
    ) {
        self.marker.stringValue = marker
        self.marker.textColor = markerColor
        shortcutLabel.stringValue = shortcut.map(String.init) ?? ""
        titleLabel.stringValue = title
        detailLabel.stringValue = detail

        let symbol = trailingSymbol ?? (closeAction == nil ? nil : "xmark")
        if let symbol {
            trailingButton.image = NSImage(
                systemSymbolName: symbol,
                accessibilityDescription: trailingTooltip
            )
            trailingButton.imagePosition = .imageOnly
            trailingButton.contentTintColor = .secondaryLabelColor
        } else {
            trailingButton.image = nil
        }
        trailingButton.toolTip = trailingTooltip
        if let id = trailingAccessibilityID {
            trailingButton.setAccessibilityIdentifier(id)
        }
        self.trailingShowsOnHover = trailingShowsOnHover
        self.trailingAction = trailingAction ?? closeAction
        updateTrailingVisibility()
    }
}

private struct CommandVisibilityKey: Hashable {
    let workspaceId: String
    let paneId: UInt32
    let title: String

    init(_ item: CommandSidebarItem) {
        workspaceId = item.workspaceId
        paneId = item.paneId
        title = item.title
    }
}
