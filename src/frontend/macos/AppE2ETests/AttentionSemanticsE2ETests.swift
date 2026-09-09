import AppKit
import XCTest
@testable import MuxtermAppLib

/// blocked 看见后标记已读并离开 Attention；TOML 正则能再次点亮后台 pane。
final class AttentionSemanticsE2ETests: XCTestCase {
    func testBlockedLeavesAttentionAfterViewAndRegexLightsAgain() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-attn-\(ProcessInfo.processInfo.processIdentifier)")
        try FileManager.default.createDirectory(at: tmp.appendingPathComponent("muxterm"), withIntermediateDirectories: true)
        let cfg = tmp.appendingPathComponent("muxterm/config.toml")
        try """
        [attention]
        enabled = true
        debounce_ms = 50
        blocked_regex = ["NEED_INPUT"]
        """.write(to: cfg, atomically: true, encoding: .utf8)
        setenv("XDG_CONFIG_HOME", tmp.path, 1)
        defer {
            unsetenv("XDG_CONFIG_HOME")
            try? FileManager.default.removeItem(at: tmp)
        }

        let fx = TwoPaneCat(label: "gtk-attn-sem")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 2), "attach 后应有 2 个 pane")
        let pane0 = UInt32(fx.panes[0].replacingOccurrences(of: "%", with: "")) ?? 0
        let pane1 = UInt32(fx.panes[1].replacingOccurrences(of: "%", with: "")) ?? 1
        app.testSwitchPane(pane0)
        AppE2E.pump(50)

        fx.sendBelOnBackground()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testBlockedCount() >= 1
            },
            "后台 pane 真 BEL 必须点亮 blocked。count=\(app.testBlockedCount())"
        )

        app.testSwitchPane(pane1)
        AppE2E.pump(80)
        app.testPollOnce()
        app.testBecameVisible(pane1)
        XCTAssertEqual(
            app.testBlockedCount(),
            0,
            "切到 blocked pane 后已读，必须离开 Attention。count=\(app.testBlockedCount())"
        )

        app.testSendInput(Data("x".utf8))
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testBlockedCount() == 0
            },
            "已读 pane 输入后仍不得重新进入 Attention。count=\(app.testBlockedCount())"
        )

        app.testSwitchPane(pane0)
        AppE2E.pump(50)
        Tmux.ok(socket: fx.socket, args: ["send-keys", "-t", fx.panes[1], "-l", "NEED_INPUT"])
        Tmux.ok(socket: fx.socket, args: ["send-keys", "-t", fx.panes[1], "Enter"])
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testBlockedCount() >= 1
            },
            "后台 pane 写出 TOML 正则 NEED_INPUT 必须再点亮 blocked（不许要求 BEL）。count=\(app.testBlockedCount())"
        )
    }
}
