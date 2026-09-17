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
    /// 回底胶囊（愿景：滚离底部后显示「↓ 最新 · +N」）。
    let jumpLatestButton = NSButton()
    /// 最近一次离开 pane 时的行位置。
    let lastSeenButton = NSButton()
    /// OSC 133 命令轨（滚动条一侧覆盖层，绿成功 / 红失败）。
    let commandMarkRail = CommandMarkRailView()
    private var jumpLatestTrailing: NSLayoutConstraint?
    private var railWidthConstraint: NSLayoutConstraint?
    /// 正在打开的 Workspace 页面。只占终端区，不能盖住 tab/status bar。
    let connectProgressOverlay = WorkspaceOpeningView()
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

        replyOverlayContainer.translatesAutoresizingMaskIntoConstraints = false
        replyOverlayContainer.wantsLayer = true
        replyOverlayContainer.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        replyOverlayContainer.isHidden = true

        jumpLatestButton.translatesAutoresizingMaskIntoConstraints = false
        jumpLatestButton.title = JumpLatestCaption.title(unseenLines: 0, latestWord: "最新")
        jumpLatestButton.bezelStyle = .rounded
        jumpLatestButton.isBordered = false
        jumpLatestButton.wantsLayer = true
        jumpLatestButton.layer?.cornerRadius = 13
        jumpLatestButton.layer?.masksToBounds = true
        jumpLatestButton.font = NSFont.systemFont(ofSize: 12, weight: .semibold)
        jumpLatestButton.contentTintColor = .labelColor
        jumpLatestButton.focusRingType = .none
        jumpLatestButton.controlSize = .small
        jumpLatestButton.isHidden = true
        jumpLatestButton.setAccessibilityIdentifier("muxterm.jumpLatest")
        jumpLatestButton.setAccessibilityElement(true)
        jumpLatestButton.toolTip = "回到底部"

        lastSeenButton.translatesAutoresizingMaskIntoConstraints = false
        lastSeenButton.title = "上次看到这里"
        lastSeenButton.bezelStyle = .rounded
        lastSeenButton.isHidden = true
        lastSeenButton.setAccessibilityIdentifier("muxterm.lastSeen")
        lastSeenButton.setAccessibilityElement(true)
        lastSeenButton.toolTip = "跳回上次离开 pane 的位置"

        commandMarkRail.translatesAutoresizingMaskIntoConstraints = false
        commandMarkRail.isHidden = true
        commandMarkRail.onExpandedChange = { [weak self] expanded in
            self?.railWidthConstraint?.constant = CommandMarkRailLayout.width(expanded: expanded)
            self?.refreshJumpLatestTrailing()
        }

        addSubview(paneLayout)
        addSubview(statusBar)
        addSubview(disconnectOverlay)
        addSubview(lastSeenButton)
        addSubview(commandMarkRail)
        addSubview(jumpLatestButton)
        addSubview(connectProgressOverlay)
        addSubview(replyOverlayContainer)

        let railWidth = commandMarkRail.widthAnchor.constraint(
            equalToConstant: CommandMarkRailLayout.collapsedWidth
        )
        let jumpTrailing = jumpLatestButton.trailingAnchor.constraint(
            equalTo: paneLayout.trailingAnchor,
            constant: -12
        )
        railWidthConstraint = railWidth
        jumpLatestTrailing = jumpTrailing

        NSLayoutConstraint.activate([
            disconnectOverlay.centerXAnchor.constraint(equalTo: centerXAnchor),
            disconnectOverlay.centerYAnchor.constraint(equalTo: centerYAnchor),
            lastSeenButton.centerXAnchor.constraint(equalTo: paneLayout.centerXAnchor),
            lastSeenButton.topAnchor.constraint(equalTo: paneLayout.topAnchor, constant: 12),
            commandMarkRail.trailingAnchor.constraint(equalTo: paneLayout.trailingAnchor),
            commandMarkRail.topAnchor.constraint(equalTo: paneLayout.topAnchor, constant: 4),
            commandMarkRail.bottomAnchor.constraint(equalTo: paneLayout.bottomAnchor, constant: -4),
            railWidth,
            jumpTrailing,
            jumpLatestButton.bottomAnchor.constraint(equalTo: paneLayout.bottomAnchor, constant: -12),
            jumpLatestButton.heightAnchor.constraint(equalToConstant: 26),
            jumpLatestButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 72),
            connectProgressOverlay.leadingAnchor.constraint(equalTo: paneLayout.leadingAnchor),
            connectProgressOverlay.trailingAnchor.constraint(equalTo: paneLayout.trailingAnchor),
            connectProgressOverlay.topAnchor.constraint(equalTo: paneLayout.topAnchor),
            connectProgressOverlay.bottomAnchor.constraint(equalTo: paneLayout.bottomAnchor),
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

    /// 连接进度页：stage 为 nil 时隐藏。页面只覆盖 PaneGrid，因此等待时
    /// Workspace tab、侧栏入口与切换快捷键始终可用。
    func setConnectProgress(stage: ConnectProgressStage?, title: String? = nil) {
        guard let stage else {
            connectProgressOverlay.hide()
            return
        }
        connectProgressOverlay.show(stage: stage, title: title)
    }

    /// 断线水印：tmux server 死后保留最后一帧 + 覆盖提示。
    func setDisconnected(_ disconnected: Bool) {
        disconnectOverlay.isHidden = !disconnected
        disconnectOverlay.stringValue = MuxtermI18n.shared.tr(.statusDisconnected)
        needsLayout = true
    }

    /// 回底胶囊：viewport 滚离底部时显示；有未读行时显示 `↓ 最新 · +N`。
    func setJumpLatestVisible(_ visible: Bool, unseenLines: UInt32 = 0) {
        jumpLatestButton.isHidden = !visible
        jumpLatestButton.title = "  \(JumpLatestCaption.title(unseenLines: unseenLines, latestWord: "最新"))  "
        jumpLatestButton.toolTip = unseenLines > 0
            ? "回到底部（\(unseenLines) 行新输出）"
            : "回到底部"
        styleJumpLatestCapsule(emphasized: unseenLines > 0)
        refreshJumpLatestTrailing()
        needsLayout = true
    }

    func setLastSeenVisible(_ visible: Bool) {
        lastSeenButton.isHidden = !visible
        needsLayout = true
    }

    func setCommandMarks(_ marks: [CommandMarkTick], maxOffset: UInt32) {
        commandMarkRail.apply(marks: marks, maxOffset: maxOffset)
        if commandMarkRail.isHidden {
            railWidthConstraint?.constant = 0
        } else if !commandMarkRail.expanded {
            railWidthConstraint?.constant = CommandMarkRailLayout.collapsedWidth
        }
        refreshJumpLatestTrailing()
        needsLayout = true
    }

    private func refreshJumpLatestTrailing() {
        let railSpace: CGFloat = commandMarkRail.isHidden
            ? 0
            : CommandMarkRailLayout.width(expanded: commandMarkRail.expanded) + 6
        jumpLatestTrailing?.constant = -(12 + railSpace)
    }

    private func styleJumpLatestCapsule(emphasized: Bool) {
        let fill = emphasized
            ? NSColor.controlAccentColor.withAlphaComponent(0.22)
            : NSColor.labelColor.withAlphaComponent(0.08)
        jumpLatestButton.layer?.backgroundColor = fill.cgColor
        jumpLatestButton.layer?.borderWidth = 1
        jumpLatestButton.layer?.borderColor = NSColor.separatorColor.withAlphaComponent(0.7).cgColor
    }
}

