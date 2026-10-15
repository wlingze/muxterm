import AppKit
import MuxtermChrome

/// 快速浮层列表共用的上下导航。方向键与 Emacs 的 Ctrl-N/Ctrl-P
/// 使用同一条选择路径，避免每个 panel 对修饰键的处理逐渐分叉。
enum CompactPanelKeyNavigation {
    static func selectionOffset(for event: NSEvent) -> Int? {
        switch event.keyCode {
        case 125: return 1
        case 126: return -1
        default: break
        }

        let flags = event.modifierFlags
            .intersection(.deviceIndependentFlagsMask)
            .intersection([.command, .option, .control, .shift])
        guard flags == [.control] else { return nil }
        switch event.charactersIgnoringModifiers?.lowercased() {
        case "n": return 1
        case "p": return -1
        default:
            // 输入法未提供 characters 时，仍按物理 N/P 键识别。
            if event.keyCode == 45 { return 1 }
            if event.keyCode == 35 { return -1 }
            return nil
        }
    }
}

/// macOS 浮层的统一几何：优先使用紧凑尺寸，小窗口时留 12pt 四周边距。
enum CompactPanelLayout {
    static let edgeClearance: CGFloat = 24
    static let minimumSize = NSSize(width: 360, height: 240)

    static func contentSize(preferred: NSSize, available: NSSize?) -> NSSize {
        guard let available else { return preferred }
        func dimension(preferred: CGFloat, minimum: CGFloat, available: CGFloat) -> CGFloat {
            let limit = max(1, available - edgeClearance)
            return limit >= minimum
                ? max(minimum, min(preferred, limit))
                : limit
        }
        return NSSize(
            width: dimension(
                preferred: preferred.width,
                minimum: minimumSize.width,
                available: available.width
            ),
            height: dimension(
                preferred: preferred.height,
                minimum: minimumSize.height,
                available: available.height
            )
        )
    }

    static func prepare(_ panel: NSWindow, owner: NSWindow?, preferred: NSSize) {
        let available = owner?.contentView?.bounds.size
        panel.setContentSize(contentSize(preferred: preferred, available: available))
        guard let owner else {
            panel.center()
            return
        }
        var origin = NSPoint(
            x: owner.frame.midX - panel.frame.width / 2,
            y: owner.frame.midY - panel.frame.height / 2
        )
        if let visible = owner.screen?.visibleFrame ?? NSScreen.main?.visibleFrame {
            origin.x = min(max(origin.x, visible.minX + 8), visible.maxX - panel.frame.width - 8)
            origin.y = min(max(origin.y, visible.minY + 8), visible.maxY - panel.frame.height - 8)
        }
        panel.setFrameOrigin(origin)
    }

    static func bringForward(_ panel: NSWindow) {
        panel.makeKeyAndOrderFront(nil)
        if !NSApp.isActive {
            NSApp.activate(ignoringOtherApps: true)
        }
    }
}

enum PaletteCommand: Equatable {
    case local
    case ssh
    case newTab
    case quickConnect
    case searchPanes
    case renameTab
    case renameWorkspace
    case moveTabLeft
    case moveTabRight
    case switchTab(Int)
    case switchLastTab
    case previousTab
    case nextTab
    case movePaneToNewTab
    case splitHorizontal
    case splitVertical
    case nextPane
    case prevPane
    case previousCommand
    case nextCommand
    case closePane
    case closeTab
    case closeWindow
    case detach
    case language
    case theme
    case statusBarMode
    case tabBarTop
    case tabBarBottom
    case togglePaneFullscreen
    case toggleWindowFullscreen
    case toggleWindowZoom
    case openSettings
    case cycleLayout
    case increaseFontSize
    case decreaseFontSize
    case resetFontSize
    case quit
    case appInfo
}

enum PaletteItemKind: Equatable {
    case command(PaletteCommand)
    case session(target: ConnectionTarget, name: String)
    case host(SSHHostInfo)
    case newSession(target: ConnectionTarget)
    case language(MuxtermLanguage)
}

