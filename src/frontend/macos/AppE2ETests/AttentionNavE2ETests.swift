import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 注意力：↑↓ 选行，Enter 跳转关面板，去掉 peek；Cmd-Enter 开独立 replica overlay。
final class AttentionNavE2ETests: XCTestCase {
    func testEnterJumpsWithoutPeekAndCmdEnterOpensOverlay() throws {
        let fx = TwoPaneCat(label: "attn-nav")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        // 再开一个后台 pane 并 BEL，保证 ↑↓ 在本地和 CI 都会走到（单行时
        // 旧测试会跳过方向键，CI 上 working+blocked 两行才暴露 handleKey 漏键）。
        Tmux.ok(socket: fx.socket, args: ["split-window", "-v", "-t", fx.panes[0], "/bin/cat"])
        XCTAssertTrue(app.waitReady(minLeaves: 3), "第三个 pane 必须进布局")
        let extraPane = Tmux.out(socket: fx.socket, args: [
            "list-panes", "-t", fx.session, "-F", "#{pane_id}",
        ])
        .split(whereSeparator: \.isNewline)
        .map(String.init)
        .first { $0 != fx.panes[0] && $0 != fx.panes[1] } ?? ""
        XCTAssertFalse(extraPane.isEmpty, "split-window 必须给出第三个 pane id")

        let pane0 = UInt32(fx.panes[0].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 0
        let bgPane = UInt32(fx.panes[1].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 1
        app.testSwitchPane(pane0)
        fx.sendBelOnBackground()
        Tmux.sendHex(socket: fx.socket, target: extraPane, bytes: [0x07])
        Tmux.ok(socket: fx.socket, args: ["send-keys", "-t", extraPane, "Enter"])
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testOpenAttentionPanel()
                app.unifiedPanel.refreshData()
                return app.testAttentionRowCount() >= 2
            },
            "两个后台 BEL 必须各占一行。rows=\(app.testAttentionRowCount())"
        )

        app.testOpenAttentionPanel()
        AppE2E.pump(80)
        XCTAssertTrue(app.testAttentionPanelOpen())
        XCTAssertGreaterThan(app.testAttentionRowCount(), 0)
        app.attentionPanel.testSelectFirstRow()
        AppE2E.pump(40)
        XCTAssertNil(
            app.attentionPanel.testPeekView(),
            "注意力列表不得再渲染 muxterm.attention.peek"
        )

        let leavesBefore = app.testLayoutLeafIDs().count
        app.attentionPanel.window?.makeKeyAndOrderFront(nil)
        AppE2E.pump(20)

        XCTAssertGreaterThanOrEqual(app.testAttentionRowCount(), 2)
        let start = app.attentionPanel.testSelectedRow()
        let down = try XCTUnwrap(app.testMakeArrowEvent(down: true), "必须能构造 ↓")
        XCTAssertTrue(app.testDispatchKeyEvent(down), "注意力面板 ↓ 必须被 handleKey 消费")
        AppE2E.pump(40)
        XCTAssertNotEqual(
            app.attentionPanel.testSelectedRow(),
            start,
            "↓ 必须移动选中行"
        )
        // overlay / Enter 仍针对带 bgToken 的后台 pane，不能停在刚移到的那一行。
        app.unifiedPanel.testSelectAttentionPane(bgPane)

        let cmdEnter = try XCTUnwrap(app.testMakeCmdEnterEvent(), "必须能构造 Cmd-Enter")
        XCTAssertTrue(app.testDispatchKeyEvent(cmdEnter), "注意力面板 Cmd-Enter 必须被消费")
        AppE2E.pump(80)
        XCTAssertTrue(
            app.testReplyOverlayVisible(),
            "Cmd-Enter 必须打开 \(CmdEnterRouting.overlayIdentifier)，且不改主布局 SwiftTerm"
        )
        XCTAssertEqual(
            app.testLayoutLeafIDs().count,
            leavesBefore,
            "replica overlay 不得打乱主布局 leaf。leaves=\(app.testLayoutLeafIDs())"
        )
        XCTAssertTrue(
            app.testReplyOverlayText().contains(fx.bgToken)
                || AppE2E.wait(timeout: 3) {
                    app.testPollOnce()
                    app.testFlushFeeds()
                    return app.testReplyOverlayText().contains(fx.bgToken)
                },
            "overlay 必须是该 pane 的 replica（含 \(fx.bgToken)）。got=\(app.testReplyOverlayText())"
        )

        let overlayToken = "OVERLAY_IO_\(ProcessInfo.processInfo.processIdentifier)"
        app.testSendInput(Data(overlayToken.utf8))
        Tmux.waitCapture(
            socket: fx.socket,
            target: fx.panes[1],
            needle: overlayToken,
            timeout: AppE2E.featureTimeout
        )
        XCTAssertEqual(app.testLayoutLeafIDs().count, leavesBefore)

        XCTAssertTrue(app.testDispatchKeyEvent(cmdEnter), "再按 Cmd-Enter 必须关 overlay")
        AppE2E.pump(80)
        XCTAssertFalse(app.testReplyOverlayVisible(), "第二次 Cmd-Enter 必须退出 overlay")

        app.testOpenAttentionPanel()
        AppE2E.pump(40)
        app.unifiedPanel.testSelectAttentionPane(bgPane)
        app.attentionPanel.window?.makeKeyAndOrderFront(nil)
        let enter = try XCTUnwrap(app.testMakeReturnEvent())
        _ = app.testDispatchKeyEvent(enter)
        XCTAssertFalse(app.testAttentionPanelOpen(), "Enter 必须跳转并关掉面板")
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testActivePaneID() == bgPane
            },
            "Enter 必须切到该注意力 pane（期望 \(bgPane)，当前 \(app.testActivePaneID())）"
        )
    }
}
