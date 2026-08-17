import AppKit
import XCTest
@testable import MuxtermAppLib

/// W14：搜索跳转 / BEL 通知 / peek 回复 / Done 通知 / mock-codex 末帧 / tail -f。
final class FeatureE2ETests: XCTestCase {
    func testFeatureSearchNotifyCodexTail() throws {
        let fx = TwoPaneCat(label: "gtk-feat")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 2), "attach 后应有 2 个 pane，leaves=\(app.testLayoutLeafIDs())")

        let hits = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return !app.testSearchAll(fx.searchToken).isEmpty
        }
        XCTAssertTrue(hits, "search_all 必须找到播种 token \(fx.searchToken)")

        app.testOpenSearchPanel()
        AppE2E.pump(80)
        XCTAssertTrue(app.testSearchPanelOpen(), "搜索面板应打开")
        app.testSetSearchQuery(fx.searchToken)
        AppE2E.pump(120)
        XCTAssertGreaterThan(app.testSearchHitCount(), 0, "必须出现 muxterm.search.hit-*")
        app.testActivateFirstSearchHit()
        AppE2E.pump(80)
        app.testPollOnce()
        XCTAssertFalse(app.testSearchPanelOpen(), "搜索跳转后面板必须关掉")
        XCTAssertTrue(
            app.waitTerminalContains(fx.searchToken, timeout: AppE2E.featureTimeout),
            "跳转后 SwiftTerm 必须含搜索 token \(fx.searchToken)"
        )

        let pane0 = UInt32(fx.panes[0].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 0
        app.testSwitchPane(pane0)
        app.testPollOnce()
        AppE2E.pump(40)
        fx.sendBelOnBackground()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testNotificationsRecorded().contains { n in
                    let l = n.lowercased()
                    return l.contains("attention") || l.contains("blocked") || n.contains("需要")
                }
            },
            "真 %output BEL 必须 notify_blocked。实际: \(app.testNotificationsRecorded())"
        )

        app.testOpenAttentionPanel()
        AppE2E.pump(80)
        XCTAssertTrue(app.testAttentionPanelOpen(), "注意力面板应打开")
        XCTAssertGreaterThan(app.testAttentionRowCount(), 0, "应有注意力行")
        app.attentionPanel.testSelectFirstRow()
        AppE2E.pump(80)
        XCTAssertNotNil(app.attentionPanel.testPeekView(), "选中后必须出现 muxterm.attention.peek 小终端")
        XCTAssertTrue(
            app.attentionPanel.testPeekText().contains(fx.bgToken),
            "选中后小终端必须是该 pane 画面（含 \(fx.bgToken)）。peek=\(app.attentionPanel.testPeekText())"
        )
        // 快速回复必须进后台 pane（对标 linux_feature_e2e W15_REPLY）。
        app.testSwitchPane(UInt32(fx.panes[1].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 1)
        app.testSendInput(Data("W15_REPLY".utf8))
        Tmux.waitCapture(socket: fx.socket, target: fx.panes[1], needle: "W15_REPLY", timeout: AppE2E.featureTimeout)

        app.testSwitchPane(pane0)
        app.testPollOnce()
        fx.sendOsc133DoneOnBackground()
        var sawDone = false
        var sawNotify = false
        _ = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            AppE2E.pump(30)
            if app.testDoneCount() >= 1 { sawDone = true }
            if app.testNotificationsRecorded().contains(where: {
                let l = $0.lowercased()
                return l.contains("done") || l.contains("complete") || l.contains("finished") || $0.contains("完成")
            }) {
                sawNotify = true
            }
            return sawDone && sawNotify
        }
        XCTAssertTrue(sawDone, "后台 OSC 133 D 必须让 done ≥ 1")
        XCTAssertTrue(
            sawNotify,
            "任务完成必须走 notify_done。实际: \(app.testNotificationsRecorded())"
        )

        fx.respawnMockCodex(onPane: 0)
        Tmux.waitCapture(socket: fx.socket, target: fx.panes[0], needle: "MOCK_CODEX_DONE", timeout: AppE2E.featureTimeout)
        app.testSetPaneViewport(1000)
        AppE2E.pump(40)
        XCTAssertTrue(
            app.waitTerminalContains("TOKEN_HEADER", timeout: AppE2E.featureTimeout)
                && app.waitTerminalContains("TOKEN_PROMPT", timeout: AppE2E.featureTimeout),
            "mock-codex 末帧必须进 SwiftTerm（TOKEN_HEADER + TOKEN_PROMPT）。vte=\(app.testActivePaneTerminalText())"
        )

        let log = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-mac-tail-\(fx.session).log")
        try? FileManager.default.removeItem(at: log)
        try "TAIL_BOOT\n".write(to: log, atomically: true, encoding: .utf8)
        Tmux.ok(socket: fx.socket, args: [
            "respawn-pane", "-k", "-t", fx.panes[1], "/usr/bin/tail -f \(log.path)",
        ])
        Tmux.waitCapture(socket: fx.socket, target: fx.panes[1], needle: "TAIL_BOOT", timeout: AppE2E.featureTimeout)
        if let handle = FileHandle(forWritingAtPath: log.path) {
            handle.seekToEndOfFile()
            handle.write(Data("TAIL_FOLLOW_TOKEN\n".utf8))
            try handle.close()
        }
        Tmux.waitCapture(
            socket: fx.socket,
            target: fx.panes[1],
            needle: "TAIL_FOLLOW_TOKEN",
            timeout: AppE2E.featureTimeout
        )
        XCTAssertTrue(
            app.waitTerminalContains("TAIL_FOLLOW_TOKEN", timeout: AppE2E.featureTimeout),
            "tail -f 追加行必须出现在 SwiftTerm。vte=\(app.testAllVisibleTerminalText())"
        )
        try? FileManager.default.removeItem(at: log)
    }
}
