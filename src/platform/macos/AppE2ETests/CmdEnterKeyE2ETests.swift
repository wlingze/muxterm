import AppKit
import XCTest
@testable import MuxtermAppLib

/// 用户路径：Cmd-Enter 切换当前 pane 全屏。不要只测 KeyChord 表，要走 handleKey。
final class CmdEnterKeyE2ETests: XCTestCase {
    func testCmdEnterKeyEventZoomsTmuxAndGuiLeaf() throws {
        let painted = PaintedWorkspace(label: "cmd-enter")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3), "zoom 前应有 3 leaf")

        app.window?.makeKeyAndOrderFront(nil)
        AppE2E.pump(40)
        let event = try XCTUnwrap(app.testMakeCmdEnterEvent(), "必须能构造 Cmd-Enter")
        XCTAssertTrue(app.testDispatchKeyEvent(event), "handleKey 必须消费 Cmd-Enter")

        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(30)
                return Tmux.out(
                    socket: painted.socket,
                    args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                ) == "1"
            },
            "Cmd-Enter 后 tmux window_zoomed_flag 应为 1"
        )
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testLayoutLeafIDs().count == 1
            },
            "Cmd-Enter 后 GUI 必须单 leaf。leaves=\(app.testLayoutLeafIDs())"
        )
    }
}
