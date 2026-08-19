import AppKit

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
    case increaseFontSize
    case decreaseFontSize
    case resetFontSize
    case quit
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
}

/// VSCode 风格的单行输入 + 可过滤列表。
final class CommandPaletteController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    private static let preferredContentSize = NSSize(width: 600, height: 360)

    var onSelect: ((PaletteItem) -> Void)?

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
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
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
        if let ownerWindow, let first = ownerWindow.contentView {
            ownerWindow.makeFirstResponder(first)
        }
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
            switch event.keyCode {
            case 53: // Escape
                self.dismiss()
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
