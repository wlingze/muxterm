import AppKit
import MuxtermChrome

/// 根内容视图：终端区 + 底部统一 StatusBar（tab + 状态/通知/新建，一个 bar）。
///
/// 渲染纪律（文档 §2.15.2 追加 B）：输出直接渲染到末尾，不逐帧滚动刷新。
final class ContentView: NSView {
    let paneLayout: PaneLayoutView
    let statusBar = StatusBarView()

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

        addSubview(paneLayout)
        addSubview(statusBar)

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
}
