import AppKit
import MuxtermChrome

/// muxterm status bar：left + 窗口列表 + right。
///
/// 连接 tmux 时渲染的是 tmux 的 status 配置（left/window/right 与样式），
/// 但这是 muxterm 自己的 status bar，只做 tmux 兼容。
final class StatusBarView: NSView {
    var onSelectWindow: ((UInt32) -> Void)?

    private let leftLabel = NSTextField(labelWithString: "")
    private let rightLabel = NSTextField(labelWithString: "")
    private let windowStack = NSStackView()
    private var heightConstraint: NSLayoutConstraint!

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

        for view in [leftLabel, windowStack, rightLabel] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }

        NSLayoutConstraint.activate([
            heightConstraint,
            leftLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            leftLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            leftLabel.trailingAnchor.constraint(lessThanOrEqualTo: windowStack.leadingAnchor, constant: -8),

            windowStack.centerXAnchor.constraint(equalTo: centerXAnchor),
            windowStack.centerYAnchor.constraint(equalTo: centerYAnchor),

            rightLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            rightLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            rightLabel.leadingAnchor.constraint(greaterThanOrEqualTo: windowStack.trailingAnchor, constant: 8),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func apply(snapshot: StatusBarSnapshot) {
        let base = StatusBarStyleParser.parse(style: snapshot.statusStyle)
        if let bg = base.bg.map(Self.color) {
            layer?.backgroundColor = bg.cgColor
        } else {
            layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        }
        leftLabel.attributedStringValue = Self.attributed(
            StatusBarStyleParser.parseInline(text: snapshot.left, base: merged(base, snapshot.leftStyle)),
            font: leftLabel.font ?? NSFont.systemFont(ofSize: 11)
        )
        rightLabel.attributedStringValue = Self.attributed(
            StatusBarStyleParser.parseInline(text: snapshot.right, base: merged(base, snapshot.rightStyle)),
            font: rightLabel.font ?? NSFont.systemFont(ofSize: 11)
        )

        windowStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for (i, win) in snapshot.windows.enumerated() {
            if i > 0 {
                let sep = NSTextField(labelWithString: snapshot.separator.isEmpty ? " " : snapshot.separator)
                sep.font = NSFont.systemFont(ofSize: 11)
                windowStack.addArrangedSubview(sep)
            }
            let button = NSButton(title: "", target: self, action: #selector(windowClicked(_:)))
            button.isBordered = false
            button.font = NSFont.monospacedDigitSystemFont(ofSize: 11, weight: win.current ? .semibold : .regular)
            button.tag = Int(win.windowId)
            let styleName = win.current ? snapshot.windowCurrentStyle : snapshot.windowStyle
            let inlineBase = StatusBarStyleParser.parse(style: styleName)
            button.attributedTitle = Self.attributed(
                StatusBarStyleParser.parseInline(text: win.text, base: inlineBase),
                font: button.font ?? NSFont.systemFont(ofSize: 11)
            )
            button.setAccessibilityIdentifier("muxterm.tmuxWindow.\(win.index)")
            windowStack.addArrangedSubview(button)
        }
    }

    @objc private func windowClicked(_ sender: NSButton) {
        onSelectWindow?(UInt32(sender.tag))
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

    private static func attributed(_ segments: [StatusBarStyledSegment], font: NSFont) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for segment in segments {
            var attributes: [NSAttributedString.Key: Any] = [.font: font]
            let style = segment.style
            if style.bold {
                attributes[.font] = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
            }
            var fg = style.fg.map(color)
            var bg = style.bg.map(color)
            if style.reverse {
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
}
