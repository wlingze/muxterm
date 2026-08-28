import XCTest
@testable import MuxtermAppLib

/// attach 恢复 mouse mode 时必须以 pane 为单位判断，不能让一个退出异常的
/// TUI 把 SGR 点击序列泄漏到已经回到 shell 的 pane。
final class MouseModeAttachE2ETests: XCTestCase {
    func testAttachClearsStaleShellMouseModeWithoutAffectingTuiPane() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("mouse-mode")
        let session = "mouse-mode"
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }

        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "env PS1='MUX_MOUSE> ' /bin/sh",
        ])
        let shellPane = Tmux.out(
            socket: socket,
            args: ["list-panes", "-t", session, "-F", "#{pane_id}"]
        )
        Tmux.sendLiteral(
            socket: socket,
            target: shellPane,
            text: "printf '\\033[?1003h\\033[?1006hSHELL_MOUSE_STALE\\n'"
        )
        Tmux.ok(socket: socket, args: ["send-keys", "-t", shellPane, "Enter"])
        Tmux.waitCapture(socket: socket, target: shellPane, needle: "SHELL_MOUSE_STALE")

        Tmux.ok(socket: socket, args: ["split-window", "-h", "-t", session, "/bin/cat"])
        let panes = Tmux.out(
            socket: socket,
            args: ["list-panes", "-t", session, "-F", "#{pane_id}"]
        ).split(whereSeparator: \.isNewline).map(String.init)
        let tuiPane = try XCTUnwrap(panes.first { $0 != shellPane })
        Tmux.sendHex(
            socket: socket,
            target: tuiPane,
            bytes: Array("\u{1b}[?1003h\u{1b}[?1006hTUI_MOUSE_ACTIVE\r".utf8)
        )
        Tmux.waitCapture(socket: socket, target: tuiPane, needle: "TUI_MOUSE_ACTIVE")
        Tmux.ok(socket: socket, args: ["select-pane", "-t", shellPane])

        let stateFormat = "#{pane_current_command}|#{mouse_any_flag}|#{mouse_sgr_flag}"
        let shellState = Tmux.out(
            socket: socket,
            args: ["display-message", "-p", "-t", shellPane, stateFormat]
        )
        let tuiState = Tmux.out(
            socket: socket,
            args: ["display-message", "-p", "-t", tuiPane, stateFormat]
        )
        XCTAssertTrue(shellState.hasSuffix("|1|1"), "夹具必须制造 shell 陈旧 mouse state: \(shellState)")
        XCTAssertTrue(tuiState.hasSuffix("|1|1"), "夹具必须制造仍活跃的 TUI mouse state: \(tuiState)")

        let bridge = try CoreBridge.connect(
            backendType: "tmux",
            socket: socket,
            session: session,
            initialClientSize: (100, 30)
        )
        defer { bridge.shutdown() }

        let shellPaneId = try XCTUnwrap(UInt32(shellPane.dropFirst()))
        let tuiPaneId = try XCTUnwrap(UInt32(tuiPane.dropFirst()))
        var snapshots: [UInt32: Data] = [:]
        let received = AppE2E.wait(timeout: AppE2E.attachTimeout) {
            for event in bridge.pollEvents() where event.isPaneSnapshot {
                snapshots[event.paneId] = event.data
            }
            return snapshots[shellPaneId] != nil && snapshots[tuiPaneId] != nil
        }
        XCTAssertTrue(received, "attach 必须为两个 pane 分别发布 snapshot: \(snapshots.keys.sorted())")

        let shellSnapshot = String(decoding: snapshots[shellPaneId] ?? Data(), as: UTF8.self)
        let tuiSnapshot = String(decoding: snapshots[tuiPaneId] ?? Data(), as: UTF8.self)
        XCTAssertFalse(
            shellSnapshot.contains("\u{1b}[?1003h") || shellSnapshot.contains("\u{1b}[?1006h"),
            "已经回到 shell 的 pane 不得恢复陈旧 mouse mode，否则点击会变成 SGR 文本"
        )
        XCTAssertTrue(
            tuiSnapshot.contains("\u{1b}[?1003h") && tuiSnapshot.contains("\u{1b}[?1006h"),
            "仍在运行 mouse-aware TUI 的另一个 pane 必须保留 mouse mode"
        )
    }
}
