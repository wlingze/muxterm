import XCTest
@testable import MuxtermChrome

/// 对标 Linux `panel_model::tests`：Cmd-P 三 tab 循环 + query 跨 tab 保留。
final class PanelModelTests: XCTestCase {
    func testTabCycleWraps() {
        var model = PanelModel.open(.workspaces)
        model.cycleTab(back: false)
        XCTAssertEqual(model.tab, .attention)
        model.cycleTab(back: false)
        XCTAssertEqual(model.tab, .search)
        model.cycleTab(back: false)
        XCTAssertEqual(model.tab, .workspaces)
        model.cycleTab(back: true)
        XCTAssertEqual(model.tab, .search)
    }

    func testQuerySurvivesTabChange() {
        var model = PanelModel.open(.workspaces)
        model.query = "legion"
        model.cycleTab(back: false)
        XCTAssertEqual(model.tab, .attention)
        XCTAssertEqual(model.query, "legion")
    }
}
