import AppKit
import MuxtermChrome

/// 注意力面板：列出 blocked / done 的 pane（对应 Linux `linux_attention_e2e`）。
///
/// 红点点击打开；回车/双击命中行 → 切到对应 tab + pane。
final class AttentionPanelController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    var onJump: ((UInt32?, UInt32, UInt64, String) -> Void)? // (tabId, paneId, seq, query)

    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = NSScrollView()
    private let peekContainer = NSView()
    private var peekView: MuxTerminalView?
    private var rows: [AttentionRow] = []
    private weak var ownerWindow: NSWindow?
    private let snapshot: () -> AttentionSnapshot?
    private let paneOutput: (UInt32) -> Data
    private let sendInput: (UInt32, Data) -> Void

    init(
        ownerWindow: NSWindow?,
        snapshot: @escaping () -> AttentionSnapshot?,
        paneOutput: @escaping (UInt32) -> Data,
        sendInput: @escaping (UInt32, Data) -> Void
    ) {
        self.ownerWindow = ownerWindow
        self.snapshot = snapshot
        self.paneOutput = paneOutput
        self.sendInput = sendInput

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

        // 选中行后显示该 pane 的小终端（对标 Linux `muxterm-attention-peek`）。
        peekContainer.translatesAutoresizingMaskIntoConstraints = false
        peekContainer.wantsLayer = true
        peekContainer.layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        peekContainer.isHidden = true
        root.addSubview(peekContainer)

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
            scrollView.heightAnchor.constraint(equalToConstant: 180),

            peekContainer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            peekContainer.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            peekContainer.topAnchor.constraint(equalTo: scrollView.bottomAnchor, constant: 8),
            peekContainer.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
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
        onJump?(nil, row.pane.paneId, 0, "")
        dismiss()
    }

    /// 选中行 → 填充 peek 小终端（该 pane 的最近输出）。
    private func updatePeek() {
        guard table.selectedRow >= 0, table.selectedRow < rows.count else {
            peekView?.removeFromSuperview()
            peekView = nil
            peekContainer.isHidden = true
            return
        }
        let paneId = rows[table.selectedRow].pane.paneId
        if peekView?.paneId != paneId {
            peekView?.removeFromSuperview()
            let view = MuxTerminalView(paneId: paneId, frame: .zero)
            view.setAccessibilityIdentifier("muxterm.attention.peek")
            view.inputHandler = self
            view.translatesAutoresizingMaskIntoConstraints = false
            peekContainer.addSubview(view)
            NSLayoutConstraint.activate([
                view.leadingAnchor.constraint(equalTo: peekContainer.leadingAnchor),
                view.trailingAnchor.constraint(equalTo: peekContainer.trailingAnchor),
                view.topAnchor.constraint(equalTo: peekContainer.topAnchor),
                view.bottomAnchor.constraint(equalTo: peekContainer.bottomAnchor),
            ])
            peekView = view
        }
        peekContainer.isHidden = false
        peekContainer.layoutSubtreeIfNeeded()
        peekView?.layoutSubtreeIfNeeded()
        _ = peekView?.syncSizeToPty(notifyResize: false)
        let data = paneOutput(paneId)
        if !data.isEmpty {
            peekView?.feedOutput(data, isSnapshot: true)
        }
    }

    // MARK: - NSSearchFieldDelegate

    func controlTextDidChange(_ obj: Notification) {
        reload()
    }

    // MARK: - NSTableViewDataSource / Delegate

    func tableViewSelectionDidChange(_ notification: Notification) {
        updatePeek()
    }

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
        cell.setAccessibilityIdentifier("muxterm.attention.hit-\(row)")
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

    func testRowCount() -> Int {
        rows.count
    }

    func testActivateFirstRow() {
        guard !rows.isEmpty else { return }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        activateSelected()
    }

    /// 只选中、不跳转（给 peek 填充）。
    func testSelectFirstRow() {
        guard !rows.isEmpty else { return }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        table.window?.makeFirstResponder(table)
    }

    func testPeekView() -> NSView? {
        window?.contentView?.findSubview { view in
            view.accessibilityIdentifier() == "muxterm.attention.peek"
        }
    }

    func testPeekText() -> String {
        if let term = testPeekView() as? MuxTerminalView {
            return term.visibleScreenText()
        }
        return ""
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

extension AttentionPanelController: TerminalInputHandler {
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        sendInput(view.paneId, Data(data))
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {
        // peek 小终端不写回 tmux 尺寸。
    }
}
