import AppKit
import MuxtermChrome

enum SidebarTestSection {
    case workspaces, agents, commands, hiddenCommands
}

/// Native main-window sidebar with four compact, independently collapsible sections.
final class WorkspaceSidebarView: NSView, NSTableViewDataSource, NSTableViewDelegate, NSSplitViewDelegate {
    var onWorkspaceActivate: ((String) -> Void)?
    var onWorkspaceClose: ((String) -> Void)?
    var onWorkspaceReorder: (([String]) -> Void)?
    var onAgentActivate: ((String, UInt32?, UInt32) -> Void)?
    var onCommandActivate: ((String, UInt32?, UInt32) -> Void)?

    private let sections = NSSplitView()
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
    private var orderedSectionScrolls: [NSScrollView] = []
    private var expandedSections: [ObjectIdentifier: Bool] = [:]
    private var workspaces: [WorkspaceSidebarItem] = []
    private var agents: [AgentSidebarItem] = []
    private var commands: [CommandSidebarItem] = []
    private var isReloadingSelection = false
    private var workspaceReloadCount = 0
    private var agentReloadCount = 0
    private var commandReloadCount = 0
    private var workspaceSelectionMutationCount = 0
    private static let workspaceDragType = NSPasteboard.PasteboardType("muxterm.sidebar.workspace")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setAccessibilityIdentifier("muxterm.sidebar")
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor

