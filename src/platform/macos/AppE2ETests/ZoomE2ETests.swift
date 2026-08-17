import AppKit
import XCTest
@testable import MuxtermAppLib

/// 先前 macOS bug：Alt/Cmd+Enter 后 tmux 已 zoom，GUI 仍显示多 pane。
final class ZoomE2ETests: XCTestCase {
    func testAltEnterCollapsesLayoutToSingleLeaf() throws {
        let painted = PaintedWorkspace(label: "zoom")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3), "zoom 前应有 3 leaf")
        let token = painted.tab1Tokens[0]
        XCTAssertTrue(app.waitTerminalContains(token))

        app.testTogglePaneFullscreen()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(30)
                return Tmux.out(
                    socket: painted.socket,
                    args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                ) == "1"
            },
            "tmux window_zoomed_flag 应为 1"
        )
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testLayoutLeafIDs().count == 1
            },
            "GUI 必须变成单 leaf（不能只 zoom tmux）。leaves=\(app.testLayoutLeafIDs())"
        )
        let leaf = try XCTUnwrap(app.testLayoutLeafIDs().first)
        let size = app.testPaneAllocation(leaf)
        XCTAssertGreaterThanOrEqual(size.width, AppE2E.minPanePx)
        XCTAssertGreaterThanOrEqual(size.height, AppE2E.minPanePx)
        XCTAssertTrue(
            app.testPaneTerminalText(leaf).contains(token)
                || app.testAllVisibleTerminalText().contains(token),
            "zoom 后最后一帧必须还在"
        )

        app.testTogglePaneFullscreen()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.testLayoutLeafIDs().count == 3
            },
            "再按一次应恢复 3 leaf。leaves=\(app.testLayoutLeafIDs())"
        )
    }
}
