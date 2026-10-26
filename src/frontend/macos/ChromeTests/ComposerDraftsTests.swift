import XCTest
@testable import MuxtermChrome

final class ComposerDraftsTests: XCTestCase {
    private func target(pane: UInt32, workspace: String = "api") -> ComposerTarget {
        ComposerTarget(
            kind: .local,
            host: "",
            workspaceName: workspace,
            workspaceID: "local:\(workspace)",
            paneID: pane
        )
    }

    func testEachPaneKeepsItsOwnDraftWhenSwitching() {
        var store = ComposerDraftStore()
        store.activate(target(pane: 1))
        store.update(text: "first", selection: NSRange(location: 5, length: 0), for: store.activeKey!)

        store.activate(target(pane: 2))
        XCTAssertEqual(store.activePage?.text, "", "新 pane 从空草稿开始")
        store.update(text: "second", selection: NSRange(location: 0, length: 0), for: store.activeKey!)

        store.activate(target(pane: 1))
        XCTAssertEqual(store.activePage?.text, "first")
        XCTAssertEqual(store.activePage?.selection.location, 5, "切回来光标也要还原")
        XCTAssertEqual(store.pages.map(\.text), ["first", "second"])
    }

    func testReactivatingAPaneOnlyRefreshesItsTargetSnapshot() {
        var store = ComposerDraftStore()
        store.activate(target(pane: 1))
        store.update(text: "keep", selection: NSRange(location: 0, length: 0), for: store.activeKey!)

        let refreshed = ComposerTarget(
            kind: .ssh,
            host: "ryzen",
            workspaceName: "api",
            workspaceID: "local:api",
            paneID: 1
        )
        store.activate(refreshed)
        XCTAssertEqual(store.pages.count, 1)
        XCTAssertEqual(store.activePage?.text, "keep")
        XCTAssertEqual(store.activePage?.target, refreshed)
        XCTAssertEqual(store.activePage?.title, "api #1")
    }

    func testClearingOnePageLeavesOtherPaneDraftsAlone() {
        var store = ComposerDraftStore()
        let first = store.activate(target(pane: 1)).key
        store.update(text: "one", selection: NSRange(location: 0, length: 0), for: first)
        let second = store.activate(target(pane: 2)).key
        store.update(text: "two", selection: NSRange(location: 0, length: 0), for: second)

        store.clear(first)
        XCTAssertEqual(store.page(for: first)?.text, "")
        XCTAssertEqual(store.page(for: second)?.text, "two")
        XCTAssertEqual(store.activeKey, second, "清空一页不能把焦点挪走")
    }

    func testTabTitlesDistinguishPanesInTheSameWorkspace() {
        var store = ComposerDraftStore()
        store.activate(target(pane: 1))
        store.activate(target(pane: 2, workspace: "another-very-long-workspace-name"))
        XCTAssertEqual(store.pages[0].title, "api #1")
        XCTAssertEqual(store.pages[1].title, "another-very #2")
    }

    func testUnboundPageStaysUsableWithoutAWorkspace() {
        var store = ComposerDraftStore()
        let page = store.activate(nil)
        XCTAssertFalse(page.key.isBound)
        XCTAssertEqual(page.title, "—")
        store.update(text: "scratch", selection: NSRange(location: 0, length: 0), for: page.key)
        XCTAssertEqual(store.activePage?.text, "scratch")
    }

    /// tab 栏只显示「当前页 + 有内容的页」；空白页面不占位置。
    func testBlankPagesStayOutOfTheTabStripButKeepTheirContent() {
        var store = ComposerDraftStore()
        let first = store.activate(target(pane: 1)).key
        XCTAssertEqual(store.visiblePages.map(\.key), [first])

        let second = store.activate(target(pane: 2)).key
        // 两个都还是空的：只剩当前页 visible。
        XCTAssertEqual(store.visiblePages.map(\.key), [second])

        store.update(text: "typed", selection: NSRange(location: 0, length: 0), for: second)
        let third = store.activate(target(pane: 3)).key
        XCTAssertEqual(store.visiblePages.map(\.key), [second, third])
        XCTAssertEqual(store.pages.count, 3, "隐藏的页面仍然保留内容")

        store.select(second)
        XCTAssertEqual(store.activeVisibleIndex, 0)
        store.update(text: "   ", selection: NSRange(location: 0, length: 0), for: second)
        XCTAssertEqual(store.visiblePages.map(\.key), [second], "只剩空格等于空草稿")
        XCTAssertEqual(store.page(for: third)?.text, "")
    }

    func testWindowSizeIsPortraitAndSlightlyLessThanHalfTheScreen() {
        for (width, height) in [(1440.0, 900.0), (1920.0, 1080.0), (2560.0, 1440.0)] {
            let size = ComposerWindowLayout.size(screenWidth: width, screenHeight: height)
            XCTAssertGreaterThan(size.height, size.width, "输入框必须是竖长窗口")
            XCTAssertLessThanOrEqual(size.width, width / 2)
            XCTAssertLessThan(size.height, height)
            XCTAssertGreaterThanOrEqual(size.width, ComposerWindowLayout.minimumWidth)
        }
    }
}
