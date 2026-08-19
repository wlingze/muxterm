import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 拖 tab 排序 = tmux `move-window`（iTerm2 moveTabAtIndex）。
final class TabReorderE2ETests: XCTestCase {
    func testReorderTabsMovesTmuxWindows() throws {
        let painted = PaintedWorkspace(label: "tab-mv")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))

        let before = app.testTabIDs()
        XCTAssertGreaterThanOrEqual(before.count, 2)
        let first = before[0]
        let last = try XCTUnwrap(before.last)
        app.testReorderTab(from: first, target: last, before: false)
        app.testPollOnce()
        AppE2E.pump(120)

        let after = app.testTabIDs()
        XCTAssertNotEqual(after.first, first, "GUI tab 顺序必须跟着拖动变。before=\(before) after=\(after)")

        let tmuxOrder = Tmux.out(
            socket: painted.socket,
            args: ["list-windows", "-t", painted.session, "-F", "#{window_id}"]
        )
        .split(whereSeparator: \.isNewline)
        .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "@%")) }
        .compactMap { UInt32($0) }
        XCTAssertEqual(
            after,
            tmuxOrder,
            "GUI tab 顺序必须等于 tmux move-window 之后的 window 列表"
        )

        let newFirst = try XCTUnwrap(after.first)
        app.testReorderTab(from: first, target: newFirst, before: true)
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.testTabIDs() == before
            },
            "向左插回时 GUI tab 顺序必须恢复。expected=\(before) got=\(app.testTabIDs())"
        )
        let restoredTmuxOrder = Tmux.out(
            socket: painted.socket,
            args: ["list-windows", "-t", painted.session, "-F", "#{window_id}"]
        )
        .split(whereSeparator: \.isNewline)
        .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "@%")) }
        .compactMap { UInt32($0) }
        XCTAssertEqual(restoredTmuxOrder, before, "向左移动也必须到达真实 tmux")
    }
}
