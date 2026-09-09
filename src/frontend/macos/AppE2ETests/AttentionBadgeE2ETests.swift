import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 对标 `linux_attention_e2e`：BEL → 状态栏红点。
final class AttentionBadgeE2ETests: XCTestCase {
    func testBelPaintsBadge() throws {
        let fx = TwoPaneCat(label: "attn-badge")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let pane0 = UInt32(fx.panes[0].replacingOccurrences(of: "%", with: "")) ?? 0
        app.testSwitchPane(pane0)
        fx.sendBelOnBackground()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testBlockedCount() >= 1
            },
            "BEL 后 blocked_count 应 ≥ 1"
        )
        app.content.statusBar.setAttention(StatusBarAttention(count: app.testBlockedCount()))
        AppE2E.pump(40)
        let value = app.content.statusBar.findAttentionValue()
        XCTAssertNotEqual(value, "0", "状态栏注意力位必须激活，value=\(value)")
        XCTAssertEqual(app.content.statusBar.testAttentionSymbolName(), "bell.fill")
    }
}

private extension StatusBarView {
    func findAttentionValue() -> String {
        subviews.first { $0.accessibilityIdentifier() == "muxterm.statusAttention" }?
            .accessibilityValue() as? String ?? testAttentionCountLabel()
    }
}
