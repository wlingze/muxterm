import AppKit
import MuxtermChrome

/// 根内容视图：终端区 + 底部统一 StatusBar（tab + 状态/通知/新建，一个 bar）。
///
/// 渲染纪律（文档 §2.15.2 追加 B）：输出直接渲染到末尾，不逐帧滚动刷新。
final class ContentView: NSView {
    let paneLayout: PaneLayoutView
    let statusBar = StatusBarView()
    /// 断线水印（W16b：tmux server 死后保留最后一帧 + 覆盖提示）。
    let disconnectOverlay = NSTextField(labelWithString: "")
    /// 回底按钮（W16a：滚离底部后显示，点击回到尾部）。
    let jumpLatestButton = NSButton()

    private var topConstraints: [NSLayoutConstraint] = []
    private var bottomConstraints: [NSLayoutConstraint] = []
    private var position: TabBarPosition = .bottom

    init(terminalManager: TerminalManager) {
        self.paneLayout = PaneLayoutView(terminalManager: terminalManager)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        paneLayout.translatesAutoresizingMaskIntoConstraints = false
        statusBar.translatesAutoresizingMaskIntoConstraints = false

        disconnectOverlay.translatesAutoresizingMaskIntoConstraints = false
        disconnectOverlay.font = NSFont.systemFont(ofSize: 18, weight: .semibold)
        disconnectOverlay.textColor = .secondaryLabelColor
        disconnectOverlay.alignment = .center
        disconnectOverlay.isHidden = true
        disconnectOverlay.setAccessibilityIdentifier("muxterm.disconnectOverlay")
        disconnectOverlay.setAccessibilityElement(true)

        jumpLatestButton.translatesAutoresizingMaskIntoConstraints = false
        jumpLatestButton.title = "↓"
        jumpLatestButton.bezelStyle = .rounded
        jumpLatestButton.isHidden = true
        jumpLatestButton.setAccessibilityIdentifier("muxterm.jumpLatest")
        jumpLatestButton.setAccessibilityElement(true)

        addSubview(paneLayout)
        addSubview(statusBar)
        addSubview(disconnectOverlay)
        addSubview(jumpLatestButton)

        NSLayoutConstraint.activate([
            disconnectOverlay.centerXAnchor.constraint(equalTo: centerXAnchor),
            disconnectOverlay.centerYAnchor.constraint(equalTo: centerYAnchor),
            jumpLatestButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            jumpLatestButton.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
            jumpLatestButton.widthAnchor.constraint(equalToConstant: 32),
            jumpLatestButton.heightAnchor.constraint(equalToConstant: 28),
        ])

        // 顶部：status | pane
        topConstraints = [
            statusBar.topAnchor.constraint(equalTo: topAnchor),
            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.topAnchor.constraint(equalTo: statusBar.bottomAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        // 底部（默认）：pane | status
        bottomConstraints = [
            paneLayout.topAnchor.constraint(equalTo: topAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        // 默认底部
        applyTabBarPosition(.bottom)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func applyTabBarPosition(_ position: TabBarPosition) {
        self.position = position
        NSLayoutConstraint.deactivate(topConstraints)
        NSLayoutConstraint.deactivate(bottomConstraints)
        switch position {
        case .top:
            NSLayoutConstraint.activate(topConstraints)
            statusBar.setEdgeLineAtBottom(true)
        case .bottom:
            NSLayoutConstraint.activate(bottomConstraints)
            statusBar.setEdgeLineAtBottom(false)
        }
        needsLayout = true
    }

    func applyStatusBar(_ snapshot: StatusBarSnapshot?) {
        let enabled = snapshot?.enabled == true
        statusBar.applyTmuxSnapshot(snapshot, enabled: enabled)
        needsLayout = true
    }

    func updateTabs(_ tabs: [Tab]) {
        statusBar.updateTabs(tabs)
    }

    func updateConnectionStatus(_ summary: (type: String, host: String?, status: String),
                                trafficRate: UInt64, totalBytes: UInt64) {
        statusBar.updateConnectionStatus(summary, trafficRate: trafficRate, totalBytes: totalBytes)
    }

    func refreshLocalization() {
        statusBar.refreshLocalization()
    }

    /// 断线水印：tmux server 死后保留最后一帧 + 覆盖提示。
    func setDisconnected(_ disconnected: Bool) {
        disconnectOverlay.isHidden = !disconnected
        disconnectOverlay.stringValue = MuxtermI18n.shared.tr(.statusDisconnected)
        needsLayout = true
    }

    /// 回底按钮：viewport 滚离底部时显示。
    func setJumpLatestVisible(_ visible: Bool) {
        jumpLatestButton.isHidden = !visible
        needsLayout = true
    }
}
