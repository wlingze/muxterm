import AppKit
import MuxtermChrome

/// 底部一行状态：连接态 + tabs/panes/活跃 pane；输出片段仅供 XCUITest AX。
final class StatusBarView: NSView {
    private let label = NSTextField(labelWithString: "")
    private let outputProbe = NSTextField(labelWithString: "")
    private let edgeLine = CALayer()
    private var baseText = ""
    private var errorText: String?
    private let layoutSyncMessage = "GUI 布局同步中：等待当前 tab 的 pane 布局"

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
        return nil
    }

    override func layout() {
        super.layout()
        edgeLine.frame = CGRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1)
    }

    func update(snapshot: FrameSnapshot) {
        baseText = FlatChrome.statusText(
            status: snapshot.status,
            tabCount: snapshot.tabs.count,
            paneCount: snapshot.panes.count,
            activePane: snapshot.activePane
        )
        renderText()
    }

    /// 将运行时错误显示在 GUI 状态栏，不让 UI 因异步快照问题崩溃。
    func showError(_ message: String) {
        errorText = message
        renderText()
    }

    /// 布局过渡态独立清理；不能清掉输入/连接等仍需用户看到的错误。
    func showLayoutSyncing() {
        errorText = layoutSyncMessage
        renderText()
    }

    func clearLayoutSyncError() {
        guard errorText == layoutSyncMessage else { return }
        errorText = nil
        renderText()
    }

    func updateOutputSnippet(_ snippet: String) {
        outputProbe.stringValue = snippet
        outputProbe.setAccessibilityValue(snippet)
    }

    private func renderText() {
        let text = errorText ?? baseText
        label.stringValue = text
        label.setAccessibilityValue(text)
        label.textColor = errorText == nil ? NSColor.tertiaryLabelColor : NSColor.systemRed
    }
}
