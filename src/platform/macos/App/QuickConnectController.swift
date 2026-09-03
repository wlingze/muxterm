import AppKit
import MuxtermChrome

/// QuickConnect 面板：Recent + Project 快速连接。
///
/// 每个目标一行：完整 name + 小色块徽章（蓝 Recent / 绿 Project），
/// 当前连接所在行用颜色高亮（不额外加 tag）。
/// 副行 `runtime @ transport`（ssh 显示名字），path 在第三行。
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
    private let scrollView = MuxtermFillWidthScrollView()
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
        QuickConnectTableLayout.fit(table)
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
        table.addTableColumn(column)
        QuickConnectTableLayout.configure(table, column: column)
        table.headerView = nil
        table.rowHeight = QuickTargetCellView.preferredRowHeight
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
        scrollView.hasHorizontalScroller = false
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
            cell.workspaceIndex = nil
            return cell
        case .newProject:
            let id = NSUserInterfaceItemIdentifier("QuickNew")
            let cell = tableView.makeView(withIdentifier: id, owner: self) as? QuickActionCellView
                ?? QuickActionCellView(identifier: id)
            cell.title = "＋ New Project"
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
enum QuickConnectItem {
    case target(TargetConfig, badges: [QuickBadge], isCurrent: Bool)
    case newProject
}

/// Quick Connect 的轻量动作行，与目标行共享 14pt 左边距。
final class QuickActionCellView: NSTableCellView {
    private let label = NSTextField(labelWithString: "")

    var title: String {
        get { label.stringValue }
        set { label.stringValue = newValue }
    }

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        label.translatesAutoresizingMaskIntoConstraints = false
        label.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -14),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }
}

/// 紧凑两行目标 cell：完整 name + 8pt 色块徽章，下一行 runtime/transport/path。
/// 名称绝不截断；徽章缩成小色块，不跟名字抢宽度。
final class QuickTargetCellView: NSTableCellView {
    static let preferredRowHeight: CGFloat = 46
    static let badgeDotSize: CGFloat = 8

    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")
    private let badgeStack = NSStackView()
    private let workspaceIndexLabel = NSTextField(labelWithString: "")

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

    /// Fixed opened-order shortcut shown in the Workspace panel. Project-only
    /// rows leave this empty.
    var workspaceIndex: Int? {
        didSet { updateLayout() }
    }

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        // 不要把 title 赋给 NSTableCellView.textField：AppKit 会按默认
        // image/text 模板重排，把名字压成一个字母。
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        titleLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        titleLabel.setContentHuggingPriority(.required, for: .horizontal)
        // `maximumNumberOfLines = 1` + truncatingTail 会把
        // preferredMaxLayoutWidth 锁成当前 bounds，启动时 0pt → 只剩一个字形。
        titleLabel.usesSingleLineMode = true
        titleLabel.maximumNumberOfLines = 0
        titleLabel.preferredMaxLayoutWidth = 0
        titleLabel.lineBreakMode = .byClipping
        titleLabel.cell?.wraps = false
        titleLabel.cell?.isScrollable = false
        titleLabel.allowsDefaultTighteningForTruncation = false
        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = NSFont.systemFont(ofSize: 11)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        detailLabel.lineBreakMode = .byTruncatingMiddle
        detailLabel.maximumNumberOfLines = 1
        workspaceIndexLabel.translatesAutoresizingMaskIntoConstraints = false
        workspaceIndexLabel.font = NSFont.systemFont(ofSize: 11, weight: .semibold)
        workspaceIndexLabel.textColor = .controlAccentColor
        workspaceIndexLabel.alignment = .center
        badgeStack.translatesAutoresizingMaskIntoConstraints = false
        badgeStack.orientation = .horizontal
        badgeStack.alignment = .centerY
        badgeStack.spacing = 4
        badgeStack.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        badgeStack.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        addSubview(titleLabel)
        addSubview(detailLabel)
        addSubview(badgeStack)
        addSubview(workspaceIndexLabel)
        NSLayoutConstraint.activate([
            workspaceIndexLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            workspaceIndexLabel.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            workspaceIndexLabel.widthAnchor.constraint(equalToConstant: 22),
            titleLabel.leadingAnchor.constraint(equalTo: workspaceIndexLabel.trailingAnchor, constant: 6),
            titleLabel.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            badgeStack.leadingAnchor.constraint(equalTo: titleLabel.trailingAnchor, constant: 6),
            badgeStack.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            detailLabel.leadingAnchor.constraint(equalTo: titleLabel.leadingAnchor),
            detailLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            detailLabel.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 2),
            detailLabel.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -5),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    private func updateLayout() {
        guard let config else { return }
        titleLabel.stringValue = config.name
        titleLabel.toolTip = config.name
        workspaceIndexLabel.stringValue = workspaceIndex.map(String.init) ?? ""
        workspaceIndexLabel.toolTip = workspaceIndex.map {
            "Workspace shortcut \($0)"
        }
        let path = config.path.trimmingCharacters(in: .whitespacesAndNewlines)
        detailLabel.stringValue = path.isEmpty
            ? QuickConnect.subtitle(for: config)
            : "\(QuickConnect.subtitle(for: config))  ·  \(path)"

        for view in badgeStack.arrangedSubviews {
            badgeStack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        for badge in badges {
            badgeStack.addArrangedSubview(QuickBadgeDotView(badge: badge))
        }
        updateHighlight()
    }

    private func updateHighlight() {
        wantsLayer = true
        layer?.backgroundColor = isCurrent
            ? NSColor.controlAccentColor.withAlphaComponent(0.18).cgColor
            : NSColor.clear.cgColor
    }

    func testTitleBoundsWidth() -> CGFloat {
        titleLabel.layoutSubtreeIfNeeded()
        return titleLabel.bounds.width
    }

    func testTitleTextWidth() -> CGFloat {
        ceil(titleLabel.attributedStringValue.size().width)
    }

    func testTitleIntrinsicWidth() -> CGFloat {
        titleLabel.intrinsicContentSize.width
    }

    func testTitleText() -> String {
        titleLabel.stringValue
    }

    func testIsCurrent() -> Bool {
        isCurrent
    }

    func testWorkspaceIndex() -> Int? {
        workspaceIndex
    }

    func testBadgeDotSizes() -> [CGSize] {
        badgeStack.layoutSubtreeIfNeeded()
        return badgeStack.arrangedSubviews.map { view in
            view.layoutSubtreeIfNeeded()
            let size = view.bounds.size
            return size.width > 0 ? size : view.fittingSize
        }
    }
}

/// Recent = 蓝，Project = 绿。固定 8pt，不占名字的宽度。
final class QuickBadgeDotView: NSView {
    private let fill: NSColor

