import AppKit
import XCTest
@testable import MuxtermAppLib

/// Rename 必须从 AppKit 生产动作穿过 C ABI 到真实隔离 tmux。
final class RenameE2ETests: XCTestCase {
    func testRenameTabAndWorkspaceReachIsolatedTmuxAndQuickConnect() throws {
        let painted = PaintedWorkspace(label: "rename")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))

        let tabId = app.testActiveTabID()
        let tabName = "native-tab-\(ProcessInfo.processInfo.processIdentifier)"
        XCTAssertTrue(app.renameTab(tabId, to: tabName))
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.lastSnapshot.tabs.contains { $0.id == tabId && $0.name == tabName }
            },
            "Core 的 TabRenamed 必须更新 AppKit tab"
        )
        XCTAssertEqual(
            Tmux.out(socket: painted.socket, args: [
                "display-message", "-p", "-t", "@\(tabId)", "#{window_name}",
            ]),
            tabName
        )

        let workspaceName = "native-workspace-\(ProcessInfo.processInfo.processIdentifier)"
        XCTAssertTrue(app.renameWorkspace(to: workspaceName))
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return Tmux.out(
                    socket: painted.socket,
                    args: ["display-message", "-p", "#{session_name}"]
                ) == workspaceName
            },
            "Core 的 RenameWorkspace 必须重命名隔离 tmux session"
        )

        app.openQuickConnect()
        AppE2E.pump(80)
        let currentNames = (0..<app.unifiedPanel.testWorkspaceRowCount()).compactMap { row -> String? in
            guard let cell = app.unifiedPanel.testWorkspaceCell(at: row),
                  cell.testIsCurrent()
            else { return nil }
            return cell.testTitleText()
        }
        XCTAssertEqual(currentNames, [workspaceName], "Quick Connect Current/Recent 必须跟随重命名")
    }
}
