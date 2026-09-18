import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 很多 tab 时 status-right 与右侧 chrome 必须仍可见；tmux 标题保持自然宽度，
/// 超出中间 viewport 的部分只被裁剪，不能挤动两侧控制区。
final class StatusBarOverflowE2ETests: XCTestCase {
    func testManyTabsDoNotCrushStatusRight() throws {
        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "tab-ovf")
        for i in 0..<12 {
            Tmux.ok(socket: fx.socket, args: [
                "new-window", "-t", fx.session, "-n", "long-window-name-\(i)", "/bin/cat",
            ])
        }
        Tmux.ok(socket: fx.socket, args: ["set-option", "-g", "status-left", "LEFT"])
        Tmux.ok(socket: fx.socket, args: [
            "set-option", "-g", "status-right", "RIGHT_MARKER_12345678",
        ])
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        app.window?.setFrame(NSRect(x: 40, y: 40, width: 720, height: 600), display: true)
        XCTAssertTrue(app.waitReady())
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(40)
                app.content.statusBar.layoutSubtreeIfNeeded()
                return app.content.statusBar.testRightText().contains("RIGHT_MARKER")
                    || app.testStatusRightWidth() > 0
            },
            "必须刷到 tmux status-right。right=\(app.content.statusBar.testRightText())"
        )
        app.content.statusBar.layoutSubtreeIfNeeded()
        app.content.layoutSubtreeIfNeeded()

        let right = app.testStatusRightWidth()
        XCTAssertGreaterThanOrEqual(
            right,
            StatusBarTabOverflow.statusRightMinWidth,
            "status-right 宽度必须 ≥ \(StatusBarTabOverflow.statusRightMinWidth)，不能被 tab 挤没。right=\(right)"
        )
        let chromeMin = app.testChromeMinX()
        XCTAssertGreaterThan(
            chromeMin,
            0,
            "状态点/铃铛/+ 必须仍在 bar 内。chromeMinX=\(chromeMin)"
        )
        let widths = app.testTabButtonWidths()
        XCTAssertFalse(widths.isEmpty, "必须画出 tab 按钮")
        XCTAssertTrue(widths.allSatisfy { $0 > 0 }, "tmux title 必须产生自然宽度。widths=\(widths)")
        let rightView = app.testView(identifier: "muxterm.statusRight")
        XCTAssertNotNil(rightView, "muxterm.statusRight 必须存在")
        XCTAssertFalse(rightView?.isHidden ?? true, "status-right 不得隐藏")
    }
}
