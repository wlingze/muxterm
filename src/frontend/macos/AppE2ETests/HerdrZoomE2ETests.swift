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
    }
}

private final class NamedHerdrZoomFixture {
    let name: String
    let socket: String
    private(set) var workspaceID = ""
    private(set) var paneID = ""
    let token: String
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

    func stop() {
        guard !stopped, name.hasPrefix("muxterm-test-") else { return }
        stopped = true
        _ = try? Self.command(["session", "stop", name])
        _ = try? Self.command(["session", "delete", name])
        if server.isRunning { server.terminate() }
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