struct PaletteItem: Equatable {
    let title: String
    let detail: String
    let keywords: String
    let kind: PaletteItemKind

    var searchText: String {
        "\(title) \(detail) \(keywords)".lowercased()
    }

    /// 快速面板 Cmd-K 复用同一条命令，不另建一份目录。
    func quickPanelCommand() -> QuickPanelCommand? {
        guard case .command(let command) = kind else { return nil }
        return QuickPanelCommand(
            id: command.quickPanelCommandID,
            title: title,
            detail: detail,
            keywords: keywords,
            section: .global
        )
    }
}

extension PaletteCommand {
    /// 面板动作 id。`palette.*` 只是这条 macOS 命令的桥，执行仍走
    /// `handlePaletteSelection`。
    var quickPanelCommandID: String {
        switch self {
        case .local: return "palette.local"
        case .ssh: return "palette.ssh"
        case .newTab: return "palette.newTab"
        case .quickConnect: return "palette.quickConnect"
        case .searchPanes: return "palette.searchPanes"
        case .renameTab: return "palette.renameTab"
        case .renameWorkspace: return "palette.renameWorkspace"
        case .moveTabLeft: return "palette.moveTabLeft"
        case .moveTabRight: return "palette.moveTabRight"
        case .switchTab(let index): return "palette.switchTab.\(index)"
        case .switchLastTab: return "palette.switchLastTab"
        case .previousTab: return "palette.previousTab"
        case .nextTab: return "palette.nextTab"
        case .movePaneToNewTab: return "palette.movePaneToNewTab"
        case .splitHorizontal: return "palette.splitHorizontal"
        case .splitVertical: return "palette.splitVertical"
        case .nextPane: return "palette.nextPane"
        case .prevPane: return "palette.prevPane"
        case .previousCommand: return "palette.previousCommand"
        case .nextCommand: return "palette.nextCommand"
        case .closePane: return "palette.closePane"
        case .closeTab: return "palette.closeTab"
        case .closeWindow: return "palette.closeWindow"
        case .detach: return "palette.detach"
        case .language: return "palette.language"
        case .theme: return "palette.theme"
        case .statusBarMode: return "palette.statusBarMode"
        case .tabBarTop: return "palette.tabBarTop"
        case .tabBarBottom: return "palette.tabBarBottom"
        case .togglePaneFullscreen: return "palette.togglePaneFullscreen"
        case .toggleWindowFullscreen: return "palette.toggleWindowFullscreen"
        case .toggleWindowZoom: return "palette.toggleWindowZoom"
        case .openSettings: return "palette.openSettings"
        case .cycleLayout: return "palette.cycleLayout"
        case .increaseFontSize: return "palette.increaseFontSize"
        case .decreaseFontSize: return "palette.decreaseFontSize"
        case .resetFontSize: return "palette.resetFontSize"
        case .quit: return "palette.quit"
        case .appInfo: return "palette.appInfo"
        }
    }