    init(badge: QuickBadge) {
        switch badge {
        case .recent: fill = .systemBlue
        case .project: fill = .systemGreen
        }
        super.init(frame: NSRect(
            x: 0,
            y: 0,
            width: QuickTargetCellView.badgeDotSize,
            height: QuickTargetCellView.badgeDotSize
        ))
        translatesAutoresizingMaskIntoConstraints = false
        toolTip = badge.label
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel(badge.label)
        setContentHuggingPriority(.defaultHigh, for: .horizontal)
        setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: QuickTargetCellView.badgeDotSize),
            heightAnchor.constraint(equalToConstant: QuickTargetCellView.badgeDotSize),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: QuickTargetCellView.badgeDotSize, height: QuickTargetCellView.badgeDotSize)
    }

    override func draw(_ dirtyRect: NSRect) {
        fill.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 2, yRadius: 2).fill()
    }
}

/// 把 NSTableView 的列宽钉在 clip view 宽度上。
/// `sizeLastColumnToFit()` 只按 **table.frame** 算；documentView 默认只有
/// ~100pt，会把 archmini-home 挤成 "a"。每次 tile 时跟滚动区域对齐。
final class MuxtermFillWidthScrollView: NSScrollView {
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        hasHorizontalScroller = false
        autohidesScrollers = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func tile() {
        super.tile()
        guard let table = documentView as? NSTableView else { return }
        let width = max(1, contentView.bounds.width)
        var frame = table.frame
        if abs(frame.width - width) > 0.5 {
            frame.size.width = width
            table.frame = frame
        }
        if let column = table.tableColumns.first {
            let target = max(column.minWidth, width)
            if abs(column.width - target) > 0.5 {
                column.width = target
            }
        }
    }
}

/// NSTableView 默认列宽约 100pt；QuickConnect 行还要放徽章，
/// 不把列拉满面板的话名称会被压成 "a" / "p"。
enum QuickConnectTableLayout {
    static func configure(_ table: NSTableView, column: NSTableColumn) {
        column.minWidth = 240
        column.width = 640
        column.resizingMask = .autoresizingMask
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.autoresizingMask = [.width, .height]
    }

    static func fit(_ table: NSTableView) {
        table.sizeLastColumnToFit()
        table.enclosingScrollView?.tile()
    }
}
