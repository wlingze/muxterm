import AppKit
import XCTest
@testable import MuxtermAppLib

/// macOS parity for Linux W18h plus the command timeline navigation promised by
/// the core OSC 133 index. The fixture uses an isolated tmux socket; OSC 133 is
/// emitted by a process after attach so it reaches the live `%output` stream
/// (tmux capture-pane intentionally does not retain consumed OSC frames).
final class CommandTimelineE2ETests: XCTestCase {
    func testCommandMarksExposeTimelineAndNavigateToLatest() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("cmd-timeline")
        let session = "cmd-timeline"
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        let pane = Tmux.out(socket: socket, args: [
            "list-panes", "-t", session, "-F", "#{pane_id}",
        ])
        let target = pane.split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        XCTAssertFalse(target.isEmpty)

        let app = try AppE2E.attachWindow(socket: socket, session: session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1), "attach 后必须有 pane")

        let script = AppE2E.repoRoot.appendingPathComponent("tests/scripts/osc133_rounds.py")
        XCTAssertTrue(FileManager.default.isReadableFile(atPath: script.path))
        let suffix = "MAC_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.ok(socket: socket, args: [
            "respawn-pane", "-k", "-t", target,
            "env MUXTERM_CMD_SUFFIX=\(suffix) MUXTERM_CMD_PAD_LINES=32 python3 -u \(script.path)",
        ])
        Tmux.waitCapture(socket: socket, target: target, needle: "CMD_FAIL_\(suffix)")
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testCommandMarkVisible().ok && app.testCommandMarkVisible().fail
            },
            "OSC 133 成功/失败刻度必须到达 macOS UI"
        )

        // 第一次 Cmd+Option+↑ 选最后一个失败命令；第二次才回到离屏成功命令。
        let previous = try XCTUnwrap(app.testMakeCommandTimelineEvent(up: true))
        XCTAssertTrue(app.testDispatchKeyEvent(previous), "Cmd+Option+↑ 必须被窗口快捷键消费")
        XCTAssertTrue(app.testDispatchKeyEvent(previous), "第二次 Cmd+Option+↑ 必须继续沿命令轨跳转")
        AppE2E.pump(80)
        XCTAssertGreaterThan(app.testPaneViewport(), 0, "上一条命令应能跳进离屏历史")

        // Cmd+Option+↓ 回到失败刻度，再下一次等价于向下滚动到实时底部。
        let next = try XCTUnwrap(app.testMakeCommandTimelineEvent(up: false))
        XCTAssertTrue(app.testDispatchKeyEvent(next), "Cmd+Option+↓ 必须被窗口快捷键消费")
        XCTAssertTrue(app.testDispatchKeyEvent(next), "末尾 Cmd+Option+↓ 必须回到底部")
        AppE2E.pump(80)
        XCTAssertEqual(app.testPaneViewport(), 0, "命令时间线末尾必须回到底部")
    }
}