    static func fromQuickPanelCommandID(_ id: String) -> PaletteCommand? {
        let switchPrefix = "palette.switchTab."
        if id.hasPrefix(switchPrefix),
           let index = Int(id.dropFirst(switchPrefix.count)),
           index > 0
        {
            return .switchTab(index)
        }
        switch id {
        case "palette.local": return .local
        case "palette.ssh": return .ssh
        case "palette.newTab": return .newTab
        case "palette.quickConnect": return .quickConnect
        case "palette.searchPanes": return .searchPanes
        case "palette.renameTab": return .renameTab
        case "palette.renameWorkspace": return .renameWorkspace
        case "palette.moveTabLeft": return .moveTabLeft
        case "palette.moveTabRight": return .moveTabRight
        case "palette.switchLastTab": return .switchLastTab
        case "palette.previousTab": return .previousTab
        case "palette.nextTab": return .nextTab
        case "palette.movePaneToNewTab": return .movePaneToNewTab
        case "palette.splitHorizontal": return .splitHorizontal
        case "palette.splitVertical": return .splitVertical
        case "palette.nextPane": return .nextPane
        case "palette.prevPane": return .prevPane
        case "palette.previousCommand": return .previousCommand
        case "palette.nextCommand": return .nextCommand
        case "palette.closePane": return .closePane
        case "palette.closeTab": return .closeTab
        case "palette.closeWindow": return .closeWindow
        case "palette.detach": return .detach
        case "palette.language": return .language
        case "palette.theme": return .theme
        case "palette.statusBarMode": return .statusBarMode
        case "palette.tabBarTop": return .tabBarTop
        case "palette.tabBarBottom": return .tabBarBottom
        case "palette.togglePaneFullscreen": return .togglePaneFullscreen
        case "palette.toggleWindowFullscreen": return .toggleWindowFullscreen
        case "palette.toggleWindowZoom": return .toggleWindowZoom
        case "palette.openSettings": return .openSettings
        case "palette.cycleLayout": return .cycleLayout
        case "palette.increaseFontSize": return .increaseFontSize
        case "palette.decreaseFontSize": return .decreaseFontSize
        case "palette.resetFontSize": return .resetFontSize
        case "palette.quit": return .quit
        case "palette.appInfo": return .appInfo
        default: return nil
        }
    }
}

