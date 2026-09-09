import XCTest
@testable import MuxtermChrome

final class CmdEnterRoutingTests: XCTestCase {
    func testMainWindowKeepsTmuxZoom() {
        XCTAssertEqual(CmdEnterRouting.action(on: .mainWindow), .toggleTmuxZoom)
    }

    func testAttentionPanelOpensReplicaOverlay() {
        XCTAssertEqual(CmdEnterRouting.action(on: .attentionPanel), .openReplyOverlay)
        XCTAssertEqual(CmdEnterRouting.overlayIdentifier, "muxterm.replyOverlay")
    }

    func testOverlayTogglesClosed() {
        XCTAssertEqual(CmdEnterRouting.action(on: .replyOverlay), .closeReplyOverlay)
    }
}
