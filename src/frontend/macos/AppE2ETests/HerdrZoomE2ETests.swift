import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Real SwiftTerm window + isolated Herdr server. The tmux zoom tests do not
/// exercise Herdr's own layout and full-frame stream.
final class HerdrZoomE2ETests: XCTestCase {
    func testCommandEnterZoomsHerdrAndKeepsPaintedPaneVisible() throws {
        XCTAssertTrue(NamedHerdrZoomFixture.available, "herdr 0.8.0 is required")
        let fixture = try NamedHerdrZoomFixture()
        defer { fixture.stop() }

        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let opened = try bridge.openWorkspace(
            target: TargetConfig(
                name: "herdr-zoom", runtime: .herdr, transport: .local,
                path: "/tmp", session: fixture.name, socket: fixture.socket,
                workspaceID: fixture.workspaceID
            ),
            intent: .attachOnly,
            initialClientSize: (120, 40)
        )
        XCTAssertEqual(bridge.workspaceList().first(where: \.active)?.id, opened.id)
        let app = MainWindowController(bridge: bridge, debug: true)
        defer { app.testShutdown() }
        app.window?.setFrame(AppE2E.fixedWindowFrame(width: 1000, height: 700), display: true)
        app.window?.orderFront(nil)
        AppE2E.pump(200)

        XCTAssertTrue(app.waitReady(minLeaves: 2), "Herdr split must paint both panes")
        XCTAssertTrue(app.waitTerminalContains(fixture.token), "painted Herdr pane must not be blank")
        let herdrSidebarID = try XCTUnwrap(
            app.testWorkspaceIDs().first(where: { $0.contains(fixture.name) })
        )
        let key = try XCTUnwrap(app.testMakeCmdEnterEvent())
        XCTAssertTrue(app.testDispatchKeyEvent(key))
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            return fixture.isZoomed
                && app.testLayoutLeafIDs().count == 1
                && app.testAllVisibleTerminalText().contains(fixture.token)
        }, "Cmd-Enter must zoom the server and retain the painted SwiftTerm surface")

        XCTAssertTrue(app.testDispatchKeyEvent(key))
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            return !fixture.isZoomed && app.testLayoutLeafIDs().count == 2
        }, "second Cmd-Enter must restore both Herdr panes")

        // Attach through the production Existing Connection callback and
        // switch back. Frames from the two runtimes must remain in their own
        // persistent surfaces even if both contain a pane with a low ID.
        let tmux = OnePaneCat(label: "herdr-tmux-switch")
        app.testAttachExistingConnection(ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(name: tmux.session, windowCount: 1, attached: false),
            socket: tmux.socket
        ))
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            return app.testActiveWorkspaceSession() == tmux.session
                && app.testAllVisibleTerminalText().contains(tmux.token)
                && !app.testAllVisibleTerminalText().contains(fixture.token)
        }, "tmux attach must display only the tmux pane")
        app.refreshWorkspaceSidebarForTest()
        XCTAssertTrue(app.testWorkspaceIDs().contains(herdrSidebarID),
                      "Herdr workspace must stay in sidebar: \(app.testWorkspaceIDs())")
        app.testSelectSidebarWorkspace(herdrSidebarID)
        let restored = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            return app.testAllVisibleTerminalText().contains(fixture.token)
                && !app.testAllVisibleTerminalText().contains(tmux.token)
        }
        XCTAssertTrue(restored, "returning to Herdr must restore its own painted surface; "
            + "selected=\(app.testSelectedSidebarWorkspaceID() ?? "nil") "
            + "active=\(app.bridge.workspaceList().first(where: \.active)?.id ?? "nil") "
            + "leaves=\(app.testLayoutLeafIDs()) "
            + "text=\(app.testAllVisibleTerminalText().prefix(240))")
        guard restored else { return }

        try fixture.startArrowProbe()
        XCTAssertTrue(app.waitTerminalContains("HERDR_KEY_READY"), "raw pager must become visible")
        app.testMakeActiveTerminalFirstResponder()
        let arrow = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: .function,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: try XCTUnwrap(app.window).windowNumber, context: nil,
            characters: "\u{F700}", charactersIgnoringModifiers: "\u{F700}",
            isARepeat: false, keyCode: 126
        ))
        let routed = try XCTUnwrap(app.testRouteMonitoredKeyEvent(arrow))
        app.testActiveTerminalView().keyDown(with: routed)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            // Narrow panes can wrap the marker across terminal rows. Compare
            // the displayed characters after removing row boundaries.
            let text = app.testActivePaneTerminalText().filter { !$0.isWhitespace }
            return text.contains("HERDR_KEY_BYTES=1b5b41")
                || text.contains("HERDR_KEY_BYTES=1b4f41")
        }, "Up arrow must reach the Herdr pane's raw-mode program after a workspace round trip; "
            + "server=\(fixture.recentScreen().suffix(320)) "
            + "ui=\(app.testAllVisibleTerminalText().suffix(320)) "
            + "active=\(app.bridge.workspaceList().first(where: \.active)?.id ?? "nil") "
            + "responder=\(String(describing: app.window?.firstResponder))")
    }
}

private final class NamedHerdrZoomFixture {
    let name: String
    let socket: String
    private(set) var workspaceID = ""
    private(set) var paneID = ""
    let token: String
    let probeScriptPath: URL
    private let server: Process
    private var stopped = false

