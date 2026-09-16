import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

final class AggregateWorkspaceE2ETests: XCTestCase {
    func testColdStartIsShellsAndCmdKReturnsToFirstLocalTab() throws {
        let project = OnePaneCat(label: "aggregate-shell-project")
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge, debug: true)
        defer { app.testShutdown() }
        app.window?.setFrame(
            AppE2E.fixedWindowFrame(width: 960, height: 640),
            display: true
        )
        app.window?.orderFront(nil)

        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.refreshWorkspaceSidebarForTest()
            return !app.testPresentedTabTitles().isEmpty
        })
        XCTAssertEqual(app.testSidebarWorkspaceNames(), ["Shells", "Agents"])
        XCTAssertTrue(app.testPresentedTabTitles()[0].hasPrefix("local ·"))

        app.testNewTab()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testPresentedTabIDs().count == 2
        })
        app.testSwitchTab(2)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testPresentedActiveTabID() == 2
        })

        let projectBridge = try CoreBridge(
            backendType: "tmux",
            socket: project.socket,
            session: project.session
        )
        app.testActivateWorkspaceBridge(projectBridge, session: project.session)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testActiveWorkspaceSession() == project.session
        })

        app.testSelectSidebarWorkspace(AggregateWorkspaceIdentity.agents)
        XCTAssertEqual(app.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.agents)
        XCTAssertEqual(app.testPresentedTabIDs(), [])
        XCTAssertEqual(app.testLayoutLeafIDs(), [])

        let cmdK = try XCTUnwrap(
            app.testMakeKeyEvent(key: "k", keyCode: 40, command: true),
            "必须能构造 Cmd-K"
        )
        XCTAssertTrue(app.testDispatchKeyEvent(cmdK), "Cmd-K 必须由窗口消费")
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testPresentedActiveTabID() == 1
        })
    }

    func testAgentsBorrowsSourceSurfaceAndCloseOnlyLeavesProjection() throws {
        let fixture = TwoPaneCat(label: "aggregate-agents")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let agentPane = UInt32(
            fixture.panes[1].trimmingCharacters(in: CharacterSet(charactersIn: "%"))
        ) ?? 0
        let sourceView = app.testTerminalViewIdentity(agentPane)
        app.testInjectAgent(paneId: agentPane, name: "Codex", title: "Review aggregate")
        app.testOpenAgents()

        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testLayoutLeafIDs() == [agentPane]
        })
        XCTAssertEqual(app.testTerminalViewIdentity(agentPane), sourceView)
        XCTAssertEqual(app.testPresentedTabTitles(), [
            "\(fixture.session) · local · Codex · Review aggregate",
        ])
        XCTAssertEqual(app.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.agents)

        let aggregateTab = try XCTUnwrap(app.testPresentedTabIDs().first)
        app.testCloseTab(aggregateTab)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return Set(app.testLayoutLeafIDs()) == Set([
                UInt32(fixture.panes[0].dropFirst()) ?? 0,
                agentPane,
            ])
        })
        XCTAssertEqual(
            app.testTerminalViewIdentity(agentPane),
            sourceView,
            "关闭 Agents 投影页不能销毁源 pane Surface"
        )
        XCTAssertFalse(app.testWindowClosing(), "关闭 Agents 投影页不能关闭源 Workspace")
    }
}
