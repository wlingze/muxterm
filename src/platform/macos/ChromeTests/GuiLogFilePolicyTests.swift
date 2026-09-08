import XCTest
@testable import MuxtermChrome

final class GuiLogFilePolicyTests: XCTestCase {
    func testRedirectsWhenLogFileIsSet() {
        XCTAssertTrue(
            GuiLogFilePolicy.shouldRedirectStandardError(logFile: "test-2026-0908-1051.log")
        )
        XCTAssertTrue(
            GuiLogFilePolicy.shouldRedirectStandardError(logFile: "/tmp/muxterm.log")
        )
    }

    func testDoesNotRedirectWithoutLogFile() {
        XCTAssertFalse(GuiLogFilePolicy.shouldRedirectStandardError(logFile: nil))
        XCTAssertFalse(GuiLogFilePolicy.shouldRedirectStandardError(logFile: ""))
        XCTAssertFalse(GuiLogFilePolicy.shouldRedirectStandardError(logFile: "   "))
    }
}
