import AppKit
import MuxtermChrome

/// 统一状态栏：tab 列表 + SSH 连接状态 + 流量监控 + tmux status（left/right）。
///
/// 一个 bar 装下全部：tab（tmux 窗口列表或本地 tab 列表）在中间，
/// 左侧 SSH 连接状态 + 流量速率，右侧 tmux status-right 或 session 名。
/// 渲染纪律：高频输出时只更新流量数字，不重建 tab 列表。
final class StatusBarView: NSView {
    var onSelectWindow: ((UInt32) -> Void)?
    var onSelectTab: ((UInt32) -> Void)?
    var onNewTab: (() -> Void)?
    /// 提醒位点击（文档 §B.1：与 QuickConnect 入口同一位置）。
    var onAttentionClick: (() -> Void)?
    /// status bar 模式：tmux = 有 tmux 就跟 tmux 一致（默认）；
    /// theme = 只用 muxterm 主题黑白。
    var colorMode: StatusBarMode = .tmux

    private let sshStatusLabel = NSTextField(labelWithString: "")
    private let trafficLabel = NSTextField(labelWithString: "")
    private let leftLabel = NSTextField(labelWithString: "")
    private let rightLabel = NSTextField(labelWithString: "")
    /// tab / 窗口列表（中间区域）。
    private let tabStack = NSStackView()
    /// 「+」新建 tab 按钮。
    private let newTabButton = NSButton()
    /// 消息弹窗/提醒位：右侧固定预留的窄槽。
    private let attentionSlot = NSView()
    private let attentionDot = CALayer()
    private let attentionCountLabel = NSTextField(labelWithString: "")
    private let edgeLine = CALayer()
    private var justifyConstraints: [NSLayoutConstraint] = []
    private var heightConstraint: NSLayoutConstraint!
    /// 最近一次 tmux 快照与样式基准。
    private var lastTmuxSnapshot: StatusBarSnapshot?
    private var lastBase = StatusBarTextStyle.default
    private var lastLeftStyle = "default"
    private var lastRightStyle = "default"
    private var lastPlainForeground: NSColor?
    /// 当前 tab 列表（供非 tmux status 模式渲染）。
    private var currentTabs: [Tab] = []
    /// tmux status 是否启用：启用时 tab 列表从 tmux 窗口列表渲染，
    /// 否则从 FrameSnapshot 的 tab 列表渲染。
    private var tmuxStatusEnabled = false
    private var edgeAtBottom = true

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        heightConstraint = heightAnchor.constraint(equalToConstant: FlatChrome.tabBarHeight)
        setAccessibilityIdentifier("muxterm.statusBar")

        edgeLine.backgroundColor = NSColor.separatorColor.cgColor
        layer?.addSublayer(edgeLine)

        // SSH 连接状态（左侧固定）：显示 backend 类型 + host + 连接状态。
        sshStatusLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        sshStatusLabel.lineBreakMode = .byTruncatingTail
        sshStatusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        sshStatusLabel.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        sshStatusLabel.textColor = NSColor.tertiaryLabelColor

        // 流量监控（SSH 状态右侧）：显示实时下行速率。
        trafficLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        trafficLabel.lineBreakMode = .byTruncatingTail
        trafficLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        trafficLabel.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        trafficLabel.textColor = NSColor.tertiaryLabelColor

        leftLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        leftLabel.lineBreakMode = .byTruncatingTail
        leftLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        rightLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        rightLabel.lineBreakMode = .byTruncatingHead
        rightLabel.alignment = .right
        rightLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        tabStack.orientation = .horizontal
        tabStack.alignment = .centerY
        tabStack.spacing = 2
        tabStack.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        tabStack.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        // 「+」新建 tab 按钮。
        newTabButton.title = "+"
        newTabButton.bezelStyle = .shadowlessSquare
        newTabButton.isBordered = false
        newTabButton.font = NSFont.systemFont(ofSize: 13, weight: .regular)
        newTabButton.contentTintColor = NSColor.secondaryLabelColor
        newTabButton.target = self
        newTabButton.action = #selector(newTabClicked)
        newTabButton.setAccessibilityIdentifier("muxterm.newTabButton")
        newTabButton.translatesAutoresizingMaskIntoConstraints = false

