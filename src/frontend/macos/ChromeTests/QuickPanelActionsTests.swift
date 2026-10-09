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
        XCTAssertEqual(commands.map(\.id), [QuickPanelActions.connect, QuickPanelActions.edit])
        XCTAssertEqual(commands.map(\.detail), ["legion", "legion"])
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
}
