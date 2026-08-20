import AppKit
import XCTest
@testable import MuxtermAppLib

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