        for view in [sshStatusLabel, trafficLabel, leftLabel, tabStack, rightLabel, attentionSlot, newTabButton] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        // 提醒位。
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
        let click = NSClickGestureRecognizer(target: self, action: #selector(attentionClicked))
        attentionSlot.addGestureRecognizer(click)

        let leftMaxWidth = leftLabel.widthAnchor.constraint(
            lessThanOrEqualTo: widthAnchor,
            multiplier: StatusBarLayoutPolicy.sideMaxFraction
        )
        let rightMaxWidth = rightLabel.widthAnchor.constraint(
            lessThanOrEqualTo: widthAnchor,
            multiplier: StatusBarLayoutPolicy.sideMaxFraction
        )

        NSLayoutConstraint.activate([
            heightConstraint,

            // SSH 状态在最左侧。
            sshStatusLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            sshStatusLabel.centerYAnchor.constraint(equalTo: centerYAnchor),

            // 流量在 SSH 状态右侧。
            trafficLabel.leadingAnchor.constraint(equalTo: sshStatusLabel.trailingAnchor, constant: 8),
            trafficLabel.centerYAnchor.constraint(equalTo: centerYAnchor),

            // tmux status-left 在流量右侧（tmux 模式才显示）。
            leftLabel.leadingAnchor.constraint(equalTo: trafficLabel.trailingAnchor, constant: 8),
            leftLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            leftLabel.trailingAnchor.constraint(lessThanOrEqualTo: tabStack.leadingAnchor, constant: -8),

            // tab 列表居中。
            tabStack.centerYAnchor.constraint(equalTo: centerYAnchor),

            // 「+」按钮在 tab 列表右侧。
            newTabButton.leadingAnchor.constraint(equalTo: tabStack.trailingAnchor, constant: 4),
            newTabButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            newTabButton.widthAnchor.constraint(equalToConstant: FlatChrome.newTabButtonWidth),

            // tmux status-right 在「+」右侧。
            rightLabel.leadingAnchor.constraint(equalTo: newTabButton.trailingAnchor, constant: 4),
            rightLabel.trailingAnchor.constraint(equalTo: attentionSlot.leadingAnchor, constant: -4),
            rightLabel.centerYAnchor.constraint(equalTo: centerYAnchor),

            attentionSlot.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            attentionSlot.centerYAnchor.constraint(equalTo: centerYAnchor),
            attentionSlot.widthAnchor.constraint(equalToConstant: 22),

            attentionCountLabel.leadingAnchor.constraint(equalTo: attentionSlot.leadingAnchor),
            attentionCountLabel.trailingAnchor.constraint(equalTo: attentionSlot.trailingAnchor),
            attentionCountLabel.centerYAnchor.constraint(equalTo: attentionSlot.centerYAnchor),

            leftMaxWidth,
            rightMaxWidth,
        ])

        applyJustify("centre")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func layout() {
        super.layout()
        let y: CGFloat = edgeAtBottom ? 0 : bounds.height - 1
        edgeLine.frame = CGRect(x: 0, y: y, width: bounds.width, height: 1)
        attentionDot.frame = CGRect(
            x: attentionSlot.bounds.midX - 4,
            y: attentionSlot.bounds.midY - 4,
            width: 8,
            height: 8
        )
    }

    /// `edgeAtBottom == true` 时画底部分隔线（bar 在上）；否则画顶部。
    func setEdgeLineAtBottom(_ atBottom: Bool) {
        edgeAtBottom = atBottom
        needsLayout = true
    }

    // MARK: - Tab 列表更新

    /// 用 FrameSnapshot 的 tab 列表更新（非 tmux status 模式）。
    func updateTabs(_ tabs: [Tab]) {
        currentTabs = tabs
        guard !tmuxStatusEnabled else { return }
        rebuildTabButtons(tabs.map { TabBarItem(id: $0.id, title: tabTitle($0), active: $0.isActive) })
    }

    /// 应用 tmux status 快照：启用时 tab 列表从窗口列表渲染。
    func applyTmuxSnapshot(_ snapshot: StatusBarSnapshot?, enabled: Bool) {
        tmuxStatusEnabled = enabled
        let useTmuxColors = colorMode == .tmux
        if let snapshot, enabled {
            lastTmuxSnapshot = snapshot
            lastBase = StatusBarStyleParser.parse(style: snapshot.statusStyle)
            lastLeftStyle = snapshot.leftStyle
            lastRightStyle = snapshot.rightStyle
            lastPlainForeground = useTmuxColors ? nil : Self.themeForeground
            if useTmuxColors, let bg = lastBase.bg.map(Self.color) {
                layer?.backgroundColor = bg.cgColor
            } else {
                layer?.backgroundColor = Self.themeBackground.cgColor
            }
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
            // tab 列表从 tmux 窗口列表渲染。
            rebuildTabButtons(snapshot.windows.map { win in
                TabBarItem(id: win.windowId, title: win.text, active: win.current)
            })
        } else {
            leftLabel.isHidden = true
            rightLabel.isHidden = true
            layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
            // 回退到 FrameSnapshot tab 列表。
            rebuildTabButtons(currentTabs.map { TabBarItem(id: $0.id, title: tabTitle($0), active: $0.isActive) })
        }
        needsLayout = true
    }

    // MARK: - SSH 连接状态 + 流量监控

    /// 更新 SSH 连接状态 + 流量速率显示。
    func updateConnectionStatus(_ summary: (type: String, host: String?, status: String),
                                trafficRate: UInt64, totalBytes: UInt64) {
        // SSH 状态标签。
        let typeText: String
        switch summary.type {
        case "ssh": typeText = "SSH"
        case "tmux": typeText = "tmux"
        case "local": typeText = "local"
        case "daemon": typeText = "daemon"
        default: typeText = summary.type
        }
        let hostPart = summary.host.map { " \($0)" } ?? ""
        let statusColor: NSColor
        switch summary.status {
        case "connected": statusColor = NSColor.systemGreen
        case "connecting": statusColor = NSColor.systemYellow
        case "disconnected", "exited": statusColor = NSColor.systemRed
        default: statusColor = NSColor.tertiaryLabelColor
        }
        let sshText = "\(typeText)\(hostPart)"
        let attrSSH = NSMutableAttributedString(string: sshText, attributes: [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: NSColor.secondaryLabelColor,
        ])
        // 状态指示点。
        let dot = NSAttributedString(string: " ● ", attributes: [
            .font: NSFont.systemFont(ofSize: 11),
            .foregroundColor: statusColor,
        ])
        attrSSH.append(dot)
        sshStatusLabel.attributedStringValue = attrSSH
        sshStatusLabel.isHidden = false

        // 流量监控：显示实时下行速率 + 累计。
        trafficLabel.stringValue = formatTraffic(rate: trafficRate, total: totalBytes)
        trafficLabel.isHidden = summary.type == "local"
    }

    /// 格式化流量速率：bytes/s → KB/s / MB/s。
    private func formatTraffic(rate: UInt64, total: UInt64) -> String {
        func humanReadable(_ bytes: UInt64) -> String {
            if bytes < 1024 { return "\(bytes) B" }
            if bytes < 1024 * 1024 { return String(format: "%.1f KB", Double(bytes) / 1024) }
            return String(format: "%.1f MB", Double(bytes) / (1024 * 1024))
        }
        return "↓ \(humanReadable(rate))/s"
    }

    // MARK: - tab 列表重建

    private struct TabBarItem {
        let id: UInt32
        let title: String
        let active: Bool
    }

    private func tabTitle(_ tab: Tab) -> String {
        if tab.name.isEmpty { return "\(tab.id)" }
        return "\(tab.id):\(tab.name)"
    }

    private func rebuildTabButtons(_ items: [TabBarItem]) {
        tabStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for item in items {
            let button = NSButton(title: item.title, target: self, action: #selector(tabClicked(_:)))
            button.isBordered = false
            button.font = NSFont.monospacedDigitSystemFont(
                ofSize: 11,
                weight: item.active ? .semibold : .regular
            )
            button.tag = Int(item.id)
            button.lineBreakMode = .byTruncatingTail
            button.cell?.lineBreakMode = .byTruncatingTail
            button.cell?.truncatesLastVisibleLine = true
            button.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            button.contentTintColor = item.active ? NSColor.labelColor : NSColor.secondaryLabelColor
            button.wantsLayer = true
            if item.active {
                button.layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.12).cgColor
                button.layer?.cornerRadius = 3
            } else {
                button.layer?.backgroundColor = NSColor.clear.cgColor
            }
            button.setAccessibilityIdentifier("muxterm.tab.\(item.id)")
            tabStack.addArrangedSubview(button)
        }
        // 单 tab 时隐藏「+」按钮也保留，用户仍可新建。
        newTabButton.isHidden = false
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

    /// 前端驱动高亮：切 tab 时立即把高亮移到目标窗口。
    func markCurrentWindow(_ windowId: UInt32) {
        guard let snapshot = lastTmuxSnapshot else { return }
        let updated = snapshot.updatingCurrentWindow(windowId)
        guard updated.windows != snapshot.windows else { return }
        lastTmuxSnapshot = updated
        if tmuxStatusEnabled {
            rebuildTabButtons(updated.windows.map { win in
                TabBarItem(id: win.windowId, title: win.text, active: win.current)
            })
        }
    }

    /// 设置提醒位。
    func setAttention(_ attention: StatusBarAttention) {
        attentionDot.isHidden = !attention.isActive
        attentionCountLabel.isHidden = attention.count <= 1
        attentionCountLabel.stringValue = "\(attention.count)"
        attentionSlot.setAccessibilityValue(
            attention.isActive ? "\(attention.count)" : "0"
        )
        attentionSlot.needsLayout = true
    }

    /// 订阅推送：只更新 left/right 文本。
    func applySubscription(name: String, value: String) {
        guard lastTmuxSnapshot != nil else { return }
        switch name {
        case "muxterm.status-left":
            lastTmuxSnapshot?.left = value
            leftLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(
                    text: value,
                    base: merged(lastBase, lastLeftStyle)
                ),
                font: leftLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
        case "muxterm.status-right":
            lastTmuxSnapshot?.right = value
            rightLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(
                    text: value,
                    base: merged(lastBase, lastRightStyle)
                ),
                font: rightLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
        default:
            break
        }
    }

    func refreshLocalization() {
        attentionSlot.setAccessibilityLabel(MuxtermI18n.shared.tr(.statusAttention))
        newTabButton.toolTip = MuxtermI18n.shared.tr(.newTabTooltip)
    }

    @objc private func attentionClicked() {
        onAttentionClick?()
    }

    private func applyJustify(_ justify: String) {
        NSLayoutConstraint.deactivate(justifyConstraints)
        justifyConstraints.removeAll()
        switch justify {
        case "left":
            justifyConstraints = [
                tabStack.leadingAnchor.constraint(equalTo: leftLabel.trailingAnchor, constant: 8),
                tabStack.trailingAnchor.constraint(lessThanOrEqualTo: rightLabel.leadingAnchor, constant: -8),
            ]
        case "right":
            justifyConstraints = [
                tabStack.leadingAnchor.constraint(greaterThanOrEqualTo: leftLabel.trailingAnchor, constant: 8),
                tabStack.trailingAnchor.constraint(equalTo: rightLabel.leadingAnchor, constant: -8),
            ]
        default:
            justifyConstraints = [
                tabStack.centerXAnchor.constraint(equalTo: centerXAnchor),
                tabStack.leadingAnchor.constraint(greaterThanOrEqualTo: leftLabel.trailingAnchor, constant: 8),
                tabStack.trailingAnchor.constraint(lessThanOrEqualTo: rightLabel.leadingAnchor, constant: -8),
            ]
        }
        NSLayoutConstraint.activate(justifyConstraints)
        needsLayout = true
    }

    private func merged(_ base: StatusBarTextStyle, _ overrideStyle: String) -> StatusBarTextStyle {
        let style = StatusBarStyleParser.parse(style: overrideStyle)
        return StatusBarTextStyle(
            fg: style.fg ?? base.fg,
            bg: style.bg ?? base.bg,
            bold: style.bold || base.bold,
            reverse: style.reverse || base.reverse
        )
    }

    private static func attributed(
        _ segments: [StatusBarStyledSegment],
        font: NSFont,
        plainForeground: NSColor? = nil
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
            if style.reverse, plainForeground == nil {
                swap(&fg, &bg)
            }
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
        Self.color(
            StatusBarStyleParser.color(MuxtermTerminalColors.activePalette.fg)
                ?? StatusBarColor(red: 0, green: 0, blue: 0)
        )
    }

    private static var themeBackground: NSColor {
        Self.color(
            StatusBarStyleParser.color(MuxtermTerminalColors.activePalette.bg)
                ?? StatusBarColor(red: 1, green: 1, blue: 1)
        )
    }
}
