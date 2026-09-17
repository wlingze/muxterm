import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 慢速 SSH attach 立即成为可切换的 Workspace 页面，进度只占 PaneGrid。
final class ConnectProgressE2ETests: XCTestCase {
    func testUnreachableSshShowsSwitchableWorkspaceProgress() throws {
        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "conn-prog")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())
        app.testSetSidebarOpen(true)
        app.refreshWorkspaceSidebarForTest()
        let sourceID = try XCTUnwrap(
            app.testSidebarAllWorkspaceIDs().first(where: {
                $0 != AggregateWorkspaceIdentity.shells
                    && $0 != AggregateWorkspaceIdentity.agents
            })
        )

        let target = TargetConfig(
            name: "w19-blackhole",
            runtime: .tmux,
            transport: .ssh(name: "192.0.2.1"),
            path: "~"
        )
        app.testConnectTarget(target)
        AppE2E.pump(30)

        let visible = AppE2E.wait(timeout: 4) {
            app.testPollOnce()
            return app.testConnectProgressVisible()
        }
        XCTAssertTrue(
            visible,
            "PaneGrid 必须出现 \(ConnectProgress.identifier)，不能只用小 alert"
        )
        app.window?.layoutIfNeeded()
        XCTAssertEqual(app.testConnectProgressFrame(), app.testPaneLayoutFrame())
        XCTAssertFalse(
            app.testConnectProgressFrame().intersects(app.testStatusBarFrame()),
            "Opening 页面不能遮住 Workspace tab/status bar"
        )
        let value = app.testConnectProgressValue().lowercased()
        let stages = ConnectProgressStage.allCases.map(\.rawValue)
        XCTAssertTrue(
            stages.contains(where: { value.contains($0) }),
            "进度 AX value 必须含 resolving/ssh/list-sessions/attach/capture 之一。got=\(value)"
        )
        app.refreshWorkspaceSidebarForTest()
        let pendingID = try XCTUnwrap(
            app.testSidebarAllWorkspaceIDs().first(where: { $0.hasPrefix("__muxterm_opening__.") })
        )
        XCTAssertEqual(app.testSelectedSidebarWorkspaceID(), pendingID)

        app.testSelectSidebarWorkspace(sourceID)
        XCTAssertFalse(app.testConnectProgressVisible(), "等待期间必须能切回原 Workspace")
        app.testSelectSidebarWorkspace(pendingID)
        XCTAssertTrue(app.testConnectProgressVisible(), "正在打开的 Workspace 必须能重新进入")
        XCTAssertTrue(app.testWindowVisible(), "进度过程中主窗口必须还在")
    }
}
