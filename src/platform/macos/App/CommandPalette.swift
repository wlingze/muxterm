import AppKit

enum PaletteCommand: Equatable {
    case local
    case ssh
    case newTab
    case renameTab
    case renameWorkspace
    case splitHorizontal
    case splitVertical
    case nextPane
    case prevPane
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
    var onSelect: ((PaletteItem) -> Void)?

    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = NSScrollView()
    private var allItems: [PaletteItem] = []
    private var visibleItems: [PaletteItem] = []
    private var keyMonitor: Any?
    private weak var ownerWindow: NSWindow?

    init(ownerWindow: NSWindow?) {
        self.ownerWindow = ownerWindow

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 430),
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
        if let ownerWindow {
            window.level = .floating
            window.center()
            // center() 已足够，但在多屏时让面板靠近当前主窗口中心。
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
        input.font = NSFont.systemFont(ofSize: 18)
        input.placeholderString = MuxtermI18n.shared.tr(.commandPalettePlaceholder)
        input.focusRingType = .none
        input.delegate = self
        input.setAccessibilityIdentifier("muxterm.commandPalette.input")
        root.addSubview(input)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("command"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 42
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.usesAlternatingRowBackgroundColors = false
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(tableActivated)
        table.setAccessibilityIdentifier("muxterm.commandPalette.list")

        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.documentView = table
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        root.addSubview(scrollView)

        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),

            input.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 18),
            input.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -18),
            input.topAnchor.constraint(equalTo: root.topAnchor, constant: 18),
            input.heightAnchor.constraint(equalToConstant: 34),

            scrollView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: input.bottomAnchor, constant: 12),
            scrollView.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
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
        }
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
        titleLabel.font = NSFont.systemFont(ofSize: 14, weight: .medium)
        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = NSFont.systemFont(ofSize: 11)
        detailLabel.textColor = .secondaryLabelColor
        addSubview(titleLabel)
        addSubview(detailLabel)
        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
            titleLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -18),
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 1),
            detailLabel.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -5),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }
}
