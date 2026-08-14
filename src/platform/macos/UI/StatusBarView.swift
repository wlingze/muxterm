import AppKit
import MuxtermChrome

/// 统一状态栏（tab + tmux status + 状态/通知/新建，一个 bar 全装下）。
///
/// 布局（从左到右）：
/// ```
/// [tab1][tab2]...  [tmux-left]  [tmux-right]  [●状态点][🔔通知][+新建]
/// ```
/// - tab 列表从左侧排过来（tmux 窗口列表 = tab，或本地 tab 列表）
/// - tmux status-left/right 在 tab 右侧（tmux 有时才显示）
/// - 最右侧三个图标：状态点（点击展开 debug 信息）、通知红点、新建 tab
///
/// 渲染纪律：高频输出时只更新状态点颜色，不重建 tab 列表。
final class StatusBarView: NSView {
    var onSelectWindow: ((UInt32) -> Void)?
    var onSelectTab: ((UInt32) -> Void)?
    var onNewTab: (() -> Void)?
    var onAttentionClick: (() -> Void)?
    var colorMode: StatusBarMode = .tmux

    // 左→右：tab 列表 → tmux-left → tmux-right → 状态点 → 通知 → 新建
    private let tabStack = NSStackView()
    private let leftLabel = NSTextField(labelWithString: "")
    private let rightLabel = NSTextField(labelWithString: "")
    private let statusDot = StatusDotButton()
    private let attentionSlot = NSView()
    private let attentionDot = CALayer()
    private let attentionCountLabel = NSTextField(labelWithString: "")
    private let newTabButton = NSButton()
    private let edgeLine = CALayer()

    // 状态点弹出框（点击展开 debug 信息）
    private var statusPopover: NSPopover?
    private var statusInfoView: NSTextField?

    private var justifyConstraints: [NSLayoutConstraint] = []
    private var heightConstraint: NSLayoutConstraint!
    private var lastTmuxSnapshot: StatusBarSnapshot?
    private var lastBase = StatusBarTextStyle.default
    private var lastLeftStyle = "default"
    private var lastRightStyle = "default"
    private var lastPlainForeground: NSColor?
    private var currentTabs: [Tab] = []
    private var tmuxStatusEnabled = false
    private var edgeAtBottom = false

    // debug / 状态信息（点击状态点时弹出显示）
    private var isDebug = false
    private var debugText = ""
    private var errorText: String?
    private var layoutSyncMessage = ""

    // 连接摘要 + 流量
    private var connectionSummary: (type: String, host: String?, status: String) = ("local", nil, "connected")
    private var trafficRate: UInt64 = 0
    private var totalBytes: UInt64 = 0

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        heightConstraint = heightAnchor.constraint(equalToConstant: FlatChrome.tabBarHeight)
        setAccessibilityIdentifier("muxterm.statusBar")

        edgeLine.backgroundColor = NSColor.separatorColor.cgColor
        layer?.addSublayer(edgeLine)

        // tab 列表
        tabStack.orientation = .horizontal
        tabStack.alignment = .centerY
        tabStack.spacing = 4
        tabStack.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        tabStack.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        // tmux status-left
        leftLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        leftLabel.lineBreakMode = .byTruncatingTail
        leftLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        leftLabel.isHidden = true

        // tmux status-right
        rightLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        rightLabel.lineBreakMode = .byTruncatingHead
        rightLabel.alignment = .right
        rightLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        rightLabel.isHidden = true

        // 状态点（绿/黄/红）—— 自身就是 NSButton，保证点击热区 18×18。
        statusDot.target = self
        statusDot.action = #selector(statusDotClicked)
        statusDot.toolTip = "Click for connection details"
        statusDot.translatesAutoresizingMaskIntoConstraints = false

