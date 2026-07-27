import AppKit

/// 根内容视图：扁平 TabBar + 主导终端区 + 一行 StatusBar（无嵌套卡片）。
final class ContentView: NSView {
    let tabBar = TabBarView()
    let paneLayout: PaneLayoutView
    let statusBar = StatusBarView()

    private var tabTopConstraints: [NSLayoutConstraint] = []
    private var tabBottomConstraints: [NSLayoutConstraint] = []
    private var position: TabBarPosition = .top

    init(terminalManager: TerminalManager) {
        self.paneLayout = PaneLayoutView(terminalManager: terminalManager)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor

        tabBar.translatesAutoresizingMaskIntoConstraints = false
        paneLayout.translatesAutoresizingMaskIntoConstraints = false
        statusBar.translatesAutoresizingMaskIntoConstraints = false

        addSubview(paneLayout)
        addSubview(tabBar)
        addSubview(statusBar)

        // 顶部布局：tab | pane | status
        tabTopConstraints = [
            tabBar.topAnchor.constraint(equalTo: topAnchor),
            tabBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            tabBar.trailingAnchor.constraint(equalTo: trailingAnchor),

            paneLayout.topAnchor.constraint(equalTo: tabBar.bottomAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: statusBar.topAnchor),

            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        // 底部布局：pane | tab | status（tab 紧贴状态栏上方）
        tabBottomConstraints = [
            paneLayout.topAnchor.constraint(equalTo: topAnchor),
            paneLayout.leadingAnchor.constraint(equalTo: leadingAnchor),
            paneLayout.trailingAnchor.constraint(equalTo: trailingAnchor),
            paneLayout.bottomAnchor.constraint(equalTo: tabBar.topAnchor),

            tabBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            tabBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            tabBar.bottomAnchor.constraint(equalTo: statusBar.topAnchor),

            statusBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]

        applyTabBarPosition(TabBarPosition.current)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func applyTabBarPosition(_ position: TabBarPosition) {
        self.position = position
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
}
