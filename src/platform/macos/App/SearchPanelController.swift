import AppKit
import MuxtermChrome

/// 搜索面板：跨工作区 pane 文本搜索（对应 Linux `linux_search_e2e` / `linux_search_jump_e2e`）。
///
/// Cmd+Shift+F 打开；输入即搜；回车/双击命中行 → 切到对应 tab + pane。
final class SearchPanelController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    var onJump: ((UInt32?, UInt32, UInt64, String) -> Void)? // (tabId, paneId, seq, query)

    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = NSScrollView()
    private var hits: [SearchHit] = []
    private weak var ownerWindow: NSWindow?
    private let search: (String) -> [SearchHit]

    init(ownerWindow: NSWindow?, search: @escaping (String) -> [SearchHit]) {
        self.ownerWindow = ownerWindow
        self.search = search

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 420),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.title = "Search"
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.hasShadow = true

        super.init(window: panel)
        buildView()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func present() {
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
        window.makeFirstResponder(input)
        input.stringValue = ""
        runSearch()
    }

    func dismiss() {
        window?.orderOut(nil)
        ownerWindow?.makeKeyAndOrderFront(nil)
    }

    private func buildView() {
        guard let window, let content = window.contentView else { return }

        let root = NSView()
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)

        input.translatesAutoresizingMaskIntoConstraints = false
        input.font = NSFont.systemFont(ofSize: 18)
        input.placeholderString = "Search panes"
        input.focusRingType = .none
        input.delegate = self
        input.setAccessibilityIdentifier("muxterm.search.input")
        root.addSubview(input)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("search"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 40
        table.usesAlternatingRowBackgroundColors = false
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(tableActivated)
        table.doubleAction = #selector(tableDoubleActivated)
        table.setAccessibilityIdentifier("muxterm.search.list")

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

    private func runSearch() {
        let query = input.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        hits = query.isEmpty ? [] : search(query)
        table.reloadData()
        if !hits.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            table.scrollRowToVisible(0)
        }
    }

    private func activateSelected() {
        guard table.selectedRow >= 0, table.selectedRow < hits.count else { return }
        let hit = hits[table.selectedRow]
        onJump?(hit.tabId, hit.paneId, hit.seq, input.stringValue)
        dismiss()
    }

    // MARK: - NSSearchFieldDelegate

    func controlTextDidChange(_ obj: Notification) {
        runSearch()
    }

    // MARK: - NSTableViewDataSource / Delegate

    func numberOfRows(in tableView: NSTableView) -> Int {
        hits.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
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

    @objc private func tableActivated() {
        activateSelected()
    }

    @objc private func tableDoubleActivated() {
        activateSelected()
    }

    func testIsPresented() -> Bool {
        window?.isVisible == true
    }

    func testSetQuery(_ query: String) {
        input.stringValue = query
        runSearch()
    }

    func testActivateFirstHit() {
        guard !hits.isEmpty else { return }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        activateSelected()
    }

    func testHitCount() -> Int {
        hits.count
    }
}
