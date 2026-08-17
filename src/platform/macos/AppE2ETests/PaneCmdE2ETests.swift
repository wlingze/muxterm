import AppKit
import XCTest
@testable import MuxtermAppLib

/// Linux 把 `muxterm.pane-cmd` 订阅写进 `AttentionEngine.set_process_name`，
/// 注意力列表才能显示 cat/codex/sleep。macOS poll 到 STATUS_SUBSCRIPTION
/// 时只更新了 status-left/right，没转 process_name。
final class PaneCmdE2ETests: XCTestCase {
    func testPaneCommandSubscriptionSetsAttentionProcessName() throws {
        let fx = TwoPaneCat(label: "pane-cmd")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let found = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            AppE2E.pump(30)
            let names = app.testAttentionProcessNames().values.map { $0.lowercased() }
            return names.contains(where: { $0.contains("cat") })
        }
        XCTAssertTrue(
            found,
            "/bin/cat pane 的 process_name 必须来自 muxterm.pane-cmd。got=\(app.testAttentionProcessNames())"
        )
    }
}
