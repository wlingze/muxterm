import AppKit
import XCTest
@testable import MuxtermAppLib
@testable import MuxtermChrome

final class WorkspaceSidebarE2ETests: XCTestCase {
    func testMainWindowSidebarHasPersistentWorkspaceAndAgentSections() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge)
        defer { app.testShutdown() }

        app.showWindow(nil)
        app.testSetSidebarOpen(true)
        app.testPollOnce()
        app.window?.layoutIfNeeded()

        XCTAssertTrue(app.testSidebarOpen())
        XCTAssertFalse(app.testSidebarWorkspaceNames().isEmpty)
        XCTAssertNotNil(findView(app.window?.contentView, id: "muxterm.sidebar.workspaces.section"))
        XCTAssertNotNil(findView(app.window?.contentView, id: "muxterm.sidebar.agents.section"))
        XCTAssertNotNil(findView(app.window?.contentView, id: "muxterm.sidebar.commands.section"))
        XCTAssertNotNil(findView(app.window?.contentView, id: "muxterm.sidebar.hiddenCommands.section"))
        XCTAssertNotNil(findView(
            app.window?.titlebarAccessoryViewControllers.first?.view,
            id: "muxterm.sidebar.toggle"
        ))

        app.testSetSidebarOpen(false)
        XCTAssertFalse(app.testSidebarOpen())
    }

    func testCmdCtrlNumberSwitchesFixedWorkspaceOrder() throws {
        let first = OnePaneCat(label: "ws-shortcut-first")
        let second = OnePaneCat(label: "ws-shortcut-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))

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

        let event = try XCTUnwrap(
            app.testMakeKeyEvent(key: "1", keyCode: 18, command: true, control: true),
            "必须能构造 Cmd-Ctrl-1"
        )
        XCTAssertTrue(app.testDispatchKeyEvent(event), "Cmd-Ctrl-1 必须被窗口快捷键消费")
        XCTAssertEqual(
            app.testActiveWorkspaceSession(),
            first.session,
            "Cmd-Ctrl-1 应切回固定顺序的第一个 Workspace"
        )

        let secondEvent = try XCTUnwrap(
            app.testMakeKeyEvent(key: "2", keyCode: 19, command: true, control: true),
            "必须能构造 Cmd-Ctrl-2"
        )
        XCTAssertTrue(app.testDispatchKeyEvent(secondEvent), "Cmd-Ctrl-2 必须被窗口快捷键消费")
        XCTAssertEqual(app.testActiveWorkspaceSession(), second.session)
    }

    func testHiddenCommandEyeTogglesVisibility() {
        let sidebar = WorkspaceSidebarView(frame: NSRect(x: 0, y: 0, width: 240, height: 640))
        let item = CommandSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            paneId: 8,
            title: "cargo test",
            detail: "dev · pane 8",
            indicator: .running
        )
        sidebar.setCommands([item])

        XCTAssertEqual(sidebar.testCommandTitles(), ["cargo test"])
        XCTAssertTrue(sidebar.testHiddenCommandTitles().isEmpty)
        sidebar.testToggleCommandVisibility(
            workspaceId: item.workspaceId,
            paneId: item.paneId
        )
        XCTAssertTrue(sidebar.testCommandTitles().isEmpty)
        XCTAssertEqual(sidebar.testHiddenCommandTitles(), ["cargo test"])

        let replacement = CommandSidebarItem(
            workspaceId: item.workspaceId,
            paneId: item.paneId,
            title: "npm test",
            detail: item.detail,
            indicator: .running
        )
        sidebar.setCommands([replacement])
        XCTAssertEqual(sidebar.testCommandTitles(), ["npm test"])
        XCTAssertTrue(sidebar.testHiddenCommandTitles().isEmpty)

    }

    func testCollapsedSectionsPackAgainstNearestBoundary() {
        let sidebar = WorkspaceSidebarView(frame: NSRect(x: 0, y: 0, width: 240, height: 640))
        sidebar.testSetSectionExpanded(.hiddenCommands, false)

        // All-collapsed packs to the top; expanded sections absorb all slack.
        sidebar.testSetSectionExpanded(.workspaces, false)
        sidebar.testSetSectionExpanded(.agents, false)
        sidebar.testSetSectionExpanded(.commands, false)
        let allCollapsed = sidebar.testSectionFrames()
        XCTAssertEqual(allCollapsed[.workspaces]?.maxY ?? 0, 104, accuracy: 0.5)
        XCTAssertEqual(allCollapsed[.agents]?.minY ?? 0, 52, accuracy: 0.5)
        XCTAssertEqual(allCollapsed[.hiddenCommands]?.maxY ?? 0, 26, accuracy: 0.5)

        sidebar.testSetSectionExpanded(.agents, true)
        let expandedMiddle = sidebar.testSectionFrames()
        XCTAssertEqual(expandedMiddle[.workspaces]?.maxY ?? 0, 640, accuracy: 0.5)
        XCTAssertEqual(expandedMiddle[.commands]?.maxY ?? 0, 52, accuracy: 0.5)
        XCTAssertEqual(expandedMiddle[.hiddenCommands]?.maxY ?? 0, 26, accuracy: 0.5)
    }

    func testWorkspaceCloseButtonRemovesWorkspaceAndFallsForward() throws {
        let first = OnePaneCat(label: "ws-close-first")
        let second = OnePaneCat(label: "ws-close-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))

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

        AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.refreshWorkspaceSidebarForTest()
            return app.testWorkspaceIDs().count == 2
        }
        let workspaceIDs = app.testWorkspaceIDs()
        XCTAssertEqual(workspaceIDs.count, 2, "关闭前应有两条 Workspace")
        app.testCloseWorkspace(workspaceId: workspaceIDs[1])
        XCTAssertEqual(
            app.testActiveWorkspaceSession(),
            first.session,
            "关闭当前 Workspace 后应切到下一个 warm Workspace"
        )
        XCTAssertEqual(app.testWorkspaceCount(), 1)
        XCTAssertTrue(
            app.testWorkspaceNames().contains(first.session),
            "关闭当前 Workspace 后剩余行应是第一个 Workspace"
        )
        XCTAssertFalse(
            app.testWorkspaceNames().contains(second.session),
            "被关闭的 Workspace 不得残留在侧边栏"
        )

        let remainingIDs = app.testWorkspaceIDs()
        XCTAssertEqual(remainingIDs.count, 1)
        app.testCloseWorkspace(workspaceId: remainingIDs[0])
        XCTAssertTrue(app.testWindowClosing(), "关闭最后一个 Workspace 应关闭窗口")
    }

    func testSidebarWorkspaceSwitchIsFastWithPendingBackgroundEvents() throws {
        let first = OnePaneCat(label: "switch-fast-first")
        let second = OnePaneCat(label: "switch-fast-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))

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

        let token = "SWITCH_FAST_\(UUID().uuidString)"
        Tmux.sendLiteral(socket: first.socket, target: first.pane, text: token)
        Tmux.ok(socket: first.socket, args: ["send-keys", "-t", first.pane, "Enter"])
        Tmux.waitCapture(socket: first.socket, target: first.pane, needle: token)

        let start = DispatchTime.now()
        app.testSwitchBackToFirstWorkspace()
        let elapsed = Double(
            DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds
        ) / 1_000_000_000
        XCTAssertLessThan(
            elapsed,
            0.12,
            "侧边栏切回 warm Workspace 不应有可感知卡顿，实际 \(elapsed)s"
        )
        XCTAssertEqual(app.testActiveWorkspaceSession(), first.session)
    }

    func testCmdBTogglesSidebarThroughProductionKeyRouter() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge)
        defer { app.testShutdown() }
        app.showWindow(nil)
        app.testSetSidebarOpen(false)
        XCTAssertFalse(app.testSidebarOpen())

        let event = try XCTUnwrap(
            app.testMakeKeyEvent(key: "b", keyCode: 11, command: true),
            "必须能构造 Cmd-B"
        )
        XCTAssertTrue(app.testDispatchKeyEvent(event), "Cmd-B 必须被窗口快捷键消费")
        XCTAssertTrue(app.testSidebarOpen(), "Cmd-B 应打开侧边栏")

        XCTAssertTrue(app.testDispatchKeyEvent(event), "第二次 Cmd-B 也必须被消费")
        XCTAssertFalse(app.testSidebarOpen(), "再按 Cmd-B 应收起侧边栏")
    }

    private func findView(_ root: NSView?, id: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let match = findView(child, id: id) {
                return match
            }
        }
        return nil
    }
}
