import XCTest
@testable import MuxtermChrome

final class SidebarSectionSplitLayoutTests: XCTestCase {
    func testZeroBoundsDoesNotAssignFrames() {
        XCTAssertNil(
            SidebarSectionSplitLayout.heights(
                boundsHeight: 0,
                dividerThickness: 1,
                expanded: [true, true, true, false],
                currentHeights: [0, 0, 0, 0]
            ),
            "高度为 0 时不能排 frame，否则会出现 {0,-25} 乱序"
        )
    }

    func testDefaultExpandedLayoutFillsTheSidebar() throws {
        let heights = SidebarSectionSplitLayout.heights(
            boundsHeight: 640,
            dividerThickness: 1,
            expanded: [true, true, true, false],
            currentHeights: [0, 0, 0, 0]
        )
        let values = try XCTUnwrap(heights)
        XCTAssertEqual(values.count, 4)
        XCTAssertEqual(values[3], 26, accuracy: 0.5)
        XCTAssertGreaterThan(values[0], 80)
        XCTAssertGreaterThan(values[1], 80)
        XCTAssertGreaterThan(values[2], 80)
        XCTAssertEqual(values.reduce(0, +), 637, accuracy: 0.5)
    }

    func testAllCollapsedKeepsHeadersAndPushesSlackToTheLastPane() throws {
        let heights = SidebarSectionSplitLayout.heights(
            boundsHeight: 640,
            dividerThickness: 1,
            expanded: [false, false, false, false],
            currentHeights: [26, 26, 26, 26]
        )
        let values = try XCTUnwrap(heights)
        XCTAssertEqual(values[0], 26, accuracy: 0.5)
        XCTAssertEqual(values[1], 26, accuracy: 0.5)
        XCTAssertEqual(values[2], 26, accuracy: 0.5)
        XCTAssertEqual(values[3], 640 - 3 - 78, accuracy: 0.5)
    }

    func testFramesCoverTheSplitViewInArrangedOrder() {
        let heights: [CGFloat] = [200, 200, 200, 37]
        let frames = SidebarSectionSplitLayout.frames(
            bounds: CGSize(width: 240, height: 640),
            dividerThickness: 1,
            heights: heights,
            flipped: false
        )
        XCTAssertEqual(frames.count, 4)
        XCTAssertEqual(frames[0].maxY, 640, accuracy: 0.5)
        XCTAssertEqual(frames[3].minY, 0, accuracy: 0.5)
        for index in 0..<(frames.count - 1) {
            XCTAssertGreaterThan(
                frames[index].minY,
                frames[index + 1].minY,
                "非 flipped 的 arranged 顺序必须从上到下"
            )
        }
        XCTAssertEqual(frames.map(\.height).reduce(0, +), 637, accuracy: 0.5)
    }
}
