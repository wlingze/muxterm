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

    func testSearchScopeDefaultsToAllAndSurvivesTabChange() {
        var model = PanelModel.open(.search)
        XCTAssertEqual(model.scope, .all)
        model.scope = .pane
        model.cycleTab(back: false)
        model.cycleTab(back: true)
        XCTAssertEqual(model.scope, .pane)
    }

    func testSearchScopeFiltersCurrentPaneAndWorkspace() {
        let hits = [
            SearchHit(workspaceId: "one", tabId: 10, paneId: 1, seq: 1, line: "one"),
            SearchHit(workspaceId: "one", tabId: 10, paneId: 2, seq: 2, line: "two"),
            SearchHit(workspaceId: "two", tabId: 20, paneId: 2, seq: 3, line: "collision"),
            SearchHit(workspaceId: "two", tabId: 20, paneId: 3, seq: 4, line: "three"),
        ]
        XCTAssertEqual(
            SearchScope.pane.filter(
                hits,
                activePane: 2,
                workspaceId: "one",
                workspacePaneIDs: [1, 2]
            ).map(\.paneId),
            [2]
        )
        XCTAssertEqual(
            SearchScope.workspace.filter(
                hits,
                activePane: 2,
                workspaceId: "one",
                workspacePaneIDs: [1, 2]
            ).map(\.paneId),
            [1, 2]
        )
        XCTAssertEqual(
            SearchScope.all.filter(
                hits,
                activePane: 2,
                workspaceId: "one",
                workspacePaneIDs: [1, 2]
            ).map(\.paneId),
            [1, 2, 2, 3]
        )
    }
}
