import AppKit
import XCTest
@testable import MuxtermAppLib

/// Core 独立消费前台进程事实；macOS 不回写 process_name，仍须能显示变化。
final class PaneCmdE2ETests: XCTestCase {
    func testPaneCommandSubscriptionSetsAttentionProcessName() throws {
        let fx = TwoPaneCat(label: "pane-cmd")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))
        XCTAssertTrue(app.testStatusSubscriptionActive(), "attach 后必须启用 tmux format 订阅")

        // refresh-client -B 只保证推送订阅值的变化；fixture 在 attach 前已经
        // 运行 cat，不能把“初始值恰好重发”当成协议契约。先切到 sleep，再
        // 切回 cat，验证两个真实 foreground-command 变化都到达 Core。
        let pane = try XCTUnwrap(fx.panes.first)
        Tmux.ok(socket: fx.socket, args: ["respawn-pane", "-k", "-t", pane, "/bin/sleep 30"])
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                AppE2E.pump(30)
                return app.testAttentionProcessNames().values.contains { name in
                    name.lowercased().contains("sleep")
                }
            },
            "pane-cmd 订阅必须先收到 sleep。got=\(app.testAttentionProcessNames())"
        )

        Tmux.ok(socket: fx.socket, args: ["respawn-pane", "-k", "-t", pane, "/bin/cat"])
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
