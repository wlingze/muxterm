import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 慢速 SSH attach 的进度必须盖住主内容区，不是一个小对话框。
final class ConnectProgressE2ETests: XCTestCase {
    func testUnreachableSshShowsFullWindowProgress() throws {
        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "conn-prog")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

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
            "主内容必须出现 \(ConnectProgress.identifier)，不能只用小 alert"
        )
        let value = app.testConnectProgressValue().lowercased()
        let stages = ConnectProgressStage.allCases.map(\.rawValue)
        XCTAssertTrue(
            stages.contains(where: { value.contains($0) }),
            "进度 AX value 必须含 resolving/ssh/list-sessions/attach/capture 之一。got=\(value)"
        )
        XCTAssertTrue(app.testWindowVisible(), "进度过程中主窗口必须还在")
    }
}
