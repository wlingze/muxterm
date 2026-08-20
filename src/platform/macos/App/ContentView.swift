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
    /// 最近一次离开 pane 时的行位置。
    let lastSeenButton = NSButton()
    /// 当前 pane 最近一次成功/失败命令的刻度。
    let commandMarkOKButton = NSButton()
    let commandMarkFailButton = NSButton()
    /// 连接进度全窗口覆盖（W19-C：不是小对话框）。
    let connectProgressOverlay = NSTextField(labelWithString: "")
    /// 注意力 Cmd-Enter 的独立 replica overlay（W19-E）。
    let replyOverlayContainer = NSView()

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

        connectProgressOverlay.translatesAutoresizingMaskIntoConstraints = false
        connectProgressOverlay.font = NSFont.systemFont(ofSize: 20, weight: .medium)
        connectProgressOverlay.textColor = .labelColor
        connectProgressOverlay.alignment = .center
        connectProgressOverlay.wantsLayer = true
        connectProgressOverlay.layer?.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.92).cgColor
        connectProgressOverlay.isHidden = true
        connectProgressOverlay.setAccessibilityIdentifier(ConnectProgress.identifier)
        connectProgressOverlay.setAccessibilityElement(true)
        connectProgressOverlay.setAccessibilityRole(.staticText)

        replyOverlayContainer.translatesAutoresizingMaskIntoConstraints = false
        replyOverlayContainer.wantsLayer = true
        replyOverlayContainer.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        replyOverlayContainer.isHidden = true

        jumpLatestButton.translatesAutoresizingMaskIntoConstraints = false
        jumpLatestButton.title = "↓"
        jumpLatestButton.bezelStyle = .rounded
        jumpLatestButton.isHidden = true
        jumpLatestButton.setAccessibilityIdentifier("muxterm.jumpLatest")
        jumpLatestButton.setAccessibilityElement(true)

        lastSeenButton.translatesAutoresizingMaskIntoConstraints = false
        lastSeenButton.title = "上次看到这里"
        lastSeenButton.bezelStyle = .rounded
        lastSeenButton.isHidden = true
        lastSeenButton.setAccessibilityIdentifier("muxterm.lastSeen")
        lastSeenButton.setAccessibilityElement(true)
        lastSeenButton.toolTip = "跳回上次离开 pane 的位置"

        for button in [commandMarkOKButton, commandMarkFailButton] {
            button.translatesAutoresizingMaskIntoConstraints = false
            button.bezelStyle = .rounded
            button.isHidden = true
            button.setAccessibilityElement(true)
        }
        commandMarkOKButton.title = "✓"
        commandMarkOKButton.setAccessibilityIdentifier("muxterm.cmdMark.ok")
        commandMarkFailButton.title = "✗"
        commandMarkFailButton.setAccessibilityIdentifier("muxterm.cmdMark.fail")
        commandMarkOKButton.contentTintColor = .systemGreen
        commandMarkFailButton.contentTintColor = .systemRed

        addSubview(paneLayout)
        addSubview(statusBar)
        addSubview(disconnectOverlay)
        addSubview(lastSeenButton)
        addSubview(commandMarkOKButton)
        addSubview(commandMarkFailButton)
        addSubview(jumpLatestButton)
        addSubview(connectProgressOverlay)
        addSubview(replyOverlayContainer)

        NSLayoutConstraint.activate([
            disconnectOverlay.centerXAnchor.constraint(equalTo: centerXAnchor),
            disconnectOverlay.centerYAnchor.constraint(equalTo: centerYAnchor),
            lastSeenButton.centerXAnchor.constraint(equalTo: paneLayout.centerXAnchor),
            lastSeenButton.topAnchor.constraint(equalTo: paneLayout.topAnchor, constant: 12),
            commandMarkFailButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            commandMarkFailButton.topAnchor.constraint(equalTo: topAnchor, constant: 12),
            commandMarkOKButton.trailingAnchor.constraint(equalTo: commandMarkFailButton.leadingAnchor, constant: -4),
            commandMarkOKButton.topAnchor.constraint(equalTo: topAnchor, constant: 12),
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

    /// 连接进度覆盖层：stage 为 nil 时隐藏。
    /// 不用 Auto Layout 全约束（NSTextField 的 intrinsic 高度会压扁窗口），
    /// 手动铺满。
    func setConnectProgress(stage: ConnectProgressStage?) {
        guard let stage else {
            connectProgressOverlay.isHidden = true
            return
        }
        layoutSubtreeIfNeeded()
        connectProgressOverlay.frame = bounds
        connectProgressOverlay.isHidden = false
        connectProgressOverlay.stringValue = stage.rawValue
        connectProgressOverlay.setAccessibilityValue(ConnectProgress.accessibilityValue(stage: stage))
        connectProgressOverlay.toolTip = stage.rawValue
        needsLayout = true
    }

    /// 断线水印：tmux server 死后保留最后一帧 + 覆盖提示。
    func setDisconnected(_ disconnected: Bool) {
        disconnectOverlay.isHidden = !disconnected
        disconnectOverlay.stringValue = MuxtermI18n.shared.tr(.statusDisconnected)
        needsLayout = true
    }

    /// 回底按钮：viewport 滚离底部时显示；有未读行时显示 `↓ +N`。
    func setJumpLatestVisible(_ visible: Bool, unseenLines: UInt32 = 0) {
        jumpLatestButton.isHidden = !visible
        jumpLatestButton.title = unseenLines > 0 ? "↓ +\(unseenLines)" : "↓"
        jumpLatestButton.toolTip = unseenLines > 0
            ? "回到底部（\(unseenLines) 行新输出）"
            : "回到底部"
        needsLayout = true
    }

    func setLastSeenVisible(_ visible: Bool) {
        lastSeenButton.isHidden = !visible
        needsLayout = true
    }

    func setCommandMarks(ok: (command: String, exitCode: Int, offset: UInt32)?,
                         fail: (command: String, exitCode: Int, offset: UInt32)?) {
        commandMarkOKButton.isHidden = ok == nil
        commandMarkFailButton.isHidden = fail == nil
        if let ok {
            commandMarkOKButton.toolTip = "成功：\(ok.command)（退出码 \(ok.exitCode)）"
            commandMarkOKButton.setAccessibilityValue(ok.command)
        } else {
            commandMarkOKButton.toolTip = nil
            commandMarkOKButton.setAccessibilityValue(nil)
        }
        if let fail {
            commandMarkFailButton.toolTip = "失败：\(fail.command)（退出码 \(fail.exitCode)）"
            commandMarkFailButton.setAccessibilityValue(fail.command)
        } else {
            commandMarkFailButton.toolTip = nil
            commandMarkFailButton.setAccessibilityValue(nil)
        }
        needsLayout = true
    }
}