        configureTable(
            workspaceTable,
            scroll: workspaceScroll,
            identifier: "muxterm.sidebar.workspaces",
            allowsReorder: true
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
        sections.isVertical = false
        sections.dividerStyle = .thin
        sections.delegate = self
        // v1 autosave 在 0 尺寸时写下全 0 frame，下次启动侧栏像消失。
        sections.autosaveName = "muxterm.sidebar.sectionSplit.v2"
        orderedSectionScrolls = [workspaceScroll, agentScroll, commandScroll, hiddenCommandScroll]
        for view in [workspaceSection, agentSection, commandSection, hiddenCommandSection] {
            // NSSplitView 靠 frame 排子视图；TAMIC=false 会把高度算成 0。
            view.translatesAutoresizingMaskIntoConstraints = true
            sections.addArrangedSubview(view)
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

        let previous = workspaces
        let identityChanged = previous.map(\.workspaceId) != items.map(\.workspaceId)
        let changedRows = identityChanged
            ? IndexSet()
            : IndexSet(items.indices.filter { index in
                !sameWorkspacePresentation(previous[index], items[index])
            })

        isReloadingSelection = true
        defer { isReloadingSelection = false }
        workspaces = items
        if identityChanged {
            workspaceReloadCount += 1
            workspaceTable.reloadData()
        } else {
            for row in changedRows {
                guard let cell = workspaceTable.view(
                    atColumn: 0,
                    row: row,
                    makeIfNecessary: false
                ) as? WorkspaceSidebarCellView else {
                    continue
                }
                configureWorkspaceCell(cell, item: items[row])
            }
        }
        updateSectionHeaderTitles()
        if let active = items.firstIndex(where: \.isActive) {
            setSelection(in: workspaceTable, row: active)
        } else {
            setSelection(in: workspaceTable, row: nil)
        }
    }

    func setAgents(_ items: [AgentSidebarItem]) {
        guard agents != items else { return }
        let previousIDs = agents.map(agentIdentity)
        let nextIDs = items.map(agentIdentity)
        isReloadingSelection = true
        defer { isReloadingSelection = false }
        agents = items
        if previousIDs != nextIDs {
            agentReloadCount += 1
            agentTable.reloadData()
        } else {
            for row in items.indices {
                guard let cell = agentTable.view(
                    atColumn: 0,
                    row: row,
                    makeIfNecessary: false
                ) as? WorkspaceSidebarCellView else {
                    continue
                }
                configureAgentCell(cell, item: items[row])
            }
        }
        updateSectionHeaderTitles()
    }

    func setCommands(_ items: [CommandSidebarItem]) {
        let currentKeys = Set(items.map(CommandVisibilityKey.init))
        hiddenCommandKeys.formIntersection(currentKeys)
        guard commands != items else {
            updateSectionHeaderTitles()
            return
        }
        let previousVisible = visibleCommands.map(commandIdentity)
        let previousHidden = hiddenCommands.map(commandIdentity)
        isReloadingSelection = true
        defer { isReloadingSelection = false }
        commands = items
        let nextVisible = visibleCommands
        let nextHidden = hiddenCommands
        if previousVisible != nextVisible.map(commandIdentity)
            || commandTable.numberOfRows != nextVisible.count
        {
            commandReloadCount += 1
            commandTable.reloadData()
        } else {
            for row in nextVisible.indices {
                guard row < commandTable.numberOfRows,
                      let cell = commandTable.view(
                        atColumn: 0,
                        row: row,
                        makeIfNecessary: false
                      ) as? WorkspaceSidebarCellView
                else {
                    continue
                }
                configureCommandCell(cell, item: nextVisible[row], visible: true)
            }
        }
        if previousHidden != nextHidden.map(commandIdentity)
            || hiddenCommandTable.numberOfRows != nextHidden.count
        {
            hiddenCommandTable.reloadData()
        } else {
            for row in nextHidden.indices {
                guard row < hiddenCommandTable.numberOfRows,
                      let cell = hiddenCommandTable.view(
                        atColumn: 0,
                        row: row,
                        makeIfNecessary: false
                      ) as? WorkspaceSidebarCellView
                else {
                    continue
                }
                configureCommandCell(cell, item: nextHidden[row], visible: false)
            }
        }
        updateSectionHeaderTitles()
    }

    /// 将四个列表的选中态绑定到当前真正显示的 pane。
    ///
    /// Agent、Command 和 Hidden Command 都是同一组导航目标的不同投影，
    /// 当前 pane 变化时只能有一个投影显示选中框；不匹配任何条目时全部
    /// 清空，避免旧 row 因为切换 Workspace 或 pane 而残留高亮。
    func setActiveTarget(workspaceId: String?, tabId: UInt32?, paneId: UInt32?) {
        isReloadingSelection = true
        defer { isReloadingSelection = false }

        if let workspaceId,
           let workspaceRow = workspaces.firstIndex(where: {
               $0.workspaceId == workspaceId
           })
        {
            setSelection(in: workspaceTable, row: workspaceRow)
        } else {
            setSelection(in: workspaceTable, row: nil)
        }

        guard let workspaceId, let paneId else {
            setSelection(in: agentTable, row: nil)
            setSelection(in: commandTable, row: nil)
            setSelection(in: hiddenCommandTable, row: nil)
            return
        }

        // paneId 在一个 Workspace 内是稳定且唯一的；如果两端都有 tabId，
        // 再用它排除过期的 pane 映射。旧版本/首帧缺失 tabId 时仍按 pane
        // 同步高亮。
        func isTarget(workspace: String, pane: UInt32, tab: UInt32?) -> Bool {
            workspace == workspaceId
                && pane == paneId
                && (tabId == nil || tab == nil || tab == tabId)
        }
        if let agentRow = agents.firstIndex(where: {
            isTarget(workspace: $0.workspaceId, pane: $0.paneId, tab: $0.tabId)
        }) {
            setSelection(in: agentTable, row: agentRow)
            setSelection(in: commandTable, row: nil)
            setSelection(in: hiddenCommandTable, row: nil)
            return
        }

        if let commandRow = visibleCommands.firstIndex(where: {
            isTarget(workspace: $0.workspaceId, pane: $0.paneId, tab: $0.tabId)
        }) {
            setSelection(in: commandTable, row: commandRow)
            setSelection(in: agentTable, row: nil)
            setSelection(in: hiddenCommandTable, row: nil)
            return
        }

        if let hiddenRow = hiddenCommands.firstIndex(where: {
            isTarget(workspace: $0.workspaceId, pane: $0.paneId, tab: $0.tabId)
        }) {
            setSelection(in: hiddenCommandTable, row: hiddenRow)
        } else {
            setSelection(in: hiddenCommandTable, row: nil)
        }
        setSelection(in: agentTable, row: nil)
        setSelection(in: commandTable, row: nil)
    }

    private func toggleCommandVisibility(_ key: CommandVisibilityKey) {
        if !hiddenCommandKeys.insert(key).inserted {
            hiddenCommandKeys.remove(key)
        }
        commandTable.reloadData()
        hiddenCommandTable.reloadData()
        updateSectionHeaderTitles()
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
            configureWorkspaceCell(cell, item: item)
            return cell
        }

        if tableView === commandTable || tableView === hiddenCommandTable {
            let visible = tableView === commandTable
            let source = visible ? visibleCommands : hiddenCommands
            guard source.indices.contains(row) else { return nil }
            let cell = sidebarCell(
                in: tableView,
                identifier: visible ? "CommandSidebarCell" : "HiddenCommandSidebarCell"
            )
            configureCommandCell(cell, item: source[row], visible: visible)
            return cell
        }

        guard agents.indices.contains(row) else { return nil }
        let cell = sidebarCell(in: tableView, identifier: "AgentSidebarCell")
        configureAgentCell(cell, item: agents[row])
        return cell
    }

