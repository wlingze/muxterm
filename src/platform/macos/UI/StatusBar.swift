import AppKit

/// 底部状态栏：连接状态、pane 数、活跃 tab/pane；并暴露输出片段供 XCUITest。
final class StatusBarView: NSView {
    private let label = NSTextField(labelWithString: "")
    private let outputProbe = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        setAccessibilityIdentifier("muxterm.statusBar")
        setAccessibilityElement(true)
        setAccessibilityRole(.staticText)

        label.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        label.textColor = NSColor.secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        label.setAccessibilityIdentifier("muxterm.statusLabel")
        addSubview(label)

        // 隐藏探测字段：XCUITest 用 accessibilityValue 读最近输出
        outputProbe.isHidden = true
        outputProbe.setAccessibilityElement(true)
        outputProbe.setAccessibilityIdentifier("muxterm.outputSnippet")
        outputProbe.setAccessibilityLabel("Terminal Output Snippet")
        outputProbe.translatesAutoresizingMaskIntoConstraints = false
        addSubview(outputProbe)

        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
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
        setAccessibilityValue(text)
    }

    func updateOutputSnippet(_ snippet: String) {
        outputProbe.stringValue = snippet
        outputProbe.setAccessibilityValue(snippet)
    }
}