    private static var executable: URL {
        let local = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".local/bin/herdr")
        return FileManager.default.isExecutableFile(atPath: local.path)
            ? local : URL(fileURLWithPath: "/usr/local/bin/herdr")
    }

    static var available: Bool {
        guard let output = try? command(["--version"]),
              let version = String(data: output, encoding: .utf8) else { return false }
        return version.contains("0.8.0")
    }

    init() throws {
        name = "muxterm-test-maczoom-\(ProcessInfo.processInfo.processIdentifier)-\(UInt64(Date().timeIntervalSince1970 * 1_000_000) % 1_000_000)"
        let base = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"]
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config").path
        socket = URL(fileURLWithPath: base)
            .appendingPathComponent("herdr/sessions/\(name)/herdr.sock").path
        token = "HERDR_MAC_ZOOM_\(ProcessInfo.processInfo.processIdentifier)"
        probeScriptPath = FileManager.default.temporaryDirectory
            .appendingPathComponent("\(name)-arrow.py")
        server = Process()
        server.executableURL = Self.executable
        server.arguments = ["--session", name, "server"]
        server.standardOutput = FileHandle.nullDevice
        server.standardError = FileHandle.nullDevice
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "HERDR_ENV")
        environment.removeValue(forKey: "HERDR_SESSION")
        server.environment = environment
        do {
            try server.run()
            guard AppE2E.wait(timeout: 8, { FileManager.default.fileExists(atPath: socket) }) else {
                throw NSError(domain: "HerdrZoomFixture", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "named Herdr socket did not appear"])
            }
            let created = try Self.json(try cli(["workspace", "create", "--cwd", "/tmp", "--label", "mac-zoom"]))
            let result = created["result"] as? [String: Any] ?? [:]
            workspaceID = ((result["workspace"] as? [String: Any])?["workspace_id"] as? String) ?? ""
            paneID = ((result["root_pane"] as? [String: Any])?["pane_id"] as? String) ?? ""
            guard !workspaceID.isEmpty, !paneID.isEmpty else {
                throw NSError(domain: "HerdrZoomFixture", code: 2,
                              userInfo: [NSLocalizedDescriptionKey: "workspace create omitted ids"])
            }
            _ = try cli(["pane", "split", paneID, "--direction", "right", "--no-focus"])
            guard AppE2E.wait(timeout: 8, { [self] in
                guard let info = try? Self.json(try cli(["pane", "process-info", "--pane", paneID])),
                      let result = info["result"] as? [String: Any],
                      let process = result["process_info"] as? [String: Any] else { return false }
                return (process["shell_pid"] as? Int ?? 0) > 0
            }) else {
                throw NSError(domain: "HerdrZoomFixture", code: 3,
                              userInfo: [NSLocalizedDescriptionKey: "named Herdr pane shell did not start"])
            }
            _ = try cli(["pane", "run", paneID, "printf", "%s\\n", token])
            guard AppE2E.wait(timeout: 8, { [self] in
                guard let data = try? cli(["pane", "read", paneID, "--source", "recent", "--format", "text"]),
                      let screen = String(data: data, encoding: .utf8) else { return false }
                return screen.contains(token)
            }) else {
                throw NSError(domain: "HerdrZoomFixture", code: 4,
                              userInfo: [NSLocalizedDescriptionKey: "fixture token missing on Herdr server"])
            }
        } catch {
            stop()
            throw error
        }
    }

    var isZoomed: Bool {
        guard let data = try? cli(["pane", "layout", "--pane", paneID]),
              let json = try? Self.json(data),
              let result = json["result"] as? [String: Any],
              let layout = result["layout"] as? [String: Any] else { return false }
        return layout["zoomed"] as? Bool ?? false
    }

    func startArrowProbe() throws {
        let script = """
        import sys,termios,tty
        fd=sys.stdin.fileno(); old=termios.tcgetattr(fd)
        try:
         tty.setraw(fd); print('HERDR_KEY_READY',flush=True)
         key=sys.stdin.buffer.read(3)
         print('HERDR_KEY_BYTES='+key.hex(),flush=True)
        finally:
         termios.tcsetattr(fd,termios.TCSADRAIN,old)
        """
        try script.write(to: probeScriptPath, atomically: true, encoding: .utf8)
        _ = try cli(["pane", "run", paneID, "python3", "-u", probeScriptPath.path])
    }

    func recentScreen() -> String {
        guard let data = try? cli(["pane", "read", paneID, "--source", "recent", "--format", "text"]) else {
            return "<read failed>"
        }
        return String(decoding: data, as: UTF8.self)
    }

    func stop() {
        guard !stopped, name.hasPrefix("muxterm-test-") else { return }
        stopped = true
        _ = try? Self.command(["session", "stop", name])
        _ = try? Self.command(["session", "delete", name])
        if server.isRunning { server.terminate() }
        if probeScriptPath.lastPathComponent.hasPrefix("muxterm-test-maczoom-") {
            try? FileManager.default.removeItem(at: probeScriptPath)
        }
    }

    private func cli(_ args: [String]) throws -> Data {
        try Self.command(["--session", name] + args)
    }

    private static func command(_ args: [String]) throws -> Data {
        let process = Process()
        process.executableURL = executable
        process.arguments = args
        process.standardInput = FileHandle.nullDevice
        let output = Pipe()
        let error = Pipe()
        process.standardOutput = output
        process.standardError = error
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "HERDR_ENV")
        environment.removeValue(forKey: "HERDR_SESSION")
        process.environment = environment
        try process.run()
        process.waitUntilExit()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        if process.terminationStatus != 0 {
            let detail = String(data: error.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            throw NSError(domain: "HerdrZoomFixture", code: Int(process.terminationStatus),
                          userInfo: [NSLocalizedDescriptionKey: "herdr \(args): \(detail)"])
        }
        return data
    }

    private static func json(_ data: Data) throws -> [String: Any] {
        try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
    }
}
