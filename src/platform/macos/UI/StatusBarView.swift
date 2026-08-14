import AppKit
import MuxtermChrome

/// muxterm status bar：left + 窗口列表 + right。
///
/// 连接控制模式会话时按兼容的 status 配置渲染（left/window/right 与样式），
/// 但这是 muxterm 自己的 status bar。
final class StatusBarView: NSView {
    var onSelectWindow: ((UInt32) -> Void)?
    /// 提醒位点击（文档 §B.1：与 QuickConnect 入口同一位置）。
    var onAttentionClick: (() -> Void)?
    /// status bar 模式：tmux = 有 tmux 就跟 tmux 一致（默认）；
    /// theme = 只用 muxterm 主题黑白。
    var colorMode: StatusBarMode = .tmux

    private let leftLabel = NSTextField(labelWithString: "")
    private let rightLabel = NSTextField(labelWithString: "")
    private let windowStack = NSStackView()
    /// 消息弹窗/提醒位：右侧固定预留的窄槽，平时空着，attention > 0 时变红点。
    private let attentionSlot = NSView()
    private let attentionDot = CALayer()
    private let attentionCountLabel = NSTextField(labelWithString: "")
    private var justifyConstraints: [NSLayoutConstraint] = []
    private var heightConstraint: NSLayoutConstraint!
    /// 最近一次快照与样式基准：订阅推送只更新 left/right 文本，不复建窗口列表。
    private var lastSnapshot: StatusBarSnapshot?
    private var lastBase = StatusBarTextStyle.default
    private var lastLeftStyle = "default"
    private var lastRightStyle = "default"
    private var lastPlainForeground: NSColor?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        heightConstraint = heightAnchor.constraint(equalToConstant: FlatChrome.tabBarHeight)

        leftLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        leftLabel.lineBreakMode = .byTruncatingTail
        leftLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        rightLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        rightLabel.lineBreakMode = .byTruncatingHead
        rightLabel.alignment = .right
        rightLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        windowStack.orientation = .horizontal
        windowStack.alignment = .centerY
        windowStack.spacing = 2
        windowStack.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        // 长窗口名/长左右段不能把整条 bar 撑出窗口：窗口列表允许压缩，
        // 每个窗口按钮按尾部截断（tmux 自己也会截断 status-left/right）。
        windowStack.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        for view in [leftLabel, windowStack, rightLabel, attentionSlot] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        // 提醒位：固定宽度常驻，给后续消息弹窗/通知红点预留位置。
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

        // 初始默认：居中。
        applyJustify("centre")
        // 左右段按比例封顶（tmux status-left/right-length 的等价物），
        // 窗口列表至少保留一块可见宽度（软约束，空间不足时优先压缩窗口按钮）。
        let leftMaxWidth = leftLabel.widthAnchor.constraint(
            lessThanOrEqualTo: widthAnchor,
            multiplier: StatusBarLayoutPolicy.sideMaxFraction
        )
        let rightMaxWidth = rightLabel.widthAnchor.constraint(
            lessThanOrEqualTo: widthAnchor,
            multiplier: StatusBarLayoutPolicy.sideMaxFraction
        )
        let windowMinWidth = windowStack.widthAnchor.constraint(
            greaterThanOrEqualTo: widthAnchor,
            multiplier: StatusBarLayoutPolicy.windowMinFraction
        )
        windowMinWidth.priority = .defaultHigh

