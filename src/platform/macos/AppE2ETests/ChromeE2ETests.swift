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
                Self.wnd(18, name: "code", current: true, text: " 1#[fg=colour237]:#[fg=colour250]code "),
                Self.wnd(21, name: "other", current: false, text: " 2#[fg=colour237]:#[fg=colour250]other "),
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
        XCTAssertTrue(
            bar.testAttentionCountLabel().contains("2"),
            "n=2 时按钮文本应含 2: \(bar.testAttentionCountLabel())"
        )
        find(bar, "muxterm.statusAttention")?.gestureRecognizers.forEach { rec in
            _ = rec.target?.perform(rec.action, with: rec)
        }
        AppE2E.pump(40)
        XCTAssertEqual(clicks, 1, "点击应触发回调一次")
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
                Self.wnd(18, name: "code", current: true, text: "1:code"),
                Self.wnd(21, name: "other", current: false, text: "2:other"),
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
        XCTAssertTrue(bar.testPopoverVisible(), "点状态点后 popover 应可见")
        let text = bar.testPopoverText()
        XCTAssertTrue(text.contains("type=ssh"), "应含 type=ssh: \(text)")
        XCTAssertTrue(text.contains("host=127.0.0.1"), "应含 host: \(text)")
        XCTAssertTrue(text.contains("status=connected"), "应含 status: \(text)")
        XCTAssertFalse(text.contains("1536B/s") || text.contains("1234B/s"), "禁止把累计字节标成 B/s: \(text)")
        XCTAssertTrue(text.contains("1.5 KB/s"), "必须有人类可读速率 1.5 KB/s: \(text)")
        XCTAssertTrue(text.contains("1.5 KB") && text.contains("56 B"), "必须有人类可读累计（1.5 KB 和 56 B）: \(text)")
    }

    private func find(_ root: NSView, _ id: String) -> NSView? {
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let found = find(child, id) { return found }
        }
        return nil
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

    private static func wnd(_ id: UInt32, name: String, current: Bool, text: String) -> StatusBarWindow {
        StatusBarWindow(
            windowId: id,
            index: id,
            name: name,
            flags: current ? "*" : "",
            current: current,
            text: text
        )
    }
}
