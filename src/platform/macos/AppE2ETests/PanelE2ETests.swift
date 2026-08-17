import AppKit
import XCTest
@testable import MuxtermAppLib

/// 对标 `linux_panel_e2e`：搜索 / 注意力 / QuickConnect 面板都有稳定 identifier。
final class PanelE2ETests: XCTestCase {
    func testSearchAttentionAndQuickConnectPanelsExist() throws {
        let painted = PaintedWorkspace(label: "panel")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2))

        app.testOpenSearchPanel()
        AppE2E.pump(80)
        XCTAssertTrue(app.testSearchPanelOpen())
        XCTAssertNotNil(find(app.searchPanel.window?.contentView, "muxterm.search.input"))
        XCTAssertNotNil(find(app.searchPanel.window?.contentView, "muxterm.search.list"))
        app.searchPanel.dismiss()

        app.testOpenAttentionPanel()
        AppE2E.pump(80)
        XCTAssertTrue(app.testAttentionPanelOpen())
        XCTAssertNotNil(find(app.attentionPanel.window?.contentView, "muxterm.attention.input"))
        XCTAssertNotNil(find(app.attentionPanel.window?.contentView, "muxterm.attention.list"))
        app.attentionPanel.dismiss()

        app.openQuickConnect()
        AppE2E.pump(80)
        // QuickConnect 面板 identifier 必须存在（对标 linux muxterm-panel-*）。
        XCTAssertTrue(
            app.windowsContain("muxterm.quickConnect.input")
                || app.windowsContain("muxterm.quickConnect.list"),
            "QuickConnect 必须暴露 muxterm.quickConnect.input / list"
        )
    }
}

private extension PanelE2ETests {
    func find(_ root: NSView?, _ id: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let found = find(child, id) { return found }
        }
        return nil
    }
}

private extension MainWindowController {
    func windowsContain(_ id: String) -> Bool {
        NSApp.windows.contains { win in
            contains(win.contentView, id)
        }
    }

    func contains(_ root: NSView?, _ id: String) -> Bool {
        guard let root else { return false }
        if root.accessibilityIdentifier() == id { return true }
        return root.subviews.contains { contains($0, id) }
    }
}
