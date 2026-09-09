import AppKit
import XCTest
@testable import MuxtermAppLib

/// W15b：搜索命中在另一个 tab 时，激活必须切 tab + SwiftTerm 含 token + 关面板。
final class SearchJumpE2ETests: XCTestCase {
    func testSearchHitOnOtherTabSwitchesTabAndClosesPanel() throws {
        let painted = PaintedWorkspace(label: "gtk-sj")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2), "attach 后应有 2 个 tab")
        let tab1 = app.testActiveTabID()
        let tab2 = try XCTUnwrap(app.testTabIDs().first { $0 != tab1 }, "应有第二个 tab")

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.attachTimeout) {
                app.testPollOnce()
                return !app.testSearchAll(painted.tab2Token).isEmpty
            },
            "PaneBuf 必须能搜到 tab2 token \(painted.tab2Token)"
        )

        app.testOpenSearchPanel()
        AppE2E.pump(80)
        app.testSetSearchQuery(painted.tab2Token)
        AppE2E.pump(120)
        XCTAssertGreaterThan(app.testSearchHitCount(), 0, "必须出现 muxterm.search.hit-*")
        app.testActivateFirstSearchHit()
        AppE2E.pump(80)
        app.testPollOnce()
        AppE2E.pump(80)

        XCTAssertFalse(app.testSearchPanelOpen(), "搜索跳转后面板必须关掉")
        XCTAssertEqual(
            app.testActiveTabID(),
            tab2,
            "命中在 tab 2，跳转后当前 tab 必须是 \(tab2)，实际 \(app.testActiveTabID())"
        )
        XCTAssertTrue(
            app.waitTerminalContains(painted.tab2Token),
            "跳转后 SwiftTerm 必须含 \(painted.tab2Token)"
        )
    }

    func testSearchHitOnTabZeroSwitchesBackFromOtherTab() throws {
        let painted = PaintedWorkspace(label: "sj-tab0")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2), "attach 后应有 2 个 tab")
        let tab1 = app.testActiveTabID()
        let tab2 = try XCTUnwrap(app.testTabIDs().first { $0 != tab1 }, "应有第二个 tab")
        let token = painted.tab1Tokens[0]

        app.testSwitchTab(tab2)
        AppE2E.pump(80)
        app.testPollOnce()
        XCTAssertEqual(app.testActiveTabID(), tab2, "先切到 tab 2，才能验证跳回 tab \(tab1)")

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.attachTimeout) {
                app.testPollOnce()
                return !app.testSearchAll(token).isEmpty
            },
            "PaneBuf 必须能搜到 tab1 token \(token)"
        )

        app.testOpenSearchPanel()
        AppE2E.pump(80)
        app.testSetSearchQuery(token)
        AppE2E.pump(120)
        XCTAssertGreaterThan(app.testSearchHitCount(), 0, "必须出现 muxterm.search.hit-*")
        let hits = app.testSearchAll(token)
        XCTAssertEqual(hits.first?.tabId, tab1, "命中 tab_id 必须是 \(tab1)（可以是 0）")
        app.testActivateFirstSearchHit()
        AppE2E.pump(80)
        app.testPollOnce()
        AppE2E.pump(80)

        XCTAssertFalse(app.testSearchPanelOpen(), "搜索跳转后面板必须关掉")
        XCTAssertEqual(
            app.testActiveTabID(),
            tab1,
            "从 tab 2 跳 tab \(tab1)（含 0）后当前 tab 必须回去，实际 \(app.testActiveTabID())"
        )
        XCTAssertTrue(
            app.waitTerminalContains(token),
            "跳转后 SwiftTerm 必须含 \(token)"
        )
    }

    func testSearchJumpScrollsOffscreenHitIntoView() throws {
        let fx = OffscreenHistory(label: "sj-hist")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 1), "attach 后应有 pane")
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

        app.testOpenSearchPanel()
        AppE2E.pump(80)
        app.testSetSearchQuery(fx.token)
        AppE2E.pump(120)
        XCTAssertGreaterThan(app.testSearchHitCount(), 0)
        app.testActivateFirstSearchHit()
        AppE2E.pump(80)
        app.testPollOnce()
        app.testFlushFeeds()
        AppE2E.pump(80)

        XCTAssertFalse(app.testSearchPanelOpen(), "搜索跳转后面板必须关掉")
        let pane = try XCTUnwrap(app.testLayoutLeafIDs().first)
        XCTAssertTrue(
            app.testPaneTerminalText(pane).contains(fx.token)
                || app.testAllVisibleTerminalText().contains(fx.token),
            "跳转到离屏命中后 SwiftTerm 必须看见 \(fx.token)"
        )
        XCTAssertGreaterThan(app.testPaneViewport(), 0, "离屏命中跳转后 viewport 应离开底部")
    }
}