        // 通知红点
        attentionSlot.wantsLayer = true
        attentionSlot.layer?.backgroundColor = NSColor.clear.cgColor
        attentionSlot.setAccessibilityIdentifier("muxterm.statusAttention")
        attentionSlot.setAccessibilityElement(true)
        attentionSlot.setAccessibilityRole(.button)
        attentionSlot.setAccessibilityLabel(MuxtermI18n.shared.tr(.statusAttention))
        attentionDot.frame = CGRect(x: 0, y: 0, width: 8, height: 8)
        attentionDot.cornerRadius = 4
        attentionDot.backgroundColor = NSColor.systemRed.cgColor
        attentionDot.isHidden = true
        attentionSlot.layer?.addSublayer(attentionDot)
        attentionCountLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .semibold)
        attentionCountLabel.textColor = NSColor.systemRed
        attentionCountLabel.alignment = .center
        attentionCountLabel.translatesAutoresizingMaskIntoConstraints = false
        attentionCountLabel.isHidden = true
        attentionSlot.addSubview(attentionCountLabel)
        let attClick = NSClickGestureRecognizer(target: self, action: #selector(attentionClicked))
        attentionSlot.addGestureRecognizer(attClick)

        // 「+」新建 tab
        newTabButton.title = "+"
        newTabButton.bezelStyle = .shadowlessSquare
        newTabButton.isBordered = false
        newTabButton.font = NSFont.systemFont(ofSize: 13, weight: .regular)
        newTabButton.contentTintColor = NSColor.secondaryLabelColor
        newTabButton.target = self
        newTabButton.action = #selector(newTabClicked)
        newTabButton.setAccessibilityIdentifier("muxterm.newTabButton")
        newTabButton.translatesAutoresizingMaskIntoConstraints = false

        for view in [tabStack, leftLabel, rightLabel, statusDot, attentionSlot, newTabButton] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        let leftMaxWidth = leftLabel.widthAnchor.constraint(
            lessThanOrEqualTo: widthAnchor, multiplier: StatusBarLayoutPolicy.sideMaxFraction
        )
        let rightMaxWidth = rightLabel.widthAnchor.constraint(
            lessThanOrEqualTo: widthAnchor, multiplier: StatusBarLayoutPolicy.sideMaxFraction
        )

        NSLayoutConstraint.activate([
            heightConstraint,

            // tab 列表从最左侧开始。
            tabStack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            tabStack.centerYAnchor.constraint(equalTo: centerYAnchor),

            // tmux-left 在 tab 右侧。
            leftLabel.leadingAnchor.constraint(equalTo: tabStack.trailingAnchor, constant: 8),
            leftLabel.centerYAnchor.constraint(equalTo: centerYAnchor),

            // tmux-right。
            rightLabel.leadingAnchor.constraint(greaterThanOrEqualTo: leftLabel.trailingAnchor, constant: 8),
            rightLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            rightLabel.trailingAnchor.constraint(lessThanOrEqualTo: statusDot.leadingAnchor, constant: -6),

            // 最右侧三个图标：状态点 → 通知 → 新建。
            newTabButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            newTabButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            newTabButton.widthAnchor.constraint(equalToConstant: FlatChrome.newTabButtonWidth),

            attentionSlot.trailingAnchor.constraint(equalTo: newTabButton.leadingAnchor, constant: -2),
            attentionSlot.centerYAnchor.constraint(equalTo: centerYAnchor),
            attentionSlot.widthAnchor.constraint(equalToConstant: 22),

            statusDot.trailingAnchor.constraint(equalTo: attentionSlot.leadingAnchor, constant: -2),
            statusDot.centerYAnchor.constraint(equalTo: centerYAnchor),
            statusDot.widthAnchor.constraint(equalToConstant: 18),
            statusDot.heightAnchor.constraint(equalToConstant: 18),

            attentionCountLabel.leadingAnchor.constraint(equalTo: attentionSlot.leadingAnchor),
            attentionCountLabel.trailingAnchor.constraint(equalTo: attentionSlot.trailingAnchor),
            attentionCountLabel.centerYAnchor.constraint(equalTo: attentionSlot.centerYAnchor),

            leftMaxWidth, rightMaxWidth,
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func layout() {
        super.layout()
        let y: CGFloat = edgeAtBottom ? bounds.height - 1 : 0
        edgeLine.frame = CGRect(x: 0, y: y, width: bounds.width, height: 1)
        attentionDot.frame = CGRect(
            x: attentionSlot.bounds.midX - 4, y: attentionSlot.bounds.midY - 4,
            width: 8, height: 8
        )
    }

    func setEdgeLineAtBottom(_ atBottom: Bool) {
        edgeAtBottom = atBottom
        needsLayout = true
    }

    // MARK: - Tab 列表

    func updateTabs(_ tabs: [Tab]) {
        currentTabs = tabs
        guard !tmuxStatusEnabled else { return }
        rebuildTabButtons(tabs.map { TabBarItem(id: $0.id, title: tabTitle($0), active: $0.isActive) })
    }

    func applyTmuxSnapshot(_ snapshot: StatusBarSnapshot?, enabled: Bool) {
        tmuxStatusEnabled = enabled
        let useTmuxColors = colorMode == .tmux
        if let snapshot, enabled {
            lastTmuxSnapshot = snapshot
            lastBase = StatusBarStyleParser.parse(style: snapshot.statusStyle)
            lastLeftStyle = snapshot.leftStyle
            lastRightStyle = snapshot.rightStyle
            lastPlainForeground = useTmuxColors ? nil : Self.themeForeground
            // 不用 tmux status bar 的背景色给整条 bar 上色——它通常是深灰色，
            // 会让 tab 文字和状态点看不清。bar 本身始终用原生窗口背景色，
            // tmux left/right 文字保留各自的 fg/bg 样式。
            layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
            leftLabel.isHidden = false
            rightLabel.isHidden = false
            leftLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(text: snapshot.left, base: merged(lastBase, snapshot.leftStyle)),
                font: leftLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
            rightLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(text: snapshot.right, base: merged(lastBase, snapshot.rightStyle)),
                font: rightLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
            rebuildTabButtons(snapshot.windows.map { win in
                TabBarItem(
                    id: win.windowId,
                    title: StatusBarTabTitle.display(index: win.index, name: win.name),
                    active: win.current
                )
            })
        } else {
            leftLabel.isHidden = true
            rightLabel.isHidden = true
            layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
            rebuildTabButtons(currentTabs.map { TabBarItem(id: $0.id, title: tabTitle($0), active: $0.isActive) })
        }
        needsLayout = true
    }

    // MARK: - 状态点 + 连接状态 + 流量

    func setDebug(_ debug: Bool) {
        isDebug = debug
    }

    func updateConnectionStatus(_ summary: (type: String, host: String?, status: String),
                                trafficRate: UInt64, totalBytes: UInt64) {
        connectionSummary = summary
        self.trafficRate = trafficRate
        self.totalBytes = totalBytes
        updateStatusDotColor()
    }

    /// 更新 debug 摘要文本（tabs/panes/pane:@N）。
    func updateDebugSnapshot(_ snapshot: FrameSnapshot) {
        if isDebug {
            debugText = FlatChrome.statusText(
                status: localizedStatus(snapshot.status),
                tabCount: snapshot.tabs.count,
                paneCount: snapshot.panes.count,
                activePane: snapshot.activePane,
                tabsLabel: MuxtermI18n.shared.tr(.tabs),
                panesLabel: MuxtermI18n.shared.tr(.panes),
                paneLabel: MuxtermI18n.shared.tr(.pane)
            )
        } else {
            debugText = ""
        }
    }

    func showError(_ message: String) {
        errorText = message
        updateStatusDotColor()
    }

    func clearError() {
        errorText = nil
        updateStatusDotColor()
    }

    func showLayoutSyncing() {
        errorText = MuxtermI18n.shared.tr(.layoutSyncing)
        updateStatusDotColor()
    }

    func clearLayoutSyncError() {
        let syncMsg = MuxtermI18n.shared.tr(.layoutSyncing)
        guard errorText == syncMsg else { return }
        errorText = nil
        updateStatusDotColor()
    }

    func updateOutputSnippet(_ snippet: String) {
        // 供 AX 查询，不影响视觉。
        setAccessibilityValue(snippet)
    }

    private func updateStatusDotColor() {
        let color: NSColor
        if let errorText {
            color = NSColor.systemRed
            statusDot.toolTip = "Error: \(errorText) — click for details"
        } else {
            switch connectionSummary.status {
            case "connected":
                // SSH 时按流量速率变色：高速=黄，否则=绿。
                if connectionSummary.type == "ssh" && trafficRate > 1_000_000 {
                    color = NSColor.systemYellow
                    statusDot.toolTip = "SSH \(connectionSummary.host ?? "") — high traffic ↓\(formatTraffic(trafficRate))/s — click for details"
                } else {
                    color = NSColor.systemGreen
                    statusDot.toolTip = "\(connectionSummary.type) \(connectionSummary.host ?? "") — connected — click for details"
                }
            case "connecting":
                color = NSColor.systemYellow
                statusDot.toolTip = "Connecting... — click for details"
            case "disconnected", "exited":
                color = NSColor.systemRed
                statusDot.toolTip = "\(connectionSummary.status) — click for details"
            default:
                color = NSColor.tertiaryLabelColor
            }
        }
        statusDot.setDotColor(color)
        statusDot.setAccessibilityLabel(statusDotAccessibilityLabel)
    }

    private var statusDotAccessibilityLabel: String {
        if let errorText { return "Status: error - \(errorText)" }
        let typeText: String
        switch connectionSummary.type {
        case "ssh": typeText = "SSH"
        case "tmux": typeText = "tmux"
        case "local": typeText = "local"
        default: typeText = connectionSummary.type
        }
        let host = connectionSummary.host.map { " \($0)" } ?? ""
        let traffic = connectionSummary.type == "ssh" ? " ↓\(formatTraffic(trafficRate))/s" : ""
        let debug = isDebug && !debugText.isEmpty ? "\n\(debugText)" : ""
        return "\(typeText)\(host) \(connectionSummary.status)\(traffic)\(debug)"
    }

    /// 点击状态点 → 弹出 debug 信息（连接状态 + SSH 流量 + debug tabs/panes）。
    @objc private func statusDotClicked() {
        // 如果已有弹出框，先关闭再开（避免重复）。
        statusPopover?.close()
        statusPopover = nil

        let info = statusDotAccessibilityLabel
        let popover = NSPopover()
        popover.behavior = .transient
        popover.animates = true

        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        // 多行文本：连接类型 + host + 状态 + 流量 + debug 信息 + 错误。
        let field = NSTextField(wrappingLabelWithString: info)
        field.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        field.textColor = NSColor.labelColor
        field.preferredMaxLayoutWidth = 280
        field.isEditable = false
        field.isSelectable = true
        field.drawsBackground = false
        field.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(field)

        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            field.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -12),
            field.topAnchor.constraint(equalTo: container.topAnchor, constant: 10),
            field.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -10),
            container.widthAnchor.constraint(equalToConstant: 320),
        ])

        let vc = NSViewController()
        vc.view = container
        popover.contentViewController = vc
        container.layoutSubtreeIfNeeded()
        let fitting = container.fittingSize
        popover.contentSize = NSSize(width: 320, height: max(48, fitting.height))

        // 底栏向上弹，顶栏向下弹，避免 popover 画到屏幕外看起来像「点了没反应」。
        let edge: NSRectEdge = edgeAtBottom ? .minY : .maxY
        popover.show(relativeTo: statusDot.bounds, of: statusDot, preferredEdge: edge)
        statusPopover = popover
    }

    private func formatTraffic(_ rate: UInt64) -> String {
        if rate < 1024 { return "\(rate)B" }
        if rate < 1024 * 1024 { return String(format: "%.1fKB", Double(rate) / 1024) }
        return String(format: "%.1fMB", Double(rate) / (1024 * 1024))
    }

    // MARK: - tab 重建

    private struct TabBarItem {
        let id: UInt32
        let title: String
        let active: Bool
    }

    private func tabTitle(_ tab: Tab) -> String {
        StatusBarTabTitle.display(index: tab.id, name: tab.name)
    }

    private func rebuildTabButtons(_ items: [TabBarItem]) {
        tabStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for item in items {
            let button = StatusTabButton()
            button.title = item.title
            button.tag = Int(item.id)
            button.target = self
            button.action = #selector(tabClicked(_:))
            button.setAccessibilityIdentifier("muxterm.tab.\(item.id)")
            button.isActiveTab = item.active
            button.applyStyle()
            tabStack.addArrangedSubview(button)
        }
    }

    @objc private func tabClicked(_ sender: NSButton) {
        if tmuxStatusEnabled {
            onSelectWindow?(UInt32(sender.tag))
        } else {
            onSelectTab?(UInt32(sender.tag))
        }
    }

    @objc private func newTabClicked() {
        onNewTab?()
    }

    func markCurrentWindow(_ windowId: UInt32) {
        guard let snapshot = lastTmuxSnapshot else { return }
        let updated = snapshot.updatingCurrentWindow(windowId)
        guard updated.windows != snapshot.windows else { return }
        lastTmuxSnapshot = updated
        if tmuxStatusEnabled {
            rebuildTabButtons(updated.windows.map { win in
                TabBarItem(
                    id: win.windowId,
                    title: StatusBarTabTitle.display(index: win.index, name: win.name),
                    active: win.current
                )
            })
        }
    }

    func setAttention(_ attention: StatusBarAttention) {
        attentionDot.isHidden = !attention.isActive
        attentionCountLabel.isHidden = attention.count <= 1
        attentionCountLabel.stringValue = "\(attention.count)"
        attentionSlot.setAccessibilityValue(attention.isActive ? "\(attention.count)" : "0")
        attentionSlot.needsLayout = true
    }

    func applySubscription(name: String, value: String) {
        guard lastTmuxSnapshot != nil else { return }
        switch name {
        case "muxterm.status-left":
            lastTmuxSnapshot?.left = value
            leftLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(text: value, base: merged(lastBase, lastLeftStyle)),
                font: leftLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
        case "muxterm.status-right":
            lastTmuxSnapshot?.right = value
            rightLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(text: value, base: merged(lastBase, lastRightStyle)),
                font: rightLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
        default: break
        }
    }

    func refreshLocalization() {
        attentionSlot.setAccessibilityLabel(MuxtermI18n.shared.tr(.statusAttention))
        newTabButton.toolTip = MuxtermI18n.shared.tr(.newTabTooltip)
        layoutSyncMessage = MuxtermI18n.shared.tr(.layoutSyncing)
    }

    @objc private func attentionClicked() {
        onAttentionClick?()
    }

    private func merged(_ base: StatusBarTextStyle, _ overrideStyle: String) -> StatusBarTextStyle {
        let style = StatusBarStyleParser.parse(style: overrideStyle)
        return StatusBarTextStyle(
            fg: style.fg ?? base.fg, bg: style.bg ?? base.bg,
            bold: style.bold || base.bold, reverse: style.reverse || base.reverse
        )
    }

    private static func attributed(
        _ segments: [StatusBarStyledSegment], font: NSFont, plainForeground: NSColor? = nil
    ) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for segment in segments {
            var attributes: [NSAttributedString.Key: Any] = [.font: font]
            let style = segment.style
            if style.bold {
                attributes[.font] = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
            }
            var fg = plainForeground ?? style.fg.map(color)
            var bg = plainForeground == nil ? style.bg.map(color) : nil
            if style.reverse, plainForeground == nil { swap(&fg, &bg) }
            if let fg { attributes[.foregroundColor] = fg }
            if let bg { attributes[.backgroundColor] = bg }
            out.append(NSAttributedString(string: segment.text, attributes: attributes))
        }
        return out
    }

    private static func color(_ c: StatusBarColor) -> NSColor {
        NSColor(srgbRed: c.red, green: c.green, blue: c.blue, alpha: 1)
    }
    private static var themeForeground: NSColor {
        Self.color(StatusBarStyleParser.color(MuxtermTerminalColors.activePalette.fg) ?? StatusBarColor(red: 0, green: 0, blue: 0))
    }
    private static var themeBackground: NSColor {
        Self.color(StatusBarStyleParser.color(MuxtermTerminalColors.activePalette.bg) ?? StatusBarColor(red: 1, green: 1, blue: 1))
    }

    private func localizedStatus(_ status: String) -> String {
        switch status {
        case "connected": return MuxtermI18n.shared.tr(.statusConnected)
        case "connecting": return MuxtermI18n.shared.tr(.statusConnecting)
        case "disconnected": return MuxtermI18n.shared.tr(.statusDisconnected)
        case "error": return MuxtermI18n.shared.tr(.statusError)
        case "exited": return MuxtermI18n.shared.tr(.statusExited)
        default: return MuxtermI18n.shared.tr(.statusUnknown)
        }
    }
}

