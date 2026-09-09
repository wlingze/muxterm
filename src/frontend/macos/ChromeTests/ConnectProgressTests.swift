import XCTest
@testable import MuxtermChrome

final class ConnectProgressTests: XCTestCase {
    func testStagesMatchSshAttachLog() {
        XCTAssertEqual(
            ConnectProgressStage.allCases.map(\.rawValue),
            ["resolving", "ssh", "list-sessions", "attach", "capture"]
        )
    }

    func testOverlayIdentifierIsStable() {
        XCTAssertEqual(ConnectProgress.identifier, "muxterm.connectProgress")
        XCTAssertEqual(
            ConnectProgress.accessibilityValue(stage: .listSessions),
            "list-sessions"
        )
    }
}
