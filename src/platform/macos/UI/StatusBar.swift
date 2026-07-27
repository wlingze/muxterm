import AppKit
import MuxtermChrome

/// 底部一行状态：连接态 + tabs/panes/活跃 pane；输出片段仅供 XCUITest AX。
final class StatusBarView: NSView {
    private let label = NSTextField(labelWithString: "")
    private let outputProbe = NSTextField(labelWithString: "")
    private let edgeLine = CALayer()

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        // 与窗口同色，不做独立面板底
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        setAccessibilityElement(false)

        edgeLine.backgroundColor = NSColor.separatorColor.cgColor
        layer?.addSublayer(edgeLine)

        label.font = NSFont.monospacedSystemFont(ofSize: 10, weight: .regular)
        label.textColor = NSColor.tertiaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        label.setAccessibilityIdentifier("muxterm.statusBar")
        label.setAccessibilityElement(true)
        label.setAccessibilityRole(.staticText)
        addSubview(label)

        // 视觉隐藏但仍暴露给 Accessibility（isHidden 会从 AX 树移除）
        outputProbe.alphaValue = 0.01
        outputProbe.isBordered = false
        outputProbe.isEditable = false
        outputProbe.isSelectable = false
        outputProbe.drawsBackground = false
        outputProbe.setAccessibilityElement(true)
        outputProbe.setAccessibilityIdentifier("muxterm.outputSnippet")
        outputProbe.setAccessibilityLabel("Terminal Output Snippet")
        outputProbe.translatesAutoresizingMaskIntoConstraints = false
        addSubview(outputProbe)

        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(
                equalTo: leadingAnchor,
                constant: FlatChrome.statusHorizontalInset
            ),
            label.trailingAnchor.constraint(
                lessThanOrEqualTo: trailingAnchor,
                constant: -FlatChrome.statusHorizontalInset
            ),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            outputProbe.leadingAnchor.constraint(equalTo: leadingAnchor),
            outputProbe.topAnchor.constraint(equalTo: topAnchor),
            outputProbe.widthAnchor.constraint(equalToConstant: 2),
            outputProbe.heightAnchor.constraint(equalToConstant: 2),
            heightAnchor.constraint(equalToConstant: FlatChrome.statusBarHeight),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func layout() {
        super.layout()
        edgeLine.frame = CGRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1)
    }

    func update(snapshot: FrameSnapshot) {
        let text = FlatChrome.statusText(
            status: snapshot.status,
            tabCount: snapshot.tabs.count,
            paneCount: snapshot.panes.count,
            activePane: snapshot.activePane
        )
        label.stringValue = text
        label.setAccessibilityValue(text)
    }

    func updateOutputSnippet(_ snippet: String) {
        outputProbe.stringValue = snippet
        outputProbe.setAccessibilityValue(snippet)
    }
}
