import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

final class AggregateWorkspaceE2ETests: XCTestCase {
    func testColdStartIsShellsAndCmdCtrlSReturnsToFirstLocalTab() throws {
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
        XCTAssertEqual(app.testSidebarWorkspaceShortcutTexts(), ["S", "A"])
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

        let cmdCtrlS = try XCTUnwrap(
            app.testMakeKeyEvent(key: "s", keyCode: 1, command: true, control: true),
            "必须能构造 Cmd-Ctrl-S"
        )
        XCTAssertTrue(
            app.testDispatchKeyEvent(cmdCtrlS),
            "Cmd-Ctrl-S 必须由窗口消费"
        )
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testPresentedActiveTabID() == 1
        })
    }

    func testAgentsBorrowsWholeSourceTabAndCloseOnlyLeavesProjection() throws {
        let fixture = TwoPaneCat(label: "aggregate-agents")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let sourceActivePane = app.testActivePaneID()
        let paneIDs = fixture.panes.compactMap { UInt32($0.dropFirst()) }
        let agentPane = try XCTUnwrap(
            paneIDs.first(where: { $0 != sourceActivePane }),
            "夹具必须有一个非焦点 pane 作为 agent"
        )
        let sourceView = app.testTerminalViewIdentity(agentPane)
        let sourceLayout = Set(app.testLayoutLeafIDs())
        app.testInjectAgent(paneId: agentPane, name: "Codex", title: "Review aggregate")
        let cmdCtrlA = try XCTUnwrap(
            app.testMakeKeyEvent(key: "a", keyCode: 0, command: true, control: true),
            "必须能构造 Cmd-Ctrl-A"
        )
        XCTAssertTrue(
            app.testDispatchKeyEvent(cmdCtrlA),
            "Cmd-Ctrl-A 必须由窗口消费"
        )

        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return Set(app.testLayoutLeafIDs()) == sourceLayout
        })
        XCTAssertEqual(app.testTerminalViewIdentity(agentPane), sourceView)
        XCTAssertEqual(
            app.testActivePaneID(),
            sourceActivePane,
            "Agents 只聚合源 Tab，不能强制把焦点切到 agent pane"
        )
        XCTAssertEqual(app.testPresentedTabTitles(), [
            "\(fixture.session) · local · Codex · Review aggregate",
        ])
        XCTAssertEqual(app.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.agents)

        let sourceTabs = app.testTabIDs()
        app.testNewTab()
        AppE2E.pump(20)
        XCTAssertEqual(
            app.testTabIDs(),
            sourceTabs,
            "Agents aggregate must not create a real source Tab"
        )

        let leavesBeforeSplit = app.testLayoutLeafIDs().count
        app.testSplitHorizontal()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return app.testLayoutLeafIDs().count == leavesBeforeSplit + 1
        }, "Agents aggregate must allow splitting the real source Tab")
        app.testCloseActivePane()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return app.testLayoutLeafIDs().count == leavesBeforeSplit
        }, "Agents aggregate must allow closing a pane in the real source Tab")

        let aggregateTab = try XCTUnwrap(app.testPresentedTabIDs().first)
        app.testCloseTab(aggregateTab)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return Set(app.testLayoutLeafIDs()) == sourceLayout
        }, "关闭投影后应保留完整源布局，实际 leaves=\(app.testLayoutLeafIDs())")
        XCTAssertEqual(
            app.testTerminalViewIdentity(agentPane),
            sourceView,
            "关闭 Agents 投影页不能销毁源 pane Surface"
        )
        XCTAssertFalse(app.testWindowClosing(), "关闭 Agents 投影页不能关闭源 Workspace")
    }

    func testAgentsRestoresLastSelectedAggregateTab() throws {
        let first = OnePaneCat(label: "aggregate-agent-memory-first")
        let second = OnePaneCat(label: "aggregate-agent-memory-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        app.testInjectAgent(
            paneId: app.testActivePaneID(),
            name: "Codex",
            title: "First agent"
        )

        let secondBridge = try CoreBridge(
            backendType: "tmux",
            socket: second.socket,
            session: second.session
        )
        app.testActivateWorkspaceBridge(secondBridge, session: second.session)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testActiveWorkspaceSession() == second.session
        })
        app.testInjectAgent(
            paneId: app.testActivePaneID(),
            name: "Claude",
            title: "Second agent"
        )

        app.testOpenAgents()
        let aggregateTabs = app.testPresentedTabIDs()
        XCTAssertEqual(aggregateTabs.count, 2)
        app.testSwitchTab(aggregateTabs[1])
        XCTAssertEqual(app.testPresentedActiveTabID(), aggregateTabs[1])

        app.testSwitchBackToFirstWorkspace()
        XCTAssertNotEqual(app.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.agents)
        app.testOpenAgents()
        XCTAssertEqual(
            app.testPresentedActiveTabID(),
            aggregateTabs[1],
            "返回 Agents 时应保留上次选择的聚合 Tab"
        )
    }
}
