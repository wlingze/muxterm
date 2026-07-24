import AppKit

/// 底部状态栏：连接状态、pane 数、活跃 tab/pane；并暴露输出片段供 XCUITest。
final class StatusBarView: NSView {
    private let label = NSTextField(labelWithString: "")
    private let outputProbe = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        // 容器本身不当 accessibility 元素，否则子控件（含 outputSnippet）会被吞掉
        setAccessibilityElement(false)

        label.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        label.textColor = NSColor.secondaryLabelColor
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
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            outputProbe.leadingAnchor.constraint(equalTo: leadingAnchor),
            outputProbe.topAnchor.constraint(equalTo: topAnchor),
            outputProbe.widthAnchor.constraint(equalToConstant: 2),
            outputProbe.heightAnchor.constraint(equalToConstant: 2),
            heightAnchor.constraint(equalToConstant: 24),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func update(snapshot: FrameSnapshot) {
        let tabName = snapshot.tabs.first(where: { $0.id == snapshot.activeTab })?.name ?? "-"
        let text =
            "\(snapshot.status)  |  tabs: \(snapshot.tabs.count)  panes: \(snapshot.panes.count)  |  tab: \(tabName)  pane: @\(snapshot.activePane)"
        label.stringValue = text
        label.setAccessibilityValue(text)
    }

    func updateOutputSnippet(_ snippet: String) {
        outputProbe.stringValue = snippet
        outputProbe.setAccessibilityValue(snippet)
    }
}
