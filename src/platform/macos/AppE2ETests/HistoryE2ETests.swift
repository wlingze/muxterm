import AppKit
import XCTest
@testable import MuxtermAppLib

/// W16a：离屏历史可搜索、滚到顶可见、回底按钮回到尾部。
final class HistoryE2ETests: XCTestCase {
    func testAttachRestoresOffscreenHistoryAndJumpLatest() throws {
        let fx = OffscreenHistory(label: "gtk-hist")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 1), "attach 后应有 pane 控件")
        XCTAssertTrue(
            app.waitTerminalContains(fx.tailMark, timeout: AppE2E.featureTimeout),
            "可见尾标 \(fx.tailMark) 必须在 SwiftTerm 里"
        )

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return !app.testSearchAll(fx.token).isEmpty
            },
            "search_all 必须找到滚出可见区的 \(fx.token)"
        )

        let pane = try XCTUnwrap(app.testLayoutLeafIDs().first, "至少 1 个 pane")
        app.testSetPaneViewport(1000)
        AppE2E.pump(80)
        app.testFlushFeeds()
        XCTAssertTrue(
            app.testPaneTerminalText(pane).contains(fx.token)
                || app.testAllVisibleTerminalText().contains(fx.token),
            "滚到顶之后 SwiftTerm 必须能看见离屏历史 \(fx.token)"
        )
        XCTAssertTrue(app.testJumpLatestVisible(), "向上滚动后必须出现回底按钮 muxterm.jumpLatest")
        XCTAssertGreaterThan(app.testPaneViewport(), 0, "滚离底部后 viewport 应 > 0")

        app.testClickJumpLatest()
        AppE2E.pump(80)
        app.testFlushFeeds()
        let after = app.testPaneTerminalText(pane)
        XCTAssertFalse(
            after.contains(fx.token),
            "点回底之后可见区应回到尾部，不应再显示离屏 token。got=\(after)"
        )
        XCTAssertTrue(after.contains(fx.tailMark), "点回底之后可见区应含尾标 \(fx.tailMark)")
        XCTAssertFalse(app.testJumpLatestVisible(), "回底后按钮应隐藏")
    }
}
