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

    func testStatusBarHasLeftCenterRightAndChromeButtons() {
        let bar = StatusBarView(frame: .zero)
        window.contentView = bar
        window.setFrame(NSRect(x: 0, y: 0, width: 960, height: 80), display: true)
        window.orderFront(nil)
        AppE2E.pump(40)

        XCTAssertEqual(bar.accessibilityIdentifier(), "muxterm.statusBar")
        XCTAssertNotNil(find(bar, "muxterm.statusDot"), "状态点按钮应存在")
        XCTAssertNotNil(find(bar, "muxterm.statusAttention"), "通知位应存在")
        XCTAssertNotNil(find(bar, "muxterm.newTabButton"), "新建 tab 按钮应存在")

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
        XCTAssertEqual(bar.testTabTitle(18), "1  code")
        XCTAssertEqual(bar.testTabTitle(21), "2  other")
        XCTAssertFalse(bar.testTabTitle(18).contains("#["), "GUI tab 不得渲染 tmux 格式串")
        XCTAssertFalse(bar.testTabTitle(21).contains("#["), "GUI tab 不得渲染 tmux 格式串")
        XCTAssertNotNil(find(bar, "muxterm.tab.18"))
        XCTAssertNotNil(find(bar, "muxterm.tab.21"))
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
        bar.applyTmuxSnapshot(snap, enabled: true)
        AppE2E.pump(40)
        bar.testClickTab(21)
        AppE2E.pump(40)
        XCTAssertEqual(switched, [21], "回调应收到 21 而不是 1")
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
        XCTAssertEqual(sampler.sample(totalBytes: 4096, now: 14), 0, "warm workspace 切换后重建基线")
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
