import AppKit
import MuxtermChrome

/// 根内容视图：扁平 TabBar + 主导终端区 + 一行 StatusBar（无嵌套卡片）。
final class ContentView: NSView {
    let tabBar = TabBarView()
    let paneLayout: PaneLayoutView
    /// muxterm 自己的状态摘要行（连接状态 / tabs / panes / 输出片段）。
    let connectionStatus = ConnectionStatusView()
    /// muxterm status bar（连接 tmux 时按 tmux 的 status 配置渲染，替换 tab 栏）。
    let statusBar = StatusBarView()

    private var tabTopConstraints: [NSLayoutConstraint] = []
    private var tabBottomConstraints: [NSLayoutConstraint] = []
    private var statusTopConstraints: [NSLayoutConstraint] = []
    private var statusBottomConstraints: [NSLayoutConstraint] = []
    private var position: TabBarPosition = .top
    private var usingStatusBar = false

    init(terminalManager: TerminalManager) {
        self.paneLayout = PaneLayoutView(terminalManager: terminalManager)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor

        tabBar.translatesAutoresizingMaskIntoConstraints = false
        paneLayout.translatesAutoresizingMaskIntoConstraints = false
        connectionStatus.translatesAutoresizingMaskIntoConstraints = false
        statusBar.translatesAutoresizingMaskIntoConstraints = false

        addSubview(paneLayout)
        addSubview(tabBar)
        addSubview(connectionStatus)
        addSubview(statusBar)

        // 顶部布局：tab | pane | connectionStatus
        tabTopConstraints = [
            tabBar.topAnchor.constraint(equalTo: topAnchor),
            tabBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            tabBar.trailingAnchor.constraint(equalTo: trailingAnchor),

            paneLayout.topAnchor.constraint(equalTo: tabBar.bottomAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: connectionStatus.topAnchor),

            connectionStatus.leadingAnchor.constraint(equalTo: leadingAnchor),
            connectionStatus.trailingAnchor.constraint(equalTo: trailingAnchor),
            connectionStatus.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        // 底部布局：pane | tab | connectionStatus（tab 紧贴状态行上方）
        tabBottomConstraints = [
            paneLayout.topAnchor.constraint(equalTo: topAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: tabBar.topAnchor),

            tabBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            tabBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            tabBar.bottomAnchor.constraint(equalTo: connectionStatus.topAnchor),

            connectionStatus.leadingAnchor.constraint(equalTo: leadingAnchor),
            connectionStatus.trailingAnchor.constraint(equalTo: trailingAnchor),
            connectionStatus.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        // status bar 布局：top = status | pane；bottom = pane | status
        statusTopConstraints = [
            statusBar.topAnchor.constraint(equalTo: topAnchor),
            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),

            paneLayout.topAnchor.constraint(equalTo: statusBar.bottomAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]
        statusBottomConstraints = [
            paneLayout.topAnchor.constraint(equalTo: topAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: statusBar.topAnchor),

            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        applyTabBarPosition(TabBarPosition.current)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func applyTabBarPosition(_ position: TabBarPosition) {
        self.position = position
        guard !usingStatusBar else { return }
        NSLayoutConstraint.deactivate(tabTopConstraints)
        NSLayoutConstraint.deactivate(tabBottomConstraints)
        switch position {
        case .top:
            NSLayoutConstraint.activate(tabTopConstraints)
            tabBar.setEdgeLineAtBottom(true)
        case .bottom:
            NSLayoutConstraint.activate(tabBottomConstraints)
            tabBar.setEdgeLineAtBottom(false)
        }
        needsLayout = true
    }

    /// 应用 status bar；nil / 未启用时回退到默认 tab 栏。
    func applyStatusBar(_ snapshot: StatusBarSnapshot?) {
        let enabled = snapshot?.enabled == true
        usingStatusBar = enabled
        statusBar.isHidden = !enabled
        tabBar.isHidden = enabled
        connectionStatus.isHidden = enabled
        NSLayoutConstraint.deactivate(statusTopConstraints)
        NSLayoutConstraint.deactivate(statusBottomConstraints)
        guard let snapshot, enabled else {
            applyTabBarPosition(position)
            return
        }
        statusBar.apply(snapshot: snapshot)
        if snapshot.position == "top" {
            NSLayoutConstraint.activate(statusTopConstraints)
        } else {
            NSLayoutConstraint.activate(statusBottomConstraints)
        }
        needsLayout = true
    }

    func refreshLocalization() {
        tabBar.refreshLocalization()
        connectionStatus.refreshLocalization()
    }
}
