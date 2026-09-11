import XCTest
@testable import MuxtermChrome

final class CommandMarkRailTests: XCTestCase {
    func testFailTickSitsAboveNewerSuccess() {
        let marks = [
            CommandMarkTick(seq: 1, command: "ok", exitCode: 0, offset: 0),
            CommandMarkTick(seq: 2, command: "fail", exitCode: 1, offset: 40),
        ]
        let placed = CommandMarkRailLayout.place(marks: marks, maxOffset: 40, height: 200)
        XCTAssertEqual(placed.count, 2)
        let ok = placed.first { $0.mark.kind == .ok }
        let fail = placed.first { $0.mark.kind == .fail }
        XCTAssertNotNil(ok)
        XCTAssertNotNil(fail)
        XCTAssertGreaterThan(fail!.y, ok!.y, "更早的失败命令必须画在滚动轨更靠上的位置")
    }

    func testLatestMarkHugsTheBottom() {
        let marks = [
            CommandMarkTick(seq: 9, command: "ls", exitCode: 0, offset: 0),
        ]
        let placed = CommandMarkRailLayout.place(marks: marks, maxOffset: 100, height: 200)
        XCTAssertEqual(placed[0].y, CommandMarkRailLayout.inset)
    }

    func testHitTestPrefersNearbyFailTick() {
        let marks = [
            CommandMarkTick(seq: 1, command: "ok", exitCode: 0, offset: 20),
            CommandMarkTick(seq: 2, command: "fail", exitCode: 2, offset: 21),
        ]
        let placed = CommandMarkRailLayout.place(marks: marks, maxOffset: 40, height: 200)
        let y = placed.first { $0.mark.kind == .fail }!.y
        let hit = CommandMarkRailLayout.hitTest(ticks: placed, y: y)
        XCTAssertEqual(hit?.mark.kind, .fail)
        XCTAssertEqual(hit?.mark.command, "fail")
    }

    func testTrackBottomJumpsToLatest() {
        XCTAssertEqual(
            CommandMarkRailLayout.trackOffset(y: 8, height: 200, maxOffset: 80),
            0
        )
    }

    func testTrackTopJumpsTowardOldest() {
        let offset = CommandMarkRailLayout.trackOffset(y: 190, height: 200, maxOffset: 80)
        XCTAssertGreaterThan(offset, 60)
    }

    func testRailHiddenWithoutMarks() {
        XCTAssertFalse(CommandMarkRailLayout.isVisible(markCount: 0))
        XCTAssertTrue(CommandMarkRailLayout.isVisible(markCount: 2))
        XCTAssertEqual(CommandMarkRailLayout.width(expanded: false), 8)
        XCTAssertLessThanOrEqual(CommandMarkRailLayout.width(expanded: true), 14)
    }

    func testJumpLatestCaptionMatchesVisionCopy() {
        XCTAssertEqual(JumpLatestCaption.title(unseenLines: 0, latestWord: "最新"), "↓ 最新")
        XCTAssertEqual(
            JumpLatestCaption.title(unseenLines: 37, latestWord: "最新"),
            "↓ 最新 · +37"
        )
        XCTAssertEqual(
            JumpLatestCaption.title(unseenLines: 4, latestWord: "Latest"),
            "↓ Latest · +4"
        )
    }

    func testScrollDownNearBottomSnapsToLatest() {
        XCTAssertTrue(JumpLatestCaption.shouldSnapToLatest(scrollPosition: 0.95))
        XCTAssertFalse(JumpLatestCaption.shouldSnapToLatest(scrollPosition: 0.4))
        XCTAssertFalse(
            JumpLatestCaption.shouldSnapToLatest(scrollPosition: 1.0),
            "已经在底部时不要重复吸附"
        )
    }
}
