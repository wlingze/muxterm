import XCTest
@testable import MuxtermChrome

final class TmuxWindowCommandsTests: XCTestCase {
    func testReorderIsMoveWindow() {
        XCTAssertEqual(TmuxWindowCommands.reorderWindows, "move-window")
        XCTAssertEqual(
            TmuxWindowCommands.moveWindowArgs(fromIndex: 1, toIndex: 3),
            ["move-window", "-s", ":1", "-t", ":3"]
        )
    }

    func testPaneToTabIsBreakPane() {
        XCTAssertEqual(TmuxWindowCommands.paneToNewWindow, "break-pane")
        XCTAssertEqual(
            TmuxWindowCommands.breakPaneArgs(pane: "%12"),
            ["break-pane", "-s", "%12"]
        )
    }

    func testMovePaneIsNotASecondWindowHierarchy() {
        XCTAssertEqual(TmuxWindowCommands.movePane, "move-pane")
    }
}
