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
}
