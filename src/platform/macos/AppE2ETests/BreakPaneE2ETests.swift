import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 把 pane 拖成新 tab = tmux `break-pane`（iTerm2 breakOutWindowPane / MoveSessionToNewTab）。
final class BreakPaneE2ETests: XCTestCase {
    func testBreakPaneCreatesNewTabWithoutExtraHierarchy() throws {
        let fx = TwoPaneCat(label: "break-p")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let tabsBefore = app.testTabAndPaneCounts().tabs
        XCTAssertEqual(tabsBefore, 1, "夹具开始应是单 tab 两 pane")
        app.testBreakActivePaneToNewTab()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(40)
                return app.testTabAndPaneCounts().tabs == 2
            },
            "break-pane 后必须变成 2 个 tab。got=\(app.testTabAndPaneCounts())"
        )
        XCTAssertEqual(
            app.testLayoutLeafIDs().count,
            1,
            "新 tab 应只有被拆出的那一个 pane。leaves=\(app.testLayoutLeafIDs())"
        )

        let windows = Tmux.out(
            socket: fx.socket,
            args: ["list-windows", "-t", fx.session, "-F", "#{window_id}"]
        )
        .split(whereSeparator: \.isNewline)
        .filter { !$0.isEmpty }
        XCTAssertEqual(windows.count, 2, "tmux 必须真的 \(TmuxWindowCommands.paneToNewWindow)，got=\(windows)")
    }
}
