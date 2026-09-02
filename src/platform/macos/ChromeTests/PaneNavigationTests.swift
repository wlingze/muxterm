import XCTest
@testable import MuxtermChrome

final class PaneNavigationTests: XCTestCase {
    func testNextAndPreviousStayInsideCurrentTab() {
        let panes: [UInt32] = [11, 12, 13]

        XCTAssertEqual(
            PaneNavigation.target(paneIDs: panes, activePaneID: 12, offset: 1),
            13
        )
        XCTAssertEqual(
            PaneNavigation.target(paneIDs: panes, activePaneID: 12, offset: -1),
            11
        )
    }

    func testNavigationWraps() {
        let panes: [UInt32] = [21, 22]

        XCTAssertEqual(
            PaneNavigation.target(paneIDs: panes, activePaneID: 22, offset: 1),
            21
        )
        XCTAssertEqual(
            PaneNavigation.target(paneIDs: panes, activePaneID: 21, offset: -1),
            22
        )
    }

    func testMissingActivePaneFallsBackToFirstPane() {
        XCTAssertEqual(
            PaneNavigation.target(paneIDs: [31, 32], activePaneID: 99, offset: 1),
            31
        )
        XCTAssertNil(PaneNavigation.target(paneIDs: [], activePaneID: 0, offset: 1))
    }

    func testZoomedLayoutNavigatesAcrossAllPanesInTheCurrentTab() {
        let visibleLayout: [UInt32] = [41]
        let snapshotPanes: [UInt32] = [41, 42, 43]

        XCTAssertEqual(
            PaneNavigation.navigationPaneIDs(
                layoutPaneIDs: visibleLayout,
                paneIDs: snapshotPanes
            ),
            snapshotPanes
        )
        XCTAssertEqual(
            PaneNavigation.target(
                paneIDs: PaneNavigation.navigationPaneIDs(
                    layoutPaneIDs: visibleLayout,
                    paneIDs: snapshotPanes
                ),
                activePaneID: 41,
                offset: 1
            ),
            42
        )
    }

    func testNonZoomedLayoutKeepsItsGeometryOrder() {
        let layout: [UInt32] = [52, 51, 53]
        let snapshotPanes: [UInt32] = [51, 52, 53]

        XCTAssertEqual(
            PaneNavigation.navigationPaneIDs(
                layoutPaneIDs: layout,
                paneIDs: snapshotPanes
            ),
            layout
        )
    }
}
