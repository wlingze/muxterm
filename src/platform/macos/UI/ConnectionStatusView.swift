import AppKit
import MuxtermChrome

/// 底部一行状态：连接态 + tabs/panes/活跃 pane；输出片段仅供 XCUITest AX。
/// 一行连接状态（连接状态 / tabs / panes / 最近输出片段）。
/// 与主 status bar（`StatusBarView`）分开：这是 muxterm 自己的状态摘要行。
final class ConnectionStatusView: NSView {
    private let label = NSTextField(labelWithString: "")
    private let outputProbe = NSTextField(labelWithString: "")
    private let edgeLine = CALayer()
    private var baseText = ""
    private var errorText: String?
    private var layoutSyncMessage = ""

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layoutSyncMessage = MuxtermI18n.shared.tr(.layoutSyncing)
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
        outputProbe.setAccessibilityLabel(MuxtermI18n.shared.tr(.terminalOutputSnippet))
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
            status: localizedStatus(snapshot.status),
            tabCount: snapshot.tabs.count,
            paneCount: snapshot.panes.count,
            activePane: snapshot.activePane,
            tabsLabel: MuxtermI18n.shared.tr(.tabs),
            panesLabel: MuxtermI18n.shared.tr(.panes),
            paneLabel: MuxtermI18n.shared.tr(.pane)
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

    func refreshLocalization() {
        let wasShowingLayoutSync = errorText == layoutSyncMessage
        layoutSyncMessage = MuxtermI18n.shared.tr(.layoutSyncing)
        outputProbe.setAccessibilityLabel(MuxtermI18n.shared.tr(.terminalOutputSnippet))
        if wasShowingLayoutSync {
            errorText = layoutSyncMessage
        }
        renderText()
    }

    private func renderText() {
        let text = errorText ?? baseText
        label.stringValue = text
        label.setAccessibilityValue(text)
        label.textColor = errorText == nil ? NSColor.tertiaryLabelColor : NSColor.systemRed
    }

    private func localizedStatus(_ status: String) -> String {
        let key: MuxtermTextKey
        switch status {
        case "connected": key = .statusConnected
        case "connecting": key = .statusConnecting
        case "disconnected": key = .statusDisconnected
        case "error": key = .statusError
        case "exited": key = .statusExited
        default: key = .statusUnknown
        }
        return MuxtermI18n.shared.tr(key)
    }
}