/// iTerm2 风格的轻量 Workspace opening 页面。动画由系统 spinner 驱动，
/// 不在主线程另开刷新定时器。
final class WorkspaceOpeningView: NSView {
    private let spinner = NSProgressIndicator()
    private let titleLabel = NSTextField(labelWithString: "")
    private let stageLabel = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        translatesAutoresizingMaskIntoConstraints = false
        wantsLayer = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        isHidden = true
        setAccessibilityIdentifier(ConnectProgress.identifier)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)

        spinner.translatesAutoresizingMaskIntoConstraints = false
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isIndeterminate = true
        spinner.setAccessibilityIdentifier(ConnectProgress.spinnerIdentifier)

        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.font = NSFont.systemFont(ofSize: 14, weight: .medium)
        titleLabel.textColor = .labelColor
        titleLabel.alignment = .center
        titleLabel.lineBreakMode = .byTruncatingMiddle

        stageLabel.translatesAutoresizingMaskIntoConstraints = false
        stageLabel.font = NSFont.systemFont(ofSize: 11)
        stageLabel.textColor = .secondaryLabelColor
        stageLabel.alignment = .center

        let stack = NSStackView(views: [spinner, titleLabel, stageLabel])
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 7
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -24),
            titleLabel.widthAnchor.constraint(lessThanOrEqualToConstant: 360),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    func show(stage: ConnectProgressStage, title: String?) {
        titleLabel.stringValue = title?.trimmingCharacters(in: .whitespacesAndNewlines)
            .nonEmpty ?? "Opening workspace"
        stageLabel.stringValue = stage.rawValue
        setAccessibilityValue(ConnectProgress.accessibilityValue(stage: stage))
        toolTip = stage.rawValue
        isHidden = false
        spinner.startAnimation(nil)
    }

    func hide() {
        spinner.stopAnimation(nil)
        isHidden = true
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}
