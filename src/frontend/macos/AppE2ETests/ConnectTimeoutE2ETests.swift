import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// W15c：SSH 连不上时 AppKit 主线程不得冻死。
final class ConnectTimeoutE2ETests: XCTestCase {
    func testUnreachableSshDoesNotBlockMainThread() throws {
        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "timeout")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        let target = TargetConfig(
            name: "w15-blackhole",
            runtime: .tmux,
            transport: .ssh(name: "192.0.2.1"),
            path: "~"
        )
        let t0 = Date()
        app.testConnectTarget(target)
        let elapsed = Date().timeIntervalSince(t0)
        XCTAssertLessThan(
            elapsed,
            0.5,
            "testConnectTarget 必须立刻把控制权还给 AppKit（后台等 SSH），实际 \(elapsed)s。禁止在主线程同步 connect"
        )

        AppE2E.pump(80)
        _ = AppE2E.wait(timeout: 12) {
            app.testPollOnce()
            AppE2E.pump(30)
            let status = app.lastSnapshot.status.lowercased()
            return status.contains("error")
                || status.contains("disconnect")
                || app.testNotificationsRecorded().contains { $0.lowercased().contains("fail") }
                || app.content.statusBar.testPopoverText().lowercased().contains("error")
        }
        // 不断言必须失败文案（网络环境差）；只断言主线程没被堵死。
        XCTAssertTrue(app.testWindowVisible(), "超时过程中窗口必须仍在")
    }
}
