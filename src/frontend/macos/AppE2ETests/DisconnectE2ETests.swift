import AppKit
import XCTest
@testable import MuxtermAppLib

/// W16b：隔离 tmux server 被杀后，窗口留下最后一帧 + 断线水印，不关窗、不弹模态。
///
/// 当前生产路径对 Exited(4) 会 `closeSessionWindow()`——本测试必须先红。
final class DisconnectE2ETests: XCTestCase {
    func testDisconnectKeepsTerminalAndShowsWatermark() throws {
        let fx = OnePaneCat(label: "gtk-disc")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { if app.testWindowVisible() { app.testShutdown() } }

        XCTAssertTrue(app.waitTerminalContains(fx.token), "断开前 SwiftTerm 必须已有 \(fx.token)")

        Tmux.killServer(fx.socket)

        _ = AppE2E.wait(timeout: 10) {
            app.testPollOnce()
            AppE2E.pump(30)
            return app.testDisconnectOverlayVisible()
        }

        XCTAssertTrue(
            app.testWindowVisible(),
            "隔离 tmux server 被杀后主窗口必须留下，不能把最后一帧一起关掉"
        )
        app.testFlushFeeds()
        XCTAssertTrue(
            app.testActivePaneTerminalText().contains(fx.token)
                || app.testAllVisibleTerminalText().contains(fx.token),
            "断线后 SwiftTerm 必须保留最后一帧 \(fx.token)，禁止 reset 清空"
        )
        XCTAssertTrue(
            app.testDisconnectOverlayVisible(),
            "muxterm.disconnectOverlay 必须可见"
        )
        XCTAssertEqual(
            app.window?.sheets.count ?? 0,
            0,
            "断线不得弹 NSAlert / sheet"
        )
    }
}
