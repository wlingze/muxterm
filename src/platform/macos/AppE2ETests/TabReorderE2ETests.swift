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
        app.testReorderTab(from: first, toIndex: 2)
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
            "GUI tab 顺序必须等于 tmux \(TmuxWindowCommands.reorderWindows) 之后的 window 列表"
        )
    }
}
