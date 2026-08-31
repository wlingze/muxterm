import AppKit
import XCTest
@testable import MuxtermAppLib

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
        XCTAssertNotNil(findView(
            app.window?.titlebarAccessoryViewControllers.first?.view,
            id: "muxterm.sidebar.toggle"
        ))

        app.testSetSidebarOpen(false)
        XCTAssertFalse(app.testSidebarOpen())
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