/// VSCode 风格的单行输入 + 可过滤列表。
final class CommandPaletteController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    private static let preferredContentSize = NSSize(width: 600, height: 360)

    var onSelect: ((PaletteItem) -> Void)?
    var onDismissed: (() -> Void)?

    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = NSScrollView()
    private let emptyLabel = NSTextField(labelWithString: "")
    private var allItems: [PaletteItem] = []
    private var visibleItems: [PaletteItem] = []
    private var keyMonitor: Any?
    private weak var ownerWindow: NSWindow?

    init(ownerWindow: NSWindow?) {
        self.ownerWindow = ownerWindow

        let panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: Self.preferredContentSize),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.title = MuxtermI18n.shared.tr(.commandPalette)
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

    func present(items: [PaletteItem], placeholder: String? = nil) {
        allItems = items
        input.placeholderString = placeholder ?? MuxtermI18n.shared.tr(.commandPalettePlaceholder)
        input.stringValue = ""
        applyFilter()

        guard let window else { return }
        window.level = .floating
        CompactPanelLayout.prepare(window, owner: ownerWindow, preferred: Self.preferredContentSize)
        CompactPanelLayout.bringForward(window)
        window.makeFirstResponder(input)
    }

    func update(items: [PaletteItem], placeholder: String) {
        allItems = items
        input.placeholderString = placeholder
        input.stringValue = ""
        applyFilter()
        window?.makeFirstResponder(input)
    }

    /// 更新面板自身的可见 chrome；命令项由 MainWindow 在切换语言后重建。
    func refreshLocalization() {
        window?.title = MuxtermI18n.shared.tr(.commandPalette)
        emptyLabel.stringValue = MuxtermI18n.shared.tr(.commandPaletteNoResults)
        if allItems.isEmpty {
            input.placeholderString = MuxtermI18n.shared.tr(.commandPalettePlaceholder)
        }
    }

    func dismiss() {
        window?.orderOut(nil)
        ownerWindow?.makeKeyAndOrderFront(nil)
        onDismissed?()
    }

    // MARK: - View

    private func buildView() {
        guard let window, let content = window.contentView else { return }

        let root = NSView()
        root.translatesAutoresizingMaskIntoConstraints = false
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        content.addSubview(root)

        input.translatesAutoresizingMaskIntoConstraints = false
        input.font = NSFont.systemFont(ofSize: 14)
        input.controlSize = .large
        input.placeholderString = MuxtermI18n.shared.tr(.commandPalettePlaceholder)
        input.focusRingType = .none
        input.delegate = self
        input.setAccessibilityIdentifier("muxterm.commandPalette.input")
        root.addSubview(input)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("command"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 36
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.usesAlternatingRowBackgroundColors = false
        table.backgroundColor = .clear
        table.selectionHighlightStyle = .regular
        table.style = .plain
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(tableActivated)
        table.setAccessibilityIdentifier("muxterm.commandPalette.list")

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
        emptyLabel.stringValue = MuxtermI18n.shared.tr(.commandPaletteNoResults)
        emptyLabel.setAccessibilityIdentifier("muxterm.commandPalette.empty")
        emptyLabel.isHidden = true
        root.addSubview(emptyLabel)

        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),

            input.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 14),
            input.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -14),
            input.topAnchor.constraint(equalTo: root.topAnchor, constant: 12),
            input.heightAnchor.constraint(equalToConstant: 28),

            scrollView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: input.bottomAnchor, constant: 8),
            scrollView.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -6),

            emptyLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            emptyLabel.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            emptyLabel.centerYAnchor.constraint(equalTo: scrollView.centerYAnchor),
        ])
    }

    private func installKeyMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.window?.isKeyWindow == true else { return event }
            if let offset = CompactPanelKeyNavigation.selectionOffset(for: event) {
                self.selectRow(offset: offset)
                return nil
            }
            switch event.keyCode {
            case 53: // Escape
                self.dismiss()
                return nil
            case 36, 76: // Return / keypad Enter
                self.activateSelected()
                return nil
            default:
                return event
            }
        }
    }

    private func applyFilter() {
        let query = input.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        visibleItems = query.isEmpty
            ? allItems
            : allItems.filter { $0.searchText.contains(query) }
        table.reloadData()
        if !visibleItems.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            table.scrollRowToVisible(0)
        } else {
            table.deselectAll(nil)
        }
        emptyLabel.isHidden = !visibleItems.isEmpty
        scrollView.isHidden = visibleItems.isEmpty
    }

    private func selectRow(offset: Int) {
        guard !visibleItems.isEmpty else { return }
        let current = table.selectedRow >= 0 ? table.selectedRow : 0
        let count = visibleItems.count
        let next = ((current + offset) % count + count) % count
        table.selectRowIndexes(IndexSet(integer: next), byExtendingSelection: false)
        table.scrollRowToVisible(next)
    }

    private func activateSelected() {
        guard table.selectedRow >= 0, table.selectedRow < visibleItems.count else { return }
        onSelect?(visibleItems[table.selectedRow])
    }

    // MARK: - NSTextFieldDelegate

    func controlTextDidChange(_ obj: Notification) {
        applyFilter()
    }

    // MARK: - NSTableViewDataSource / Delegate

    func numberOfRows(in tableView: NSTableView) -> Int {
        visibleItems.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("PaletteCell")
        let cell = tableView.makeView(withIdentifier: id, owner: self) as? PaletteCellView
            ?? PaletteCellView(identifier: id)
        let item = visibleItems[row]
        cell.title = item.title
        cell.detail = item.detail
        return cell
    }

    @objc private func tableActivated() {
        activateSelected()
    }

    // MARK: - 测试钩子

    func testIsPresented() -> Bool {
        window?.isVisible == true
    }

    func testVisibleTitles() -> [String] {
        visibleItems.map(\.title)
    }

    func testSelect(matching needle: String) {
        let query = needle.lowercased()
        guard let index = visibleItems.firstIndex(where: {
            $0.title.lowercased().contains(query) || $0.keywords.lowercased().contains(query)
        }) else {
            return
        }
        table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        onSelect?(visibleItems[index])
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

    func testSetQuery(_ query: String) {
        input.stringValue = query
        applyFilter()
    }

    func testEmptyStateVisible() -> Bool {
        !emptyLabel.isHidden
    }

    func testEmptyStateText() -> String {
        emptyLabel.stringValue
    }
}

private final class PaletteCellView: NSTableCellView {
    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")

    var title: String = "" {
        didSet { titleLabel.stringValue = title }
    }
    var detail: String = "" {
        didSet { detailLabel.stringValue = detail }
    }

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = NSFont.systemFont(ofSize: 10.5)
        detailLabel.textColor = .secondaryLabelColor
        addSubview(titleLabel)
        addSubview(detailLabel)
        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            titleLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 3),
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 1),
            detailLabel.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -3),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }
}