/// 状态点：固定 18×18 可点击按钮，避免 NSView 高度塌缩导致点不到。
private final class StatusDotButton: NSButton {
    private let dotLayer = CALayer()

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        isBordered = false
        title = ""
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
        focusRingType = .none
        dotLayer.cornerRadius = 4
        dotLayer.backgroundColor = NSColor.systemGreen.cgColor
        layer?.addSublayer(dotLayer)
        setAccessibilityRole(.button)
        setAccessibilityLabel("Connection status")
        setAccessibilityIdentifier("muxterm.statusDot")
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        bounds.contains(point) ? self : nil
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: 18, height: 18)
    }

    override func layout() {
        super.layout()
        let side: CGFloat = 8
        dotLayer.frame = CGRect(
            x: (bounds.width - side) / 2,
            y: (bounds.height - side) / 2,
            width: side,
            height: side
        )
    }

    func setDotColor(_ color: NSColor) {
        dotLayer.backgroundColor = color.cgColor
        needsDisplay = true
    }
}

/// iTerm2 风格 GUI tab：圆角色块 + 系统字体，不用 tmux 格式串。
private final class StatusTabButton: NSButton {
    var isActiveTab = false {
        didSet { applyStyle() }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        bezelStyle = .shadowlessSquare
        isBordered = false
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.masksToBounds = true
        focusRingType = .none
        lineBreakMode = .byTruncatingTail
        cell?.lineBreakMode = .byTruncatingTail
        cell?.truncatesLastVisibleLine = true
        setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        setContentHuggingPriority(.defaultHigh, for: .horizontal)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override var intrinsicContentSize: NSSize {
        let size = super.intrinsicContentSize
        return NSSize(width: max(36, size.width + 16), height: 18)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        applyStyle()
    }

    func applyStyle() {
        let font = NSFont.systemFont(ofSize: 11, weight: isActiveTab ? .semibold : .regular)
        let fg = isActiveTab ? NSColor.labelColor : NSColor.secondaryLabelColor
        self.font = font
        attributedTitle = NSAttributedString(
            string: attributedTitle.string.isEmpty ? title : attributedTitle.string,
            attributes: [
                .font: font,
                .foregroundColor: fg,
            ]
        )
        layer?.backgroundColor = (isActiveTab
            ? NSColor.controlAccentColor.withAlphaComponent(0.22)
            : NSColor.labelColor.withAlphaComponent(0.06)
        ).cgColor
    }
}