        NSLayoutConstraint.activate([
            heightConstraint,
            leftLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            leftLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            leftLabel.trailingAnchor.constraint(lessThanOrEqualTo: windowStack.leadingAnchor, constant: -8),

            windowStack.centerYAnchor.constraint(equalTo: centerYAnchor),

            rightLabel.trailingAnchor.constraint(equalTo: attentionSlot.leadingAnchor, constant: -4),
            rightLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            rightLabel.leadingAnchor.constraint(greaterThanOrEqualTo: windowStack.trailingAnchor, constant: 8),

            attentionSlot.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            attentionSlot.centerYAnchor.constraint(equalTo: centerYAnchor),
            attentionSlot.widthAnchor.constraint(equalToConstant: 22),

            attentionCountLabel.leadingAnchor.constraint(equalTo: attentionSlot.leadingAnchor),
            attentionCountLabel.trailingAnchor.constraint(equalTo: attentionSlot.trailingAnchor),
            attentionCountLabel.centerYAnchor.constraint(equalTo: attentionSlot.centerYAnchor),

            leftMaxWidth,
            rightMaxWidth,
            windowMinWidth,
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func layout() {
        super.layout()
        // 红点始终在预留槽内居中；count 文本由 Auto Layout 铺满整个槽。
        attentionDot.frame = CGRect(
            x: attentionSlot.bounds.midX - 4,
            y: attentionSlot.bounds.midY - 4,
            width: 8,
            height: 8
        )
    }

    func apply(snapshot: StatusBarSnapshot) {
        let useTmuxColors = colorMode == .tmux
        let base = StatusBarStyleParser.parse(style: snapshot.statusStyle)
        lastSnapshot = snapshot
        lastBase = base
        lastLeftStyle = snapshot.leftStyle
        lastRightStyle = snapshot.rightStyle
        lastPlainForeground = useTmuxColors ? nil : Self.themeForeground
        if useTmuxColors, let bg = base.bg.map(Self.color) {
            layer?.backgroundColor = bg.cgColor
        } else {
            layer?.backgroundColor = Self.themeBackground.cgColor
        }
        let plainForeground = useTmuxColors ? nil : Self.themeForeground
        leftLabel.attributedStringValue = Self.attributed(
            StatusBarStyleParser.parseInline(text: snapshot.left, base: merged(base, snapshot.leftStyle)),
            font: leftLabel.font ?? NSFont.systemFont(ofSize: 11),
            plainForeground: plainForeground
        )
        rightLabel.attributedStringValue = Self.attributed(
            StatusBarStyleParser.parseInline(text: snapshot.right, base: merged(base, snapshot.rightStyle)),
            font: rightLabel.font ?? NSFont.systemFont(ofSize: 11),
            plainForeground: plainForeground
        )

        windowStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (i, win) in snapshot.windows.enumerated() {
            if i > 0 {
                let sep = NSTextField(labelWithString: snapshot.separator.isEmpty ? " " : snapshot.separator)
                sep.font = NSFont.systemFont(ofSize: 11)
                sep.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
                windowStack.addArrangedSubview(sep)
            }
            let button = NSButton(title: "", target: self, action: #selector(windowClicked(_:)))
            button.isBordered = false
            button.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: win.current ? .semibold : .regular)
            button.tag = Int(win.windowId)
            // 窗口多/名字长时按尾部截断，而不是把整条 bar 撑出窗口。
            button.lineBreakMode = .byTruncatingTail
            button.cell?.lineBreakMode = .byTruncatingTail
            button.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            let styleName = win.current ? snapshot.windowCurrentStyle : snapshot.windowStyle
            let inlineBase = StatusBarStyleParser.parse(style: styleName)
            button.attributedTitle = Self.attributed(
                StatusBarStyleParser.parseInline(text: win.text, base: inlineBase),
                font: button.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: plainForeground
            )
            // 当前窗口按 tmux window-status-current-style 的 bg 画整块高亮，
            // 切 tab 后立刻可辨（不能只靠文字颜色/粗体）。
            if useTmuxColors, let bg = inlineBase.bg {
                button.wantsLayer = true
                button.layer?.cornerRadius = 3
                button.layer?.backgroundColor = Self.color(bg).cgColor
            } else if !useTmuxColors, win.current {
                // GUI 黑白模式：当前窗口用主题色淡底高亮。
                button.wantsLayer = true
                button.layer?.cornerRadius = 3
                button.layer?.backgroundColor = Self.themeAccent.cgColor
            } else {
                button.layer?.backgroundColor = nil
            }
            button.setAccessibilityIdentifier("muxterm.statusWindow.\(win.index)")
            windowStack.addArrangedSubview(button)
        }
        applyJustify(snapshot.justify)
    }

    @objc private func windowClicked(_ sender: NSButton) {
        onSelectWindow?(UInt32(sender.tag))
    }

    /// 设置提醒位（文档 §B.1）：count > 0 时亮红点并显示计数。
    /// 消息弹窗/通知列表落地前，这个位置始终预留、不参与内容布局。
    func setAttention(_ attention: StatusBarAttention) {
        attentionDot.isHidden = !attention.isActive
        attentionCountLabel.isHidden = attention.count <= 1
        attentionCountLabel.stringValue = "\(attention.count)"
        attentionSlot.setAccessibilityValue(
            attention.isActive ? "\(attention.count)" : "0"
        )
        attentionSlot.needsLayout = true
    }

    /// 订阅推送（tmux `refresh-client -B` → `%subscription-changed`）：
    /// 只更新 left/right 文本，复用最近一次快照的样式，不重建窗口列表。
    func applySubscription(name: String, value: String) {
        guard lastSnapshot != nil else { return }
        switch name {
        case "muxterm.status-left":
            lastSnapshot?.left = value
            leftLabel.attributedStringValue = Self.attributed(
                StatusBarStyleParser.parseInline(
                    text: value,
                    base: merged(lastBase, lastLeftStyle)
                ),
                font: leftLabel.font ?? NSFont.systemFont(ofSize: 11),
                plainForeground: lastPlainForeground
            )
        case "muxterm.status-right":
            lastSnapshot?.right = value
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

    @objc private func attentionClicked() {
        onAttentionClick?()
    }

    /// 按 justify 切换窗口列表的位置：
    /// left 紧挨 left 文本之后，centre 居中，right 紧挨 right 文本之前，
    /// absolute-centre 与 centre 相同。
    private func applyJustify(_ justify: String) {
        NSLayoutConstraint.deactivate(justifyConstraints)
        justifyConstraints.removeAll()
        switch justify {
        case "left":
            justifyConstraints = [
                windowStack.leadingAnchor.constraint(equalTo: leftLabel.trailingAnchor, constant: 8),
                windowStack.trailingAnchor.constraint(lessThanOrEqualTo: rightLabel.leadingAnchor, constant: -8),
            ]
        case "right":
            justifyConstraints = [
                windowStack.leadingAnchor.constraint(greaterThanOrEqualTo: leftLabel.trailingAnchor, constant: 8),
                windowStack.trailingAnchor.constraint(equalTo: rightLabel.leadingAnchor, constant: -8),
            ]
        default:
            justifyConstraints = [
                windowStack.centerXAnchor.constraint(equalTo: centerXAnchor),
                windowStack.leadingAnchor.constraint(greaterThanOrEqualTo: leftLabel.trailingAnchor, constant: 8),
                windowStack.trailingAnchor.constraint(lessThanOrEqualTo: rightLabel.leadingAnchor, constant: -8),
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

    /// GUI 黑白模式的文字/背景（跟随 muxterm 主题）。
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

    private static var themeAccent: NSColor {
        Self.themeForeground.withAlphaComponent(0.12)
    }
}