    private func configureWorkspaceCell(
        _ cell: WorkspaceSidebarCellView,
        item: WorkspaceSidebarItem
    ) {
        cell.set(
            marker: item.isActive ? "●" : "○",
            markerColor: item.isActive ? .controlAccentColor : .tertiaryLabelColor,
            title: item.name,
            detail: "\(item.runtime) @ \(item.transport)",
            shortcut: item.shortcut,
            trailingSymbol: "xmark",
            trailingTooltip: "Close workspace",
            trailingAccessibilityID: "muxterm.sidebar.workspace.close.\(safeID(item.workspaceId))",
            trailingAction: { [weak self] in
                self?.onWorkspaceClose?(item.workspaceId)
            }
        )
        cell.setAccessibilityIdentifier("muxterm.sidebar.workspace.\(safeID(item.workspaceId))")
    }

    private func configureAgentCell(
        _ cell: WorkspaceSidebarCellView,
        item: AgentSidebarItem
    ) {
        cell.set(
            marker: "●",
            markerColor: indicatorColor(item.indicator),
            title: item.title,
            detail: item.detail
        )
        cell.setAccessibilityIdentifier(
            "muxterm.sidebar.agent.\(safeID(item.workspaceId)).\(item.paneId)"
        )
    }

    private func configureCommandCell(
        _ cell: WorkspaceSidebarCellView,
        item: CommandSidebarItem,
        visible: Bool
    ) {
        let key = CommandVisibilityKey(item)
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
    }

    private func agentIdentity(_ item: AgentSidebarItem) -> String {
        "\(item.workspaceId).\(item.paneId)"
    }

