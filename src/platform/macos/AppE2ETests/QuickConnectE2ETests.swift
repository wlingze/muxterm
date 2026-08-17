import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 对标 `linux_quickconnect_e2e`：status snapshot + zoom 任务。
final class QuickConnectE2ETests: XCTestCase {
    func testStatusSnapshotAndFullscreenZoomOnIsolatedTmux() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("qc-status")
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", "stat",
            "-x", "80", "-y", "24",
        ])
        let bridge = try CoreBridge(backendType: "tmux", socket: socket, session: "stat")
        defer { bridge.shutdown() }

        XCTAssertTrue(
            AppE2E.wait(timeout: 3) {
                _ = bridge.pollEvents()
                guard let json = bridge.statusBarSnapshotJSON(),
                      let data = json.data(using: .utf8),
                      let response = try? JSONDecoder().decode(StatusBarResponse.self, from: data),
                      let snap = response.status
                else {
                    return false
                }
                return snap.enabled && !snap.windows.isEmpty
            },
            "status snapshot 应含窗口列表"
        )
        let snapshot: StatusBarSnapshot = try {
            let json = try XCTUnwrap(bridge.statusBarSnapshotJSON())
            let response = try JSONDecoder().decode(StatusBarResponse.self, from: Data(json.utf8))
            return try XCTUnwrap(response.status)
        }()
        XCTAssertTrue(snapshot.windows.contains(where: \.current), "应有当前窗口标记")

        let tabs = bridge.getTabs()
        let active = tabs.first(where: \.isActive)?.id ?? 0
        let panes = bridge.getPanes(tabId: active)
        let paneId = panes.first(where: \.isActive)?.id ?? panes.first?.id ?? 0
        XCTAssertEqual(bridge.execute(task: MuxTask.splitPane(targetPane: paneId, horizontal: true)), 0)
        _ = AppE2E.wait(timeout: 2) {
            _ = bridge.pollEvents()
            return bridge.getPanes(tabId: active).count >= 2
                || tabs.contains { bridge.getPanes(tabId: $0.id).count >= 2 }
        }
        XCTAssertEqual(bridge.execute(task: MuxTask.togglePaneFullscreen(paneId)), 0, "resize-pane -Z 应成功")
        XCTAssertTrue(
            AppE2E.wait(timeout: 2) {
                Tmux.out(socket: socket, args: ["display-message", "-p", "-t", "stat", "#{window_zoomed_flag}"]) == "1"
            },
            "tmux window_zoomed_flag 应为 1"
        )
        _ = bridge.execute(task: MuxTask.togglePaneFullscreen(paneId))
    }
}
