import AppKit
import XCTest
@testable import MuxtermAppLib

/// 对标 `linux_live_e2e`：echo 进 SwiftTerm、CUP 停在末帧、点 status tab 切 window。
final class LiveE2ETests: XCTestCase {
    func testLiveEchoCupAndStatusTabSwitch() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("live")
        let session = "s"
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24",
        ])
        Tmux.ok(socket: socket, args: ["new-window", "-t", session])

        let app = try AppE2E.attachWindow(socket: socket, session: session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2), "live attach 后应有 2 tab")

        Tmux.ok(socket: socket, args: ["send-keys", "-t", session, "echo MUXTERM_LIVE_TOKEN", "Enter"])
        XCTAssertTrue(
            app.waitTerminalContains("MUXTERM_LIVE_TOKEN", timeout: 5),
            "5s 内 echo 应到达 SwiftTerm"
        )

        Tmux.ok(socket: socket, args: [
            "send-keys", "-t", session,
            #"python3 -c 'import sys; [sys.stdout.write("\x1b[H\x1b[2Jframe-%d\n"%i) or sys.stdout.flush() for i in range(20)]'"#,
            "Enter",
        ])
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                app.testFlushFeeds()
                let vte = app.testActivePaneTerminalText()
                if vte.contains("frame-19") {
                    XCTAssertFalse(vte.contains("frame-0"), "SwiftTerm 不应残留 frame-0: \(vte)")
                    return true
                }
                return false
            },
            "5s 内 SwiftTerm 应停在 frame-19"
        )

        let tabs = app.testTabIDs()
        let current = app.testActiveTabID()
        let other = try XCTUnwrap(tabs.first { $0 != current })
        app.testClickStatusTab(other)
        XCTAssertEqual(app.testActiveTabID(), other, "点 status tab 必须切 window")
    }
}
