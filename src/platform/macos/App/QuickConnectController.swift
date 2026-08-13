import AppKit
import MuxtermChrome

/// QuickConnect 面板：Recent + Project 快速连接。
///
/// 每个目标一行，主行 name + 小标记（Recent / Project，可同时显示），
/// 当前连接所在行用颜色高亮（不额外加 tag）。
/// 副行 `runtime @ transport`（ssh 显示名字），path 在第二行。
/// 上下选择，回车连接；双击 project 行编辑。Cmd-P 打开。
final class QuickConnectController: NSWindowController, NSSearchFieldDelegate,
    NSTableViewDataSource, NSTableViewDelegate
{
    var onConnect: ((TargetConfig) -> Void)?
    var onEditProject: ((TargetConfig) -> Void)?
    var onNewProject: (() -> Void)?
    /// 当前前台连接的目标；所在行会用颜色高亮。
    var currentConfig: TargetConfig?

    private let store: QuickConnectStore
    private let input = NSSearchField()
    private let table = NSTableView()
    private let scrollView = NSScrollView()
    private var allItems: [QuickConnectItem] = []
    private var visibleItems: [QuickConnectItem] = []
    private var keyMonitor: Any?
    private weak var ownerWindow: NSWindow?

    init(store: QuickConnectStore, ownerWindow: NSWindow?) {
        self.store = store
        self.ownerWindow = ownerWindow

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 430),
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
        if let ownerWindow, let first = ownerWindow.contentView {
            ownerWindow.makeFirstResponder(first)
        }
    }

    private func reload() {
        // 前 5 条 Recent（最新在前），再补 Project 独有的；按唯一 ID 去重。
        let currentId = currentConfig.map { QuickConnect.uniqueID(for: $0) }
        var items: [QuickConnectItem] = QuickConnect.entries(
            recents: store.recents,
            projects: store.projects
        ).map { entry in
            .target(
                entry.config,
                badges: entry.badges,
                isCurrent: currentId == QuickConnect.uniqueID(for: entry.config)
            )
        }
        items.append(.newProject)
        allItems = items
        applyFilter()
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
        input.placeholderString = "Quick Connect"
        input.focusRingType = .none
        input.delegate = self
        input.setAccessibilityIdentifier("muxterm.quickConnect.input")
        root.addSubview(input)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("quick"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 48
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.usesAlternatingRowBackgroundColors = false
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(tableActivated)
        table.doubleAction = #selector(tableDoubleActivated)
        table.setAccessibilityIdentifier("muxterm.quickConnect.list")

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
            : allItems.filter { item in
                switch item {
                case .target(let c, _, _): return QuickConnect.searchText(for: c).contains(query)
                case .newProject: return "new project 新建".contains(query)
                }
            }
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
        switch visibleItems[table.selectedRow] {
        case .target(let config, _, _):
            onConnect?(config)
        case .newProject:
            onNewProject?()
        }
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
    }

    @objc private func tableActivated() {
        activateSelected()
    }

    /// 双击目标行 → 打开配置窗口编辑；新建入口双击仍新建。
    @objc private func tableDoubleActivated() {
        let row = table.clickedRow
        guard row >= 0, row < visibleItems.count else { return }
        switch visibleItems[row] {
        case .target(let config, _, _):
            onEditProject?(config)
        default:
            activateSelected()
        }
    }
}

/// 面板条目：目标（带标记）/ 新建入口。
private enum QuickConnectItem {
    case target(TargetConfig, badges: [QuickBadge], isCurrent: Bool)
    case newProject
}

/// 两行目标 cell：主行 name + 小标记，副行 `runtime @ transport`，path 第二行。
private final class QuickTargetCellView: NSTableCellView {
    private let titleLabel = NSTextField(labelWithString: "")
    private let subtitleLabel = NSTextField(labelWithString: "")
    private let pathLabel = NSTextField(labelWithString: "")
    private let badgeStack = NSStackView()

    var config: TargetConfig? {
        didSet { updateLayout() }
    }

    var badges: [QuickBadge] = [] {
        didSet { updateLayout() }
    }

    /// 当前连接：整行用主题色淡底高亮。
    var isCurrent = false {
        didSet { updateHighlight() }
    }

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = NSFont.systemFont(ofSize: 14, weight: .medium)
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        subtitleLabel.translatesAutoresizingMaskIntoConstraints = false
        subtitleLabel.font = NSFont.systemFont(ofSize: 11)
        subtitleLabel.textColor = .secondaryLabelColor
        pathLabel.translatesAutoresizingMaskIntoConstraints = false
        pathLabel.font = NSFont.systemFont(ofSize: 11)
        pathLabel.textColor = .tertiaryLabelColor
        badgeStack.translatesAutoresizingMaskIntoConstraints = false
        badgeStack.orientation = .horizontal
        badgeStack.alignment = .centerY
        badgeStack.spacing = 4
        addSubview(titleLabel)
        addSubview(subtitleLabel)
        addSubview(pathLabel)
        addSubview(badgeStack)
        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
            titleLabel.centerYAnchor.constraint(equalTo: centerYAnchor, constant: -9),
            badgeStack.leadingAnchor.constraint(equalTo: titleLabel.trailingAnchor, constant: 6),
            badgeStack.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -18),
            badgeStack.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            subtitleLabel.leadingAnchor.constraint(equalTo: badgeStack.trailingAnchor, constant: 8),
            subtitleLabel.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -18),
            subtitleLabel.firstBaselineAnchor.constraint(equalTo: titleLabel.firstBaselineAnchor),
            pathLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            pathLabel.trailingAnchor.constraint(equalTo: titleLabel.trailingAnchor),
            pathLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 1),
            pathLabel.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -5),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    private func updateLayout() {
        guard let config else { return }
        titleLabel.stringValue = config.name
        subtitleLabel.stringValue = QuickConnect.subtitle(for: config)
        pathLabel.stringValue = config.path

        badgeStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for badge in badges {
            let label = NSTextField(labelWithString: badge.label.uppercased())
            label.font = NSFont.systemFont(ofSize: 9, weight: .semibold)
            label.textColor = .white
            label.alignment = .center
            label.translatesAutoresizingMaskIntoConstraints = false
            label.wantsLayer = true
            label.layer?.cornerRadius = 4
            label.layer?.masksToBounds = true
            switch badge {
            case .recent:
                label.layer?.backgroundColor = NSColor.systemBlue.cgColor
            case .project:
                label.layer?.backgroundColor = NSColor.systemGreen.cgColor
            }
            label.setContentHuggingPriority(.required, for: .horizontal)
            label.heightAnchor.constraint(equalToConstant: 16).isActive = true
            label.widthAnchor.constraint(greaterThanOrEqualToConstant: 42).isActive = true
            badgeStack.addArrangedSubview(label)
        }
        updateHighlight()
    }

    private func updateHighlight() {
        wantsLayer = true
        layer?.backgroundColor = isCurrent
            ? NSColor.controlAccentColor.withAlphaComponent(0.18).cgColor
            : NSColor.clear.cgColor
    }
}
