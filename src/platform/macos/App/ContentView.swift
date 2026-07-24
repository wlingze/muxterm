import AppKit

/// 根内容视图：上 TabBar、中 PaneLayout、下 StatusBar。
final class ContentView: NSView {
    let tabBar = TabBarView()
    let paneLayout: PaneLayoutView
    let statusBar = StatusBarView()

    init(terminalManager: TerminalManager) {
        self.paneLayout = PaneLayoutView(terminalManager: terminalManager)
        super.init(frame: .zero)
        wantsLayer = true

        tabBar.translatesAutoresizingMaskIntoConstraints = false
        paneLayout.translatesAutoresizingMaskIntoConstraints = false
        statusBar.translatesAutoresizingMaskIntoConstraints = false

        addSubview(tabBar)
        addSubview(paneLayout)
        addSubview(statusBar)

        NSLayoutConstraint.activate([
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
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}
