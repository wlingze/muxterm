import XCTest
@testable import MuxtermChrome

final class FlatChromeTests: XCTestCase {
    func testFlatMetricsMatchArchitectureBudget() {
        XCTAssertLessThanOrEqual(FlatChrome.tabBarHeight, 24)
        XCTAssertLessThanOrEqual(FlatChrome.statusBarHeight, 20)
        XCTAssertEqual(FlatChrome.splitDividerThickness, 1)
        XCTAssertEqual(FlatChrome.activePaneBorderWidth, 1)
        XCTAssertFalse(FlatChrome.tabBarHeight > 24, "tab 栏不得高于 ARCHITECTURE 预算")
    }

    func testStatusTextKeepsUITestTokens() {
        let text = FlatChrome.statusText(
            status: "connected",
            tabCount: 2,
            paneCount: 3,
            activePane: 12
        )
        XCTAssertTrue(text.contains("connected"))
        XCTAssertTrue(text.contains("tabs: 2"))
        XCTAssertTrue(text.contains("panes: 3"))
        XCTAssertTrue(text.contains("pane: @12"))
        // 去掉冗余「tab: name |」层级
        XCTAssertFalse(text.contains("tab:"))
        XCTAssertFalse(text.contains("|"))
    }
}

final class KeyBindingsTests: XCTestCase {
    func testCmdBracketPaneSwitchMapping() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "[")),
            .prevPane
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "]")),
            .nextPane
        )
    }

    func testCmdBracketIgnoresShift() {
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, shift: true, key: "[")))
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, shift: true, key: "]")))
    }

    func testAltBracketAlsoSwitchesPane() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "[")),
            .prevPane
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(option: true, key: "]")),
            .nextPane
        )
    }

    func testCommonCmdShortcutsUnchanged() {
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "t")), .newTab)
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "d")), .splitHorizontal)
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, shift: true, key: "d")),
            .splitVertical
        )
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "w")), .closeWindow)
        XCTAssertEqual(KeyBindings.action(for: KeyChord(command: true, key: "2")), .switchTab(2))
        XCTAssertEqual(KeyBindings.action(for: KeyChord(control: true, key: "d")), .closePane)
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, shift: true, key: "p")),
            .commandPalette
        )
    }
}


final class PaneOutputCursorTests: XCTestCase {
    func testInitialSnapshotAlreadyContainingEventDoesNotFeedEventTwice() {
        var cursor = PaneOutputCursor()
        let snapshot = Data("prompt% ls\r\n".utf8)
        let event = Data("prompt% ls\r\n".utf8)

        XCTAssertEqual(cursor.initial(snapshot: snapshot), snapshot)
        XCTAssertEqual(cursor.incremental(event: event, snapshot: snapshot), Data())
    }

    func testLaterIncrementalEventIsFedExactlyOnce() {
        var cursor = PaneOutputCursor()
        let snapshot = Data("prompt% ".utf8)
        let event = Data("echo UNIQUE_INPUT\r\nUNIQUE_INPUT\r\n".utf8)

        XCTAssertEqual(cursor.initial(snapshot: snapshot), snapshot)
        XCTAssertEqual(cursor.incremental(event: event, snapshot: snapshot + event), event)
    }

    func testBoundedBufferTrimResetsCursorAndRefeedsTail() {
        var cursor = PaneOutputCursor()
        let old = Data(String(repeating: "x", count: 100).utf8)
        XCTAssertEqual(cursor.initial(snapshot: old), old)

        // 有界缓冲被裁剪后，snapshot 变短：应重置并重放当前尾部，而不是丢弃。
        let trimmed = Data(String(repeating: "y", count: 20).utf8)
        XCTAssertEqual(cursor.incremental(event: trimmed, snapshot: trimmed), trimmed)
    }
}
