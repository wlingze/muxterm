import AppKit
import MuxtermChrome

/// 注意力面板：列出 blocked / done 的 pane（对应 Linux `linux_attention_e2e`）。
///
/// 红点点击打开；回车/双击命中行 → 切到对应 tab + pane。
final class AttentionPanelController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    var onJump: ((UInt32, UInt32) -> Void)? // (tabId, paneId)

    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = NSScrollView()
    private var rows: [AttentionRow] = []
    private weak var ownerWindow: NSWindow?
    private let snapshot: () -> AttentionSnapshot?

    init(ownerWindow: NSWindow?, snapshot: @escaping () -> AttentionSnapshot?) {
        self.ownerWindow = ownerWindow
        self.snapshot = snapshot

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 420),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.title = "Attention"
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
        window.makeFirstResponder(input)
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
        input.placeholderString = "Attention"
        input.focusRingType = .none
        input.delegate = self
        input.setAccessibilityIdentifier("muxterm.attention.input")
        root.addSubview(input)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("attention"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 44
        table.usesAlternatingRowBackgroundColors = false
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(tableActivated)
        table.doubleAction = #selector(tableDoubleActivated)
        table.setAccessibilityIdentifier("muxterm.attention.list")

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

    private func reload() {
        guard let snap = snapshot() else {
            rows = []
            table.reloadData()
            return
        }
        rows = AttentionList.rows(from: snap, query: input.stringValue)
        table.reloadData()
        if !rows.isEmpty {
            table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            table.scrollRowToVisible(0)
        }
    }

    private func activateSelected() {
        guard table.selectedRow >= 0, table.selectedRow < rows.count else { return }
        let row = rows[table.selectedRow]
        onJump?(0, row.pane.paneId)
        dismiss()
    }

    // MARK: - NSSearchFieldDelegate

    func controlTextDidChange(_ obj: Notification) {
        reload()
    }

    // MARK: - NSTableViewDataSource / Delegate

    func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
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
        let status = row.pane.status == .blocked ? "●" : "✓"
        let process = row.pane.processName ?? "?"
        label.stringValue = "\(status) \(row.workspaceId)  pane @\(row.pane.paneId)  \(process)\n\(row.pane.lastLine)"
        label.font = NSFont.systemFont(ofSize: 12)
        label.maximumNumberOfLines = 2
        return cell
    }

    @objc private func tableActivated() {
        activateSelected()
    }

    @objc private func tableDoubleActivated() {
        activateSelected()
    }
}
