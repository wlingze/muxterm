import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Linux Cmd-P 是**一个**三 tab 面板（Workspaces / Attention / Search），
/// Tab / Shift+Tab 循环。macOS 现在仍是三个独立 NSPanel，本测试必须红。
final class UnifiedPanelE2ETests: XCTestCase {
    func testCmdPPanelHasThreeTabsAndTabCycles() throws {
        let painted = PaintedWorkspace(label: "panel-parity")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2))

        app.openQuickConnect()
        AppE2E.pump(80)

        let workspaces = findOpen("muxterm.panel.tab.workspaces")
            ?? findOpen("muxterm-panel-tab-workspaces")
        let attention = findOpen("muxterm.panel.tab.attention")
            ?? findOpen("muxterm-panel-tab-attention")
        let search = findOpen("muxterm.panel.tab.search")
            ?? findOpen("muxterm-panel-tab-search")
        XCTAssertNotNil(workspaces, "Cmd-P 必须露出 Workspaces tab（muxterm.panel.tab.workspaces）")
        XCTAssertNotNil(attention, "Cmd-P 必须露出 Attention tab（muxterm.panel.tab.attention）")
        XCTAssertNotNil(search, "Cmd-P 必须露出 Search tab（muxterm.panel.tab.search）")

        let workspaceWindow = workspaces?.window
        XCTAssertEqual(workspaceWindow, attention?.window, "三个 tab 必须在同一个面板窗口")
        XCTAssertEqual(workspaceWindow, search?.window, "三个 tab 必须在同一个面板窗口")

        guard let tabEvent = app.testMakeTabEvent(shift: false) else {
            XCTFail("无法构造 Tab 键事件")
            return
        }
        workspaceWindow?.makeKeyAndOrderFront(nil)
        AppE2E.pump(20)
        _ = app.testDispatchKeyEvent(tabEvent)
        AppE2E.pump(80)

        XCTAssertTrue(
            isOn(attention),
            "Cmd-P 后按 Tab 必须切到 Attention。windows=\(NSApp.windows.map { $0.title })"
        )

        guard let shiftTab = app.testMakeTabEvent(shift: true) else {
            XCTFail("无法构造 Shift+Tab")
            return
        }
        _ = app.testDispatchKeyEvent(shiftTab)
        AppE2E.pump(80)
        XCTAssertTrue(isOn(workspaces), "Shift+Tab 必须回到 Workspaces")
    }

    func testPanelShortcutsSwitchTabsWithoutClosingPanel() throws {
        let painted = PaintedWorkspace(label: "panel-shortcuts")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2))

        app.openQuickConnect()
        AppE2E.pump(40)
        let panel = app.unifiedPanel.window
        XCTAssertTrue(panel?.isVisible == true)

        app.openAttentionPanel() // Cmd-R
        AppE2E.pump(40)
        XCTAssertTrue(panel?.isVisible == true, "Cmd-R 不应关闭已打开的统一面板")
        XCTAssertEqual(app.unifiedPanel.modelTab, .attention)

        app.openWorkspaceSearchPanel() // Cmd-F
        AppE2E.pump(40)
        XCTAssertTrue(panel?.isVisible == true, "Cmd-F 不应关闭已打开的统一面板")
        XCTAssertEqual(app.unifiedPanel.modelTab, .search)

        app.openGlobalSearchPanel() // Cmd-Shift-F
        AppE2E.pump(40)
        XCTAssertEqual(app.unifiedPanel.modelTab, .search)
    }

    func testWorkspacePanelSelectsCurrentWorkspaceAndShowsSidebarIndex() throws {
        let first = OnePaneCat(label: "panel-index-first")
        let second = OnePaneCat(label: "panel-index-second")
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

        app.openQuickConnect()
        AppE2E.pump(80)

        XCTAssertEqual(
            Array(app.unifiedPanel.testWorkspaceTitles().prefix(4)),
            ["Shells", "Agents", first.session, second.session],
            "快速面板必须复用侧边栏的固定项和真实 Workspace 打开顺序"
        )
        XCTAssertEqual(app.unifiedPanel.testWorkspaceShortcutText(matching: "Shells"), "S")
        XCTAssertEqual(app.unifiedPanel.testWorkspaceShortcutText(matching: "Agents"), "A")
        XCTAssertEqual(
            app.unifiedPanel.testSelectedWorkspaceTitle(),
            second.session,
            "快速面板打开时必须默认选中当前 Workspace"
        )
        XCTAssertEqual(
            app.unifiedPanel.testWorkspaceIndex(matching: first.session),
            1,
            "固定聚合槽不能占用真实 Workspace 编号"
        )
        XCTAssertEqual(
            app.unifiedPanel.testWorkspaceIndex(matching: second.session),
            2,
            "当前 Workspace 即使按 Recent 排在前面，也必须保留侧栏编号"
        )
        XCTAssertFalse(app.unifiedPanel.testWorkspaceCloseVisible(matching: "Shells"))
        XCTAssertFalse(app.unifiedPanel.testWorkspaceCloseVisible(matching: "Agents"))
        XCTAssertTrue(app.unifiedPanel.testWorkspaceCloseVisible(matching: first.session))
        XCTAssertTrue(app.unifiedPanel.testWorkspaceCloseVisible(matching: second.session))

        app.unifiedPanel.testCloseWorkspaceItem(matching: first.session)
        AppE2E.pump(40)
        XCTAssertTrue(app.unifiedPanel.window?.isVisible == true)
        XCTAssertFalse(app.unifiedPanel.testWorkspaceTitles().contains(first.session))
        XCTAssertEqual(app.testActiveWorkspaceSession(), second.session)
    }

    func testAggregateRowsActivateTheSameSidebarDestinations() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge, debug: true)
        defer { app.testShutdown() }

        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.refreshWorkspaceSidebarForTest()
            return app.testActivePaneID() != 0
        })
        app.testInjectAgent(
            paneId: app.testActivePaneID(),
            name: "Codex",
            title: "Panel aggregate activation"
        )

        app.openQuickConnect()
        AppE2E.pump(40)
        app.unifiedPanel.testActivateWorkspaceItem(matching: "Agents")
        XCTAssertEqual(
            app.testSelectedSidebarWorkspaceID(),
            AggregateWorkspaceIdentity.agents
        )

        app.openQuickConnect()
        AppE2E.pump(40)
        app.unifiedPanel.testActivateWorkspaceItem(matching: "Shells")
        XCTAssertEqual(
            app.testSelectedSidebarWorkspaceID(),
            AggregateWorkspaceIdentity.shells
        )
    }

    func testWorkspaceShortcutsDismissPanelAndNavigate() throws {
        let fixture = OnePaneCat(label: "panel-workspace-shortcuts")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        app.testInjectAgent(
            paneId: app.testActivePaneID(),
            name: "Codex",
            title: "Panel shortcut"
        )

        app.openQuickConnect()
        let agents = try XCTUnwrap(app.testMakeUnifiedPanelKeyEvent(
            key: "a", keyCode: 0, command: true, control: true
        ))
        XCTAssertNil(app.testRouteMonitoredKeyEvent(agents))
        XCTAssertFalse(app.unifiedPanel.window?.isVisible == true)
        XCTAssertEqual(app.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.agents)

        app.openQuickConnect()
        let first = try XCTUnwrap(app.testMakeUnifiedPanelKeyEvent(
            key: "1", keyCode: 18, command: true, control: true
        ))
        XCTAssertNil(app.testRouteMonitoredKeyEvent(first))
        XCTAssertFalse(app.unifiedPanel.window?.isVisible == true)
        XCTAssertEqual(app.testActiveWorkspaceSession(), fixture.session)
        XCTAssertNotEqual(
            app.testSelectedSidebarWorkspaceID(),
            AggregateWorkspaceIdentity.agents
        )
    }

    func testShellShortcutDismissesPanel() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge, debug: true)
        defer { app.testShutdown() }
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testActivePaneID() != 0
        })

        app.openQuickConnect()
        let shells = try XCTUnwrap(app.testMakeUnifiedPanelKeyEvent(
            key: "s", keyCode: 1, command: true, control: true
        ))
        XCTAssertNil(app.testRouteMonitoredKeyEvent(shells))
        XCTAssertFalse(app.unifiedPanel.window?.isVisible == true)
        XCTAssertEqual(app.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.shells)
    }
}

private extension UnifiedPanelE2ETests {
    func findOpen(_ id: String) -> NSView? {
        for window in NSApp.windows where window.isVisible {
            if let found = find(window.contentView, id) {
                return found
            }
        }
        return nil
    }

    func find(_ root: NSView?, _ id: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let found = find(child, id) { return found }
        }
        return nil
    }

    func isOn(_ view: NSView?) -> Bool {
        guard let view else { return false }
        if let button = view as? NSButton {
            return button.state == .on
        }
        return view.window?.isKeyWindow == true
    }
}
