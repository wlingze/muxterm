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

        let pane0 = UInt32(fx.panes[0].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 0
        app.testSwitchPane(pane0)
        fx.sendBelOnBackground()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testOpenAttentionPanel()
                app.unifiedPanel.refreshData()
                return app.testBlockedCount() >= 1 || app.testAttentionRowCount() >= 1
            },
            "后台 BEL 必须进注意力"
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

        if app.testAttentionRowCount() >= 2 {
            let start = app.attentionPanel.testSelectedRow()
            if let down = app.testMakeArrowEvent(down: true) {
                _ = app.testDispatchKeyEvent(down)
                AppE2E.pump(40)
                XCTAssertNotEqual(
                    app.attentionPanel.testSelectedRow(),
                    start,
                    "↓ 必须移动选中行"
                )
            }
        }

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
        app.attentionPanel.testSelectFirstRow()
        app.attentionPanel.window?.makeKeyAndOrderFront(nil)
        let enter = try XCTUnwrap(app.testMakeReturnEvent())
        _ = app.testDispatchKeyEvent(enter)
        XCTAssertFalse(app.testAttentionPanelOpen(), "Enter 必须跳转并关掉面板")
        let target = UInt32(fx.panes[1].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 1
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testActivePaneID() == target
            },
            "Enter 必须切到该注意力 pane（期望 \(target)，当前 \(app.testActivePaneID())）"
        )
    }
}
