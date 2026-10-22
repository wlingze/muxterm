import XCTest
@testable import MuxtermChrome

final class QuickPanelActionsTests: XCTestCase {
    private let labels = QuickPanelActionLabels(
        open: "Open",
        close: "Close workspace",
        update: "Update runtime",
        connect: "Connect",
        edit: "Edit",
        create: "New Project",
        attach: "Attach",
        save: "Save",
        delete: "Delete",
        jump: "Jump",
        hide: "Hide",
        show: "Show",
        openAttention: "Open"
    )

    private func global(_ id: String, title: String, keywords: String) -> QuickPanelCommand {
        QuickPanelCommand(
            id: id,
            title: title,
            detail: "",
            keywords: keywords,
            section: .global
        )
    }

    func testWorkspaceActionsStayAheadOfGlobalCommands() {
        let contextual = QuickPanelActions.contextual(
            for: .workspace(name: "ryzen", closable: true, canUpdate: true),
            labels: labels
        )
        let commands = QuickPanelActions.merge(
            contextual: contextual,
            global: [
                global("palette.newTab", title: "New Tab", keywords: "new tab"),
                global("palette.closePane", title: "Close Pane", keywords: "close pane"),
            ]
        )
        XCTAssertEqual(
            commands.map(\.id),
            [
                QuickPanelActions.open,
                QuickPanelActions.close,
                QuickPanelActions.update,
                "palette.newTab",
                "palette.closePane",
            ]
        )
        XCTAssertEqual(commands.map(\.section), [
            .contextual, .contextual, .contextual, .global, .global,
        ])
    }

    func testFixedWorkspaceOmitsCloseAndUpdate() {
        let commands = QuickPanelActions.contextual(
            for: .workspace(name: "Shells", closable: false, canUpdate: false),
            labels: labels
        )
        XCTAssertEqual(commands.map(\.id), [QuickPanelActions.open])
    }

    func testTargetOffersConnectThenEdit() {
        let commands = QuickPanelActions.contextual(
            for: .target(name: "legion"),
            labels: labels
        )
        XCTAssertEqual(
            commands.map(\.id),
            [QuickPanelActions.connect, QuickPanelActions.edit, QuickPanelActions.delete]
        )
        XCTAssertEqual(commands.map(\.detail), ["legion", "legion", "legion"])
    }

    func testHiddenAttentionOffersShowInsteadOfHide() {
        let visible = QuickPanelActions.contextual(
            for: .attention(title: "codex", hidden: false),
            labels: labels
        )
        let hidden = QuickPanelActions.contextual(
            for: .attention(title: "codex", hidden: true),
            labels: labels
        )
        XCTAssertEqual(
            visible.map(\.id),
            [QuickPanelActions.jump, QuickPanelActions.openAttention, QuickPanelActions.hide]
        )
        XCTAssertEqual(hidden.last?.id, QuickPanelActions.show)
        XCTAssertEqual(hidden.last?.title, "Show")
    }

    func testFilterKeepsContextualMatchesBeforeGlobalOnes() {
        let commands = QuickPanelActions.merge(
            contextual: QuickPanelActions.contextual(
                for: .workspace(name: "ryzen", closable: true, canUpdate: false),
                labels: labels
            ),
            global: [
                global("palette.closePane", title: "Close Pane", keywords: "close pane"),
                global("palette.split", title: "Split Pane", keywords: "split horizontal"),
            ]
        )
        let close = QuickPanelActions.filter(commands, query: "Close")
        XCTAssertEqual(close.map(\.id), [QuickPanelActions.close, "palette.closePane"])
        XCTAssertEqual(
            QuickPanelActions.filter(commands, query: "close pane").map(\.id),
            ["palette.closePane"]
        )
        XCTAssertEqual(QuickPanelActions.filter(commands, query: "   "), commands)
    }

    func testEscapeRestoresThePanelQueryNotTheActionFilter() {
        let root = QuickPanelActions.contextual(
            for: .search(line: "auth failed"),
            labels: labels
        )
        var session = QuickPanelActionSession(savedQuery: "auth", commands: root)
        session.setQuery("jump")
        XCTAssertEqual(session.visibleCommands.map(\.id), [QuickPanelActions.jump])
        XCTAssertEqual(session.escape(), .leave(savedQuery: "auth"))
    }

    func testNestedEscapeReturnsToTheActionFilter() {
        let root = QuickPanelActions.merge(
            contextual: [],
            global: [
                global("palette.language", title: "Language", keywords: "language 语言"),
                global("palette.newTab", title: "New Tab", keywords: "new tab"),
            ]
        )
        var session = QuickPanelActionSession(savedQuery: "ryzen", commands: root)
        session.setQuery("lang")
        session.push([
            global("palette.language.en", title: "English", keywords: "language en"),
            global("palette.language.zh-CN", title: "简体中文", keywords: "language zh"),
        ])
        XCTAssertEqual(session.query, "")
        XCTAssertTrue(session.isNested)
        XCTAssertEqual(session.escape(), .stay)
        XCTAssertFalse(session.isNested)
        XCTAssertEqual(session.query, "lang")
        XCTAssertEqual(session.visibleCommands.map(\.id), ["palette.language"])
        XCTAssertEqual(session.escape(), .leave(savedQuery: "ryzen"))
    }

    func testActionPopoverSitsAtTheBottomRight() {
        let bounds = CGRect(x: 0, y: 0, width: 640, height: 420)
        let frame = QuickActionPopoverLayout.frame(in: bounds, rowCount: 4)
        XCTAssertEqual(frame.width, 300, accuracy: 0.1)
        XCTAssertEqual(frame.maxX, bounds.maxX - QuickActionPopoverLayout.margin, accuracy: 0.1)
        XCTAssertEqual(frame.minY, bounds.minY + QuickActionPopoverLayout.margin, accuracy: 0.1)
        XCTAssertLessThan(frame.width, bounds.width / 2)
        XCTAssertLessThan(frame.height, bounds.height / 2)
    }

    /// yaklang 里 grok 白屏：字符宽先按 2x 对齐成 11.5，再按 scale 1 round 成 12。
    /// 1020pt 上就是 88 列和 85 列。两套列数来回 resize，整帧被丢掉，TUI 停在白底。
    func testPixelRoundingAtUnitScaleInventsThreeColumns() {
        let snapped = TerminalCellGrid.snapped(rawWidth: 11.2, rawHeight: 21, scale: 2)
        XCTAssertEqual(snapped.width, 11.5, accuracy: 0.01)
        let shrunk = TerminalCellGrid.pixelRoundedWidth(snappedWidth: snapped.width, scale: 1)
        XCTAssertEqual(shrunk, 12, accuracy: 0.01)
        let width: CGFloat = 1020
        let live = TerminalCellGrid.columns(width: width, cellWidth: snapped.width)
        let rounded = TerminalCellGrid.columns(width: width, cellWidth: shrunk)
        XCTAssertEqual(live, 88)
        XCTAssertEqual(rounded, 85)
    }

    func testFittingCountDoesNotPaintPastTheView() {
        let cell: CGFloat = 11.5
        let width: CGFloat = 1020
        let cols = TerminalCellGrid.fittingCount(length: width, cell: cell)
        XCTAssertEqual(cols, 88)
        XCTAssertLessThanOrEqual(CGFloat(cols) * cell, width + 0.01)
        XCTAssertGreaterThan(CGFloat(cols + 1) * cell, width)
    }
}