    private func commandIdentity(_ item: CommandSidebarItem) -> String {
        "\(item.workspaceId).\(item.paneId)"
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
            commandTable.deselectAll(nil)
            hiddenCommandTable.deselectAll(nil)
            onAgentActivate?(agent.workspaceId, agent.tabId, agent.paneId)
        } else if table === commandTable || table === hiddenCommandTable {
            let source = table === commandTable ? visibleCommands : hiddenCommands
            let row = table.selectedRow
            guard source.indices.contains(row) else { return }
            let command = source[row]
            agentTable.deselectAll(nil)
            if table === commandTable {
                hiddenCommandTable.deselectAll(nil)
            } else {
                commandTable.deselectAll(nil)
            }
            onCommandActivate?(command.workspaceId, command.tabId, command.paneId)
        }
    }

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        guard tableView === workspaceTable, workspaces.indices.contains(row) else {
            return nil
        }
        let item = NSPasteboardItem()
        item.setString(workspaces[row].workspaceId, forType: Self.workspaceDragType)
        return item
    }

    func tableView(
        _ tableView: NSTableView,
        validateDrop info: NSDraggingInfo,
        proposedRow row: Int,
        proposedDropOperation dropOperation: NSTableView.DropOperation
    ) -> NSDragOperation {
        guard tableView === workspaceTable, dropOperation == .above else { return [] }
        return .move
    }

    func tableView(
        _ tableView: NSTableView,
        acceptDrop info: NSDraggingInfo,
        row: Int,
        dropOperation: NSTableView.DropOperation
    ) -> Bool {
        guard tableView === workspaceTable,
              dropOperation == .above,
              let dragged = info.draggingPasteboard.string(forType: Self.workspaceDragType),
              let from = workspaces.firstIndex(where: { $0.workspaceId == dragged })
        else {
            return false
        }
        var ids = workspaces.map(\.workspaceId)
        ids.remove(at: from)
        let destination = from < row ? row - 1 : row
        let clamped = min(max(destination, 0), ids.count)
        ids.insert(dragged, at: clamped)
        onWorkspaceReorder?(ids)
        return true
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
        identifier: String,
        allowsReorder: Bool = false
    ) {
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("main"))
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 36
        table.style = .sourceList
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.dataSource = self
        table.delegate = self
        table.allowsEmptySelection = true
        table.setAccessibilityIdentifier(identifier + ".list")
        if allowsReorder {
            table.registerForDraggedTypes([Self.workspaceDragType])
            table.setDraggingSourceOperationMask(.move, forLocal: true)
        }
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.documentView = table
        scroll.setAccessibilityIdentifier(identifier)
    }

    private func configureHeader(_ button: NSButton, title: String, action: Selector) {
        button.title = title
        button.image = NSImage(
            systemSymbolName: "chevron.down",
            accessibilityDescription: nil
        )
        button.imagePosition = .imageLeading
        button.imageScaling = .scaleProportionallyDown
        button.contentTintColor = .secondaryLabelColor
        button.font = .systemFont(ofSize: 10.5, weight: .semibold)
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
            header.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 6),
            header.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -6),
            header.topAnchor.constraint(equalTo: view.topAnchor),
            header.heightAnchor.constraint(equalToConstant: 26),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: header.bottomAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        return view
    }

    private func setSection(_ scroll: NSScrollView, expanded: Bool) {
        expandedSections[ObjectIdentifier(scroll)] = expanded
        scroll.isHidden = !expanded
        let header: NSButton
        let section: SidebarTestSection
        if scroll === workspaceScroll {
            header = workspaceHeader
            section = .workspaces
        } else if scroll === agentScroll {
            header = agentHeader
            section = .agents
        } else if scroll === commandScroll {
            header = commandHeader
            section = .commands
        } else {
            header = hiddenCommandHeader
            section = .hiddenCommands
        }
        header.state = expanded ? .on : .off
        header.image = NSImage(
            systemSymbolName: expanded ? "chevron.down" : "chevron.right",
            accessibilityDescription: nil
        )
        header.title = sectionTitle(section)
        updateSectionConstraints()
    }

    private func updateSectionHeaderTitles() {
        for (scroll, expanded) in [
            (workspaceScroll, workspaceHeader.state == .on),
            (agentScroll, agentHeader.state == .on),
            (commandScroll, commandHeader.state == .on),
            (hiddenCommandScroll, hiddenCommandHeader.state == .on),
        ] {
            let header: NSButton
            let section: SidebarTestSection
            if scroll === workspaceScroll {
                header = workspaceHeader
                section = .workspaces
            } else if scroll === agentScroll {
                header = agentHeader
                section = .agents
            } else if scroll === commandScroll {
                header = commandHeader
                section = .commands
            } else {
                header = hiddenCommandHeader
                section = .hiddenCommands
            }
            header.image = NSImage(
                systemSymbolName: expanded ? "chevron.down" : "chevron.right",
                accessibilityDescription: nil
            )
            header.title = sectionTitle(section)
        }
    }

    private func setSelection(in table: NSTableView, row: Int?) {
        let selectedRow = table.selectedRow
        let targetRow = row ?? -1
        guard selectedRow != targetRow else { return }
        if table === workspaceTable {
            workspaceSelectionMutationCount += 1
        }
        if let row {
            table.selectRowIndexes(
                IndexSet(integer: row),
                byExtendingSelection: false
            )
        } else {
            table.deselectAll(nil)
        }
    }

    private func sameWorkspacePresentation(
        _ lhs: WorkspaceSidebarItem,
        _ rhs: WorkspaceSidebarItem
    ) -> Bool {
        lhs.workspaceId == rhs.workspaceId
            && lhs.name == rhs.name
            && lhs.runtime == rhs.runtime
            && lhs.transport == rhs.transport
            && lhs.isActive == rhs.isActive
            && lhs.shortcut == rhs.shortcut
    }

    private func sectionTitle(_ section: SidebarTestSection) -> String {
        let title: String
        let count: Int
        switch section {
        case .workspaces:
            title = "WORKSPACES"
            count = workspaces.count
        case .agents:
            title = "AGENTS"
            count = agents.count
        case .commands:
            title = "COMMANDS"
            count = visibleCommands.count
        case .hiddenCommands:
            title = "HIDDEN COMMANDS"
            count = hiddenCommands.count
        }
        return count > 0 ? "\(title)  \(count)" : title
    }

    /// Collapsed sections keep a 26pt header. Expanded sections stay draggable
    /// in the split view so Agents/Commands can be resized independently.
    private func updateSectionConstraints() {
        for constraint in sectionHeightConstraints.values {
            constraint.isActive = false
        }
        sectionHeightConstraints.removeAll()
        for (index, scroll) in orderedSectionScrolls.enumerated() {
            let expanded = isSectionExpanded(scroll)
            sections.setHoldingPriority(
                expanded ? .defaultLow : .required,
                forSubviewAt: index
            )
        }
        sections.needsLayout = true
        applySectionFrames()
    }

    override func layout() {
        super.layout()
        applySectionFrames()
    }

    private func applySectionFrames() {
        splitView(sections, resizeSubviewsWithOldSize: sections.bounds.size)
    }

    private func isSectionExpanded(_ scroll: NSScrollView) -> Bool {
        expandedSections[ObjectIdentifier(scroll)] ?? (scroll !== hiddenCommandScroll)
    }

    func splitView(
        _ splitView: NSSplitView,
        constrainMinCoordinate proposedMinimumPosition: CGFloat,
        ofSubviewAt dividerIndex: Int
    ) -> CGFloat {
        let minHeight: CGFloat = isSectionExpanded(orderedSectionScrolls[dividerIndex]) ? 80 : 26
        var origin: CGFloat = 0
        for index in 0..<dividerIndex {
            origin += splitView.arrangedSubviews[index].bounds.height + splitView.dividerThickness
        }
        return origin + minHeight
    }

    func splitView(
        _ splitView: NSSplitView,
        constrainMaxCoordinate proposedMaximumPosition: CGFloat,
        ofSubviewAt dividerIndex: Int
    ) -> CGFloat {
        let next = dividerIndex + 1
        let minNext: CGFloat = next < orderedSectionScrolls.count
            && isSectionExpanded(orderedSectionScrolls[next]) ? 80 : 26
        return splitView.bounds.height - minNext
    }

    func splitView(_ splitView: NSSplitView, shouldHideDividerAt dividerIndex: Int) -> Bool {
        false
    }

    func splitView(_ splitView: NSSplitView, canCollapse subview: NSView) -> Bool {
        false
    }

    func splitView(_ splitView: NSSplitView, resizeSubviewsWithOldSize oldSize: NSSize) {
        let views = splitView.arrangedSubviews
        let expanded = orderedSectionScrolls.map(isSectionExpanded)
        let frames: [CGRect]
        if let heights = SidebarSectionSplitLayout.heights(
            boundsHeight: splitView.bounds.height,
            dividerThickness: splitView.dividerThickness,
            expanded: expanded,
            currentHeights: views.map(\.bounds.height)
        ) {
            frames = SidebarSectionSplitLayout.frames(
                bounds: splitView.bounds.size,
                dividerThickness: splitView.dividerThickness,
                heights: heights,
                flipped: splitView.isFlipped
            )
        } else {
            frames = SidebarSectionSplitLayout.degenerateFrames(
                count: views.count,
                width: splitView.bounds.width
            )
        }
        for (view, frame) in zip(views, frames) {
            view.setFrameOrigin(frame.origin)
            view.setFrameSize(frame.size)
        }
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
        setSection(scroll, expanded: expanded)
        header.state = expanded ? .on : .off
        needsLayout = true
        layoutSubtreeIfNeeded()
    }

    func testSectionsAreResizable() -> Bool {
        !sections.isVertical && sections.arrangedSubviews.count == 4
    }

    func testArrangedSectionFrames() -> [NSRect] {
        sections.arrangedSubviews.map(\.frame)
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
    func testSelectWorkspace(_ workspaceId: String) {
        guard let row = workspaces.firstIndex(where: { $0.workspaceId == workspaceId }) else {
            return
        }
        workspaceTable.selectRowIndexes(
            IndexSet(integer: row),
            byExtendingSelection: false
        )
    }

    func testSelectAgent(workspaceId: String, paneId: UInt32) {
        guard let row = agents.firstIndex(where: {
            $0.workspaceId == workspaceId && $0.paneId == paneId
        }) else {
            return
        }
        agentTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
    }

    func testSelectCommand(workspaceId: String, paneId: UInt32) {
        guard let row = visibleCommands.firstIndex(where: {
            $0.workspaceId == workspaceId && $0.paneId == paneId
        }) else {
            return
        }
        commandTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
    }
    func testWorkspaceReloadCount() -> Int { workspaceReloadCount }
    func testAgentReloadCount() -> Int { agentReloadCount }
    func testCommandReloadCount() -> Int { commandReloadCount }
    func testWorkspaceSelectionMutationCount() -> Int { workspaceSelectionMutationCount }
    func testReorderWorkspaces(_ ids: [String]) {
        onWorkspaceReorder?(ids)
    }
    func testSelectedWorkspaceID() -> String? {
        let row = workspaceTable.selectedRow
        return workspaces.indices.contains(row) ? workspaces[row].workspaceId : nil
    }
    func testSelectedAgentPaneID() -> UInt32? {
        let row = agentTable.selectedRow
        return agents.indices.contains(row) ? agents[row].paneId : nil
    }
    func testSelectedCommandPaneID() -> UInt32? {
        let row = commandTable.selectedRow
        let values = visibleCommands
        return values.indices.contains(row) ? values[row].paneId : nil
    }
    func testSelectedHiddenCommandPaneID() -> UInt32? {
        let row = hiddenCommandTable.selectedRow
        let values = hiddenCommands
        return values.indices.contains(row) ? values[row].paneId : nil
    }
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
        marker.font = .systemFont(ofSize: 9.5)
        marker.alignment = .center
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = .systemFont(ofSize: 12, weight: .medium)
        titleLabel.lineBreakMode = .byTruncatingTail
        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = .systemFont(ofSize: 10)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.lineBreakMode = .byTruncatingTail
        shortcutLabel.translatesAutoresizingMaskIntoConstraints = false
        shortcutLabel.font = .systemFont(ofSize: 9.5, weight: .semibold)
        shortcutLabel.textColor = .tertiaryLabelColor
        shortcutLabel.alignment = .center
        trailingButton.translatesAutoresizingMaskIntoConstraints = false
        trailingButton.isBordered = false
        trailingButton.font = .systemFont(ofSize: 11, weight: .semibold)
        trailingButton.controlSize = .small
        trailingButton.imageScaling = .scaleProportionallyDown
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
            marker.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 9),
            marker.centerYAnchor.constraint(equalTo: centerYAnchor),
            marker.widthAnchor.constraint(equalToConstant: 11),
            shortcutLabel.leadingAnchor.constraint(equalTo: marker.trailingAnchor, constant: 6),
            shortcutLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            shortcutLabel.widthAnchor.constraint(equalToConstant: 13),
            titleLabel.leadingAnchor.constraint(equalTo: shortcutLabel.trailingAnchor, constant: 3),
            titleLeadingToTrailing,
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 3),
            trailingButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -5),
            trailingButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            trailingButtonWidth,
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 0),
            detailLabel.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -2),
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
        trailingButtonWidth.constant = showsTrailing ? 22 : 0
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
        // NSTableView reuses cells. Always clear the text before applying an
        // icon so a previous text-button configuration can never leak into a
        // Workspace close control.
        trailingButton.title = ""

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
            trailingButton.imagePosition = .noImage
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
