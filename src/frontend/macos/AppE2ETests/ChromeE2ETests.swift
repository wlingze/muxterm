import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 对标 `linux_chrome_e2e`：一条 status bar、GUI tab、点状态点出 popover。
final class ChromeE2ETests: XCTestCase {
    private var window: NSWindow!

    override func setUp() {
        super.setUp()
        AppE2E.ensureApp()
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 80),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
    }

    override func tearDown() {
        // 用 orderOut 而不是 close：XCTest memory checker 在 SwiftPM 测试进程
        // 里 dealloc 已 close 的 NSWindow 会过度 release 崩溃。
        window.orderOut(nil)
        window = nil
        super.tearDown()
    }

    func testMainWindowStartsWideAndRemainsHorizontallyResizable() throws {
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(
            bridge: bridge,
            debug: false,
            quickConnectStore: QuickConnectStore()
        )
        defer { app.testShutdown() }
        guard let mainWindow = app.window else {
            return XCTFail("主窗口必须存在")
        }
        app.showWindow(nil)
        XCTAssertTrue(app.waitReady())
        AppE2E.pump(200)

        let rootView = try XCTUnwrap(mainWindow.contentView)
        XCTAssertEqual(
            app.content.frame.width, rootView.bounds.width, accuracy: 1,
            "首次显示时内容必须铺满窗口，不能依赖手动 resize"
        )
        XCTAssertEqual(
            app.content.frame.height, rootView.bounds.height, accuracy: 1,
            "首次显示时终端区域必须获得完整高度"
        )
        XCTAssertEqual(app.content.statusBar.frame.width, app.content.bounds.width, accuracy: 1)
        XCTAssertEqual(
            app.content.paneLayout.frame.height,
            app.content.bounds.height - app.content.statusBar.frame.height,
            accuracy: 1
        )
        XCTAssertGreaterThanOrEqual(
            mainWindow.frame.width,
            900,
            "初始 960pt 窗口不能被内容约束压到最小宽度"
        )
        for width in [720.0, 1180.0, 840.0] {
            let frame = AppE2E.fixedWindowFrame(
                width: width,
                height: mainWindow.frame.height
            )
            mainWindow.setFrame(frame, display: true)
            XCTAssertEqual(
                mainWindow.frame.width,
                width,
                accuracy: 1,
                "NSWindow 必须先接受用户请求的宽度"
            )
            AppE2E.pump(40)
            XCTAssertEqual(
                mainWindow.frame.width,
                width,
                accuracy: 1,
                "状态栏和 split view 不能锁死用户设置的窗口宽度"
            )
            XCTAssertEqual(
                app.content.frame.width,
                mainWindow.contentLayoutRect.width,
                accuracy: 1,
                "终端内容区必须跟随窗口宽度"
            )
            XCTAssertEqual(
                app.content.statusBar.testTabStackFrame().width,
                app.content.statusBar.testTabViewportFrame().width,
                accuracy: 1,
                "等宽 Tab 必须继续铺满可用中间区"
            )
        }

        app.content.statusBar.applyTmuxSnapshot(
            Self.snapshot(left: "LEFT", right: "RIGHT", windows: []),
            enabled: true
        )
        let tmuxFrame = AppE2E.fixedWindowFrame(width: 900, height: mainWindow.frame.height)
        mainWindow.setFrame(tmuxFrame, display: true)
        AppE2E.pump(40)
        XCTAssertEqual(
            mainWindow.frame.width,
            tmuxFrame.width,
            accuracy: 1,
            "tmux 状态栏也不能覆盖用户调整的窗口宽度"
        )
    }

    func testStatusBarHasLeftCenterRightAndChromeButtons() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.setFrame(NSRect(x: 0, y: 0, width: 960, height: 80), display: true)
        window.orderFront(nil)
        AppE2E.pump(40)

        XCTAssertEqual(bar.accessibilityIdentifier(), "muxterm.statusBar")
        XCTAssertNotNil(find(bar, "muxterm.statusDot"), "状态点按钮应存在")
        XCTAssertNotNil(find(bar, "muxterm.statusWorkspaces"), "Workspace 快速入口应存在")
        XCTAssertNotNil(find(bar, "muxterm.statusAttention"), "通知位应存在")
        XCTAssertNotNil(find(bar, "muxterm.newTabButton"), "新建 tab 按钮应存在")

        // Tab 顺序/标题来自 Core snapshot；tmux status fixture 只覆盖
        // left/right 与样式，不再作为第二份窗口列表。
        bar.updateTabs([
            Tab(id: 18, name: "code", isActive: true),
            Tab(id: 21, name: "other", isActive: false),
        ])
        bar.applyTmuxSnapshot(Self.snapshot(
            left: "L",
            right: "R",
            windows: [
                Self.wnd(18, index: 1, name: "code", current: true, text: " 1#[fg=colour237]:#[fg=colour250]code "),
                Self.wnd(21, index: 2, name: "other", current: false, text: " 2#[fg=colour237]:#[fg=colour250]other "),
            ]
        ), enabled: true)
        AppE2E.pump(40)

        XCTAssertTrue(bar.testLeftText().contains("L"), "left 应含 L: \(bar.testLeftText())")
        XCTAssertTrue(bar.testRightText().contains("R"), "right 应含 R: \(bar.testRightText())")
        XCTAssertEqual(
            bar.testTabTitle(18).trimmingCharacters(in: .whitespaces),
            "1:code"
        )
        XCTAssertEqual(
            bar.testTabTitle(21).trimmingCharacters(in: .whitespaces),
            "2:other"
        )
        XCTAssertFalse(bar.testTabTitle(18).contains("#["), "GUI tab 不得渲染 tmux 格式串")
        XCTAssertFalse(bar.testTabTitle(21).contains("#["), "GUI tab 不得渲染 tmux 格式串")
        XCTAssertNotNil(find(bar, "muxterm.tab.18"))
        XCTAssertNotNil(find(bar, "muxterm.tab.21"))
    }

    func testWorkspaceButtonShowsAggregatePresentationAndInvokesPanel() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.setContentSize(NSSize(width: 900, height: 24))
        window.orderFront(nil)
        var clicks = 0
        bar.onWorkspaceClick = { clicks += 1 }

        bar.setWorkspacePresentation(.shells)
        XCTAssertEqual(bar.testWorkspaceTitle(), "S")
        bar.updateTabs([Tab(id: 7, name: "shell", isActive: true)])
        XCTAssertEqual(bar.testTabAggregateAppearance(7), "shells")
        bar.setWorkspacePresentation(.agents)
        XCTAssertEqual(bar.testWorkspaceTitle(), "A")
        XCTAssertEqual(bar.testTabAggregateAppearance(7), "agents")
        bar.setWorkspacePresentation(.workspace)
        XCTAssertNil(bar.testTabAggregateAppearance(7))
        bar.testClickWorkspace()
        XCTAssertEqual(clicks, 1)
    }

    func testNotifyButtonInvokesAttentionCallbackWhenNPositive() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)
        var clicks = 0
        bar.onAttentionClick = { clicks += 1 }
        bar.setAttention(StatusBarAttention(count: 2))
        AppE2E.pump(40)
        try? writeSnapshot(bar, name: "status-bar-attention")
        XCTAssertEqual(bar.testAttentionSymbolName(), "bell.fill")
        XCTAssertTrue(
            bar.testAttentionCountLabel().contains("2"),
            "n=2 时按钮文本应含 2: \(bar.testAttentionCountLabel())"
        )
        bar.testClickAttention()
        AppE2E.pump(40)
        XCTAssertEqual(clicks, 1, "点击应触发回调一次")

        bar.setAttention(StatusBarAttention(count: 0))
        XCTAssertEqual(bar.testAttentionSymbolName(), "bell")
    }

    func testAttentionBellUsesSidebarActivityState() {
        let bar = StatusBarView(frame: .zero)
        bar.setAttention(StatusBarAttention(indicators: [.working, .blocked]))
        XCTAssertEqual(bar.testAttentionIndicator(), .blocked)
        bar.setAttention(StatusBarAttention(indicators: [.working]))
        XCTAssertEqual(bar.testAttentionIndicator(), .working)
    }

    func testAttentionBellCountMatchesDisplayedColor() {
        let bar = StatusBarView(frame: .zero)
        bar.setAttention(StatusBarAttention(indicators: [
            .done, .done, .blocked, .blocked, .blocked, .working,
        ]))
        XCTAssertEqual(bar.testAttentionIndicator(), .done)
        XCTAssertTrue(
            bar.testAttentionCountLabel().contains("2"),
            "完成色铃铛应显示完成数 2，而不是总数 6: \(bar.testAttentionCountLabel())"
        )

        bar.setAttention(StatusBarAttention(indicators: [
            .blocked, .blocked, .blocked, .working,
        ]))
        XCTAssertEqual(bar.testAttentionIndicator(), .blocked)
        XCTAssertTrue(
            bar.testAttentionCountLabel().contains("3"),
            "阻塞色铃铛应显示阻塞数 3，而不是总数 4: \(bar.testAttentionCountLabel())"
        )
    }

    func testStatusButtonsReceivePhysicalHitTesting() throws {
        let bar = StatusBarView(frame: NSRect(x: 0, y: 0, width: 900, height: 28))
        window.contentView = bar
        window.orderFront(nil)
        bar.layoutSubtreeIfNeeded()
        for identifier in ["muxterm.statusDot", "muxterm.statusAttention"] {
            let button = try XCTUnwrap(bar.subviews.first { $0.accessibilityIdentifier() == identifier })
            let point = bar.convert(NSPoint(x: button.bounds.midX, y: button.bounds.midY), from: button)
            XCTAssertTrue(bar.hitTest(bar.convert(point, to: bar.superview)) === button,
                          "真正点击右侧按钮必须命中按钮，不能落到状态栏拖窗")
        }
    }

    func testWorkingSpinnerSurvivesTabTitleAndSelectionUpdates() {
        let bar = StatusBarView(frame: NSRect(x: 0, y: 0, width: 900, height: 28))
        window.contentView = bar
        window.orderFront(nil)
        bar.updateTabs([Tab(id: 1, name: "codex", isActive: true)])
        bar.setTabActivities([1: .working])
        bar.layoutSubtreeIfNeeded()
        let identity = bar.testTabActivityIdentity(1)
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        for i in 0..<5 {
            bar.updateTabs([Tab(id: 1, name: "codex \(i)", isActive: i % 2 == 0),
                            Tab(id: 2, name: "shell", isActive: i % 2 != 0)])
            bar.layoutSubtreeIfNeeded()
            XCTAssertEqual(bar.testTabActivityIdentity(1), identity,
                           "状态栏刷新不能销毁转圈视图并把动画重置到起点")
            XCTAssertTrue(
                bar.testTabActivityAnimating(1) || reduceMotion,
                "working tab 必须保持转圈；Reduce Motion 下允许静态指示"
            )
        }
    }

    func testTabsShowSharedActivityStateAndWorkingSpinner() {
        let bar = StatusBarView(frame: NSRect(x: 0, y: 0, width: 600, height: 24))
        window.contentView = bar
        window.orderFront(nil)
        bar.updateTabs([
            Tab(id: 1, name: "build", isActive: true),
            Tab(id: 2, name: "review", isActive: false),
        ])

        bar.setTabActivities([1: .working, 2: .done])
        bar.layoutSubtreeIfNeeded()
        AppE2E.pump(20)

        XCTAssertEqual(bar.testTabActivity(1), .working)
        XCTAssertTrue(
            bar.testTabActivityAnimating(1)
                || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
            "working tab 必须转圈；Reduce Motion 下允许静态指示"
        )
        XCTAssertEqual(bar.testTabActivity(2), .done)
        XCTAssertFalse(bar.testTabActivityAnimating(2))

        bar.setTabActivities([2: .blocked])
        XCTAssertNil(bar.testTabActivity(1))
        XCTAssertEqual(bar.testTabActivity(2), .blocked)
    }

    func testClickStatusTabInvokesSwitchWithWindowId() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)
        var switched: [UInt32] = []
        bar.onSelectWindow = { switched.append($0) }
        let snap = Self.snapshot(
            left: "L",
            right: "R",
            windows: [
                Self.wnd(18, index: 1, name: "code", current: true, text: "1:code"),
                Self.wnd(21, index: 2, name: "other", current: false, text: "2:other"),
            ]
        )
        bar.updateTabs([
            Tab(id: 18, name: "code", isActive: true),
            Tab(id: 21, name: "other", isActive: false),
        ])
        bar.applyTmuxSnapshot(snap, enabled: true)
        AppE2E.pump(40)
        bar.testClickTab(21)
        AppE2E.pump(40)
        XCTAssertEqual(switched, [21], "回调应收到 21 而不是 1")
    }

    func testTabStyleSwitchesBetweenEqualWidthAndCompactWithVisibleCloseButtons() {
        let bar = StatusBarView(frame: NSRect(x: 0, y: 0, width: 900, height: 24))
        let host = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 24))
        bar.translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(bar)
        NSLayoutConstraint.activate([
            bar.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            bar.topAnchor.constraint(equalTo: host.topAnchor),
            bar.bottomAnchor.constraint(equalTo: host.bottomAnchor),
        ])
        let tabs = [
            Tab(id: 1, name: "one", isActive: true),
            Tab(id: 2, name: "two", isActive: false),
            Tab(id: 3, name: "three", isActive: false),
        ]
        var closed: [UInt32] = []
        bar.onCloseTab = { closed.append($0) }
        bar.tabBarStyle = .equalWidth
        bar.updateTabs(tabs)
        host.updateConstraintsForSubtreeIfNeeded()
        host.layoutSubtreeIfNeeded()

        let equalWidths = bar.testTabButtonWidths()
        XCTAssertEqual(equalWidths.count, 3)
        XCTAssertLessThanOrEqual(
            (equalWidths.max() ?? 0) - (equalWidths.min() ?? 0),
            1,
            "等宽布局允许 AppKit 在像素对齐时产生 1pt 取整差异"
        )
        XCTAssertEqual(bar.testVisibleTabCloseIDs(), [1, 2, 3])
        bar.testClickTabClose(2)
        XCTAssertEqual(closed, [2], "关闭按钮必须直接关闭对应 Tab，不触发行选择")

        bar.tabBarStyle = .compact
        host.updateConstraintsForSubtreeIfNeeded()
        host.layoutSubtreeIfNeeded()
        let naturalWidths = bar.testTabButtonWidths()
        XCTAssertGreaterThan(naturalWidths[2], naturalWidths[0], "标题更长的 Tab 应自然更宽")
        XCTAssertTrue(naturalWidths.allSatisfy { $0 < StatusBarTabOverflow.fixedTabWidth })
    }

    func testTmuxStatusDoesNotOverrideCoreTabOrder() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)

        // Core runtime 的顺序故意与 status 的 window_index 顺序相反；
        // 渲染仍必须跟随 Core，而不是重新按 status 窗口列表排序。
        bar.updateTabs([
            Tab(id: 21, name: "core-first", isActive: true),
            Tab(id: 18, name: "core-second", isActive: false),
        ])
        bar.applyTmuxSnapshot(Self.snapshot(
            left: "",
            right: "",
            windows: [
                Self.wnd(18, index: 1, name: "tmux-one", current: false, text: "1:tmux-one"),
                Self.wnd(21, index: 7, name: "tmux-seven", current: true, text: "7:tmux-seven"),
            ]
        ), enabled: true)
        AppE2E.pump(40)

        XCTAssertEqual(bar.testTabIDs(), [21, 18], "顺序仍必须跟随 Core")
        XCTAssertEqual(bar.testTabTitle(21), "7:tmux-seven")
        XCTAssertEqual(bar.testTabTitle(18), "1:tmux-one")
        let widths = bar.testTabButtonWidths()
        XCTAssertGreaterThan(widths[0], widths[1], "tmux 标题多大，Tab 就应按标题自然宽度显示")
    }

    func testStatusDotClickOpensPopoverWithSshSummary() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.setFrame(NSRect(x: 0, y: 0, width: 960, height: 80), display: true)
        window.orderFront(nil)
        bar.updateConnectionStatus(
            (type: "ssh", host: "127.0.0.1", status: "connected"),
            trafficRate: 1536,
            totalBytes: 1536,
            upRate: 56,
            upBytes: 56
        )
        AppE2E.pump(40)
        let size = bar.testStatusDotSize()
        XCTAssertEqual(size.width, 18, "状态点热区宽必须是 18")
        XCTAssertEqual(size.height, 18, "状态点热区高必须是 18")

        bar.testClickStatusDot()
        AppE2E.pump(40)
        try? writeSnapshot(bar.testPopoverContentView(), name: "status-popover-ssh")
        XCTAssertTrue(bar.testPopoverVisible(), "点状态点后 popover 应可见")
        let text = bar.testPopoverText()
        XCTAssertEqual(bar.testStatusSymbolName(), "network")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.transport"), "SSH")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.host"), "127.0.0.1")
        XCTAssertEqual(
            bar.testPopoverValue("muxterm.statusPopover.state"),
            MuxtermI18n.shared.tr(.statusConnected)
        )
        XCTAssertFalse(text.contains("1536B/s") || text.contains("1234B/s"), "禁止把累计字节标成 B/s: \(text)")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.receiveRate"), "1.5 KB/s")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.received"), "1.5 KB")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.sendRate"), "56 B/s")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.sent"), "56 B")
    }

    func testSshPopoverDoesNotInventUnavailableUploadMetrics() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)
        bar.updateConnectionStatus(
            (type: "ssh", host: "build-host", status: "connected"),
            trafficRate: 1_048_576,
            totalBytes: 1_099_511_627_776
        )
        bar.testClickStatusDot()
        AppE2E.pump(40)

        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.receiveRate"), "1.0 MB/s")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.received"), "1.0 TB")
        XCTAssertNil(bar.testPopoverValue("muxterm.statusPopover.sendRate"))
        XCTAssertNil(bar.testPopoverValue("muxterm.statusPopover.sent"))
    }

    func testConnectionPopoverUpdatesAndRefreshesWithoutReopening() throws {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)
        var refreshes = 0
        bar.onConnectionRefresh = { refreshes += 1 }
        bar.updateConnectionStatus((type: "ssh", host: "ryzen", status: "connected"),
                                   trafficRate: 1024, totalBytes: 2048)
        bar.testClickStatusDot()
        AppE2E.pump(30)
        XCTAssertEqual(refreshes, 1)
        bar.updateConnectionStatus((type: "ssh", host: "ryzen", status: "connected"),
                                   trafficRate: 3072, totalBytes: 4096)
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.receiveRate"), "3.0 KB/s")
        XCTAssertEqual(bar.testPopoverValue("muxterm.statusPopover.received"), "4.0 KB")
        func findRefresh(_ view: NSView) -> NSButton? {
            if view.accessibilityIdentifier() == "muxterm.statusPopover.refresh" { return view as? NSButton }
            return view.subviews.lazy.compactMap(findRefresh).first
        }
        let content = try XCTUnwrap(bar.testPopoverContentView())
        try XCTUnwrap(findRefresh(content)).performClick(nil)
        XCTAssertEqual(refreshes, 2)
        XCTAssertTrue(bar.testPopoverVisible())
    }

    func testConnectionErrorUsesNativeErrorIconAndDetailRow() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.orderFront(nil)
        bar.updateConnectionStatus(
            (type: "ssh", host: "offline-host", status: "disconnected"),
            trafficRate: 0,
            totalBytes: 0
        )
        bar.showError("connection refused")
        bar.testClickStatusDot()
        AppE2E.pump(40)

        XCTAssertEqual(bar.testStatusSymbolName(), "exclamationmark.circle.fill")
        XCTAssertEqual(
            bar.testPopoverValue("muxterm.statusPopover.error"),
            "connection refused"
        )
    }

    func testHumanReadableTrafficFormatterCoversLargeSessions() {
        XCTAssertEqual(StatusTrafficFormatter.bytes(0), "0 B")
        XCTAssertEqual(StatusTrafficFormatter.rate(1536), "1.5 KB/s")
        XCTAssertEqual(StatusTrafficFormatter.bytes(1_073_741_824), "1.0 GB")
        XCTAssertEqual(StatusTrafficFormatter.bytes(1_099_511_627_776), "1.0 TB")
    }

    func testTrafficSamplerReportsCurrentIntervalAndReturnsToZero() {
        var sampler = TrafficRateSampler()
        XCTAssertEqual(sampler.sample(totalBytes: 100, now: 10), 0)
        XCTAssertEqual(sampler.sample(totalBytes: 1636, now: 11), 1536)
        XCTAssertEqual(sampler.sample(totalBytes: 1636, now: 12), 0)
        XCTAssertEqual(sampler.sample(totalBytes: 8, now: 13), 0, "连接切换后累计值归零")
        sampler.reset()
        XCTAssertEqual(sampler.sample(totalBytes: 4096, now: 14), 0, "切换 Workspace 后重建基线")
    }

    private func find(_ root: NSView, _ id: String) -> NSView? {
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let found = find(child, id) { return found }
        }
        return nil
    }

    /// 与面板快照相同：仅设置环境变量时落盘，常规 E2E 不产生文件。
    private func writeSnapshot(_ view: NSView?, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["MUXTERM_UI_SNAPSHOT_DIR"],
              let view,
              !view.bounds.isEmpty,
              let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else {
            return
        }
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { return }
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try png.write(to: root.appendingPathComponent("\(name).png"), options: .atomic)
    }

    private static func snapshot(left: String, right: String, windows: [StatusBarWindow]) -> StatusBarSnapshot {
        StatusBarSnapshot(
            enabled: true,
            position: "bottom",
            justify: "left",
            interval: 1,
            left: left,
            right: right,
            leftLength: 40,
            rightLength: 40,
            statusStyle: "",
            leftStyle: "",
            rightStyle: "",
            separator: " ",
            windowFormat: "",
            windowCurrentFormat: "",
            windowStyle: "",
            windowCurrentStyle: "",
            windows: windows,
            error: nil
        )
    }

    private static func wnd(
        _ id: UInt32,
        index: UInt32,
        name: String,
        current: Bool,
        text: String
    ) -> StatusBarWindow {
        StatusBarWindow(
            windowId: id,
            index: index,
            name: name,
            flags: current ? "*" : "",
            current: current,
            text: text
        )
    }
}
