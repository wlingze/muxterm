import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 把 pane 拖成新 tab = tmux `break-pane`（iTerm2 breakOutWindowPane / MoveSessionToNewTab）。
final class BreakPaneE2ETests: XCTestCase {
    func testPaneTitlesAppearOnlyForSplitTabs() throws {
        let single = OnePaneCat(label: "pane-title-single")
        let singleApp = try AppE2E.attachWindow(socket: single.socket, session: single.session)
        XCTAssertTrue(singleApp.waitReady(minLeaves: 1))
        let onlyPane = try XCTUnwrap(singleApp.testLayoutLeafIDs().first)
        XCTAssertFalse(singleApp.testPaneTitleVisible(onlyPane))
        singleApp.testShutdown()

        let split = TwoPaneCat(label: "pane-title-split")
        let splitApp = try AppE2E.attachWindow(socket: split.socket, session: split.session)
        defer { splitApp.testShutdown() }
        XCTAssertTrue(splitApp.waitReady(minLeaves: 2))
        XCTAssertTrue(
            splitApp.testLayoutLeafIDs().allSatisfy { splitApp.testPaneTitleVisible($0) },
            "多 pane tab 的每个 Surface 都应显示布局内标题条"
        )
        for pane in splitApp.testLayoutLeafIDs() {
            let title = try XCTUnwrap(splitApp.testPaneTitle(pane))
            XCTAssertFalse(title.isEmpty)
            XCTAssertFalse(title.hasPrefix("Pane @"), "标题应来自 Core PaneInfo，而不是 pane id")
            XCTAssertEqual(
                splitApp.testPaneAllocation(pane).height - splitApp.testPaneTerminalHeight(pane),
                22,
                accuracy: 1,
                "标题条必须占据独立布局行，不能悬浮覆盖 terminal"
            )
        }
    }

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
                    && app.testLayoutLeafIDs().count == 1
            },
            "break-pane 后必须变成 2 个 tab，且新 tab 布局完成。got=\(app.testTabAndPaneCounts()) leaves=\(app.testLayoutLeafIDs())"
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
        XCTAssertEqual(windows.count, 2, "tmux 必须真的执行 break-pane，got=\(windows)")
    }
}
