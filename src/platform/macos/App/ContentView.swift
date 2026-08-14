import AppKit
import MuxtermChrome

/// 根内容视图：主导终端区 + 一行统一 StatusBar（tab 内嵌在 statusbar 里）。
///
/// 渲染纪律（文档 §2.15.2 追加 B）：
/// - 输出直接渲染到末尾位置，不逐帧滚动刷新；
/// - statusbar 只有一个：tab 列表 + SSH 状态 + 流量监控，不分层。
final class ContentView: NSView {
    let paneLayout: PaneLayoutView
    /// 统一状态栏：tab 内嵌在 statusbar 里（tmux 窗口列表 = tab）。
    let statusBar = StatusBarView()
    /// 调试摘要行（--debug 时显示 connected/tabs/panes）。
    let connectionStatus = ConnectionStatusView()

    private var topConstraints: [NSLayoutConstraint] = []
    private var bottomConstraints: [NSLayoutConstraint] = []
    private var position: TabBarPosition = .top

    init(terminalManager: TerminalManager) {
        self.paneLayout = PaneLayoutView(terminalManager: terminalManager)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor

        paneLayout.translatesAutoresizingMaskIntoConstraints = false
        connectionStatus.translatesAutoresizingMaskIntoConstraints = false
        statusBar.translatesAutoresizingMaskIntoConstraints = false

        addSubview(paneLayout)
        addSubview(statusBar)
        addSubview(connectionStatus)

        // 顶部布局：status | pane | connectionStatus
        topConstraints = [
            statusBar.topAnchor.constraint(equalTo: topAnchor),
            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),

            paneLayout.topAnchor.constraint(equalTo: statusBar.bottomAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: connectionStatus.topAnchor),

            connectionStatus.leadingAnchor.constraint(equalTo: leadingAnchor),
            connectionStatus.trailingAnchor.constraint(equalTo: trailingAnchor),
            connectionStatus.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        // 底部布局：pane | status | connectionStatus
        bottomConstraints = [
            paneLayout.topAnchor.constraint(equalTo: topAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: statusBar.topAnchor),

            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: connectionStatus.topAnchor),

            connectionStatus.leadingAnchor.constraint(equalTo: leadingAnchor),
            connectionStatus.trailingAnchor.constraint(equalTo: trailingAnchor),
            connectionStatus.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        applyTabBarPosition(TabBarPosition.current)
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

    /// 应用 tmux status bar 快照：statusbar 始终可见，tab 列表从快照的
    /// 窗口列表渲染。nil 时用 FrameSnapshot 的 tab 列表渲染。
    func applyStatusBar(_ snapshot: StatusBarSnapshot?) {
        let enabled = snapshot?.enabled == true
        statusBar.applyTmuxSnapshot(snapshot, enabled: enabled)
        needsLayout = true
    }

    /// 用 FrameSnapshot 的 tab 列表更新 statusbar（非 tmux status 模式）。
    func updateTabs(_ tabs: [Tab]) {
        statusBar.updateTabs(tabs)
    }

    /// 更新 SSH 连接状态 + 流量监控显示。
    func updateConnectionStatus(_ summary: (type: String, host: String?, status: String),
                                trafficRate: UInt64, totalBytes: UInt64) {
        statusBar.updateConnectionStatus(summary, trafficRate: trafficRate, totalBytes: totalBytes)
    }

    func refreshLocalization() {
        statusBar.refreshLocalization()
        connectionStatus.refreshLocalization()
    }
}
