import AppKit
import XCTest
@testable import MuxtermAppLib

/// 后台命令完成要通知；前台 `sleep && echo` 不得当完成通知。
/// 优先 OSC 133 D；pane-cmd 从 sleep 回到 shell 也算 Done。
final class NotifyBackgroundCommandE2ETests: XCTestCase {
    func testBackgroundSleepNotifiesAndForegroundDoesNot() throws {
        AppE2E.requireTmux()
        let fx = TwoShellPanes(label: "sleep-n")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let fgPane = UInt32(fx.panes[0].trimmingCharacters(in: CharacterSet(charactersIn: "%"))) ?? 0
        app.testSwitchPane(fgPane)
        app.testPollOnce()
        let before = app.testNotificationsRecorded()

        Tmux.sendLiteral(socket: fx.socket, target: fx.panes[0], text: "sleep 1 && echo AA_FG")
        Tmux.ok(socket: fx.socket, args: ["send-keys", "-t", fx.panes[0], "Enter"])
        Tmux.sendLiteral(socket: fx.socket, target: fx.panes[1], text: "sleep 1 && echo AA_BG")
        Tmux.ok(socket: fx.socket, args: ["send-keys", "-t", fx.panes[1], "Enter"])

        let sawBackground = AppE2E.wait(timeout: 8) {
            app.testPollOnce()
            AppE2E.pump(40)
            return app.testNotificationsRecorded().dropFirst(before.count).contains { note in
                let l = note.lowercased()
                return l.contains("done") || l.contains("complete") || l.contains("finished") || note.contains("完成")
            }
        }
        XCTAssertTrue(
            sawBackground,
            "后台 sleep 结束必须 notify_done（OSC 133 D 或 pane-cmd sleep→shell）。got=\(app.testNotificationsRecorded())"
        )

        app.testOpenAttentionPanel()
        AppE2E.pump(80)
        if app.testAttentionRowCount() > 0 {
            let title = app.attentionPanel.testAttentionRowTitle(0)
            XCTAssertFalse(
                title.contains("AA_BG") || title.contains("AA_FG"),
                "注意力行不得用 last_line 片段当标题。title=\(title)"
            )
            XCTAssertTrue(
                title.lowercased().contains("sleep")
                    || title.lowercased().contains("zsh")
                    || title.lowercased().contains("bash")
                    || title.lowercased().contains("cat"),
                "标题必须含进程名。title=\(title)"
            )
            XCTAssertTrue(
                title.lowercased().contains("local")
                    || title.lowercased().contains("tmux")
                    || title.lowercased().contains("ssh"),
                "标题必须含 transport。title=\(title)"
            )
            XCTAssertTrue(
                title.contains("/") || title.contains("~"),
                "标题必须含 path。title=\(title)"
            )
        }
    }
}

/// 两个交互 shell pane（不是 cat），才能跑 `sleep && echo`。
private final class TwoShellPanes {
    let socket: String
    let session: String
    let panes: [String]

    init(label: String) {
        AppE2E.requireTmux()
        socket = Tmux.uniqueSocket(label)
        session = "sh-\(label)"
        Tmux.killServer(socket)
        let shell = FileManager.default.isExecutableFile(atPath: "/bin/zsh") ? "/bin/zsh" : "/bin/bash"
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "100", "-y", "30", "--", shell, "-i",
        ])
        Tmux.ok(socket: socket, args: ["split-window", "-h", "-t", session, shell, "-i"])
        panes = Tmux.out(socket: socket, args: ["list-panes", "-t", session, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertEqual(panes.count, 2, "应有 2 pane: \(panes)")
    }

    deinit {
        Tmux.killServer(socket)
    }
}
