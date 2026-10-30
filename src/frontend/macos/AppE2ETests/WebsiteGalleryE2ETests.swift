import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Captures the Agents aggregate and the local Shells aggregate for the website.
/// Both use isolated servers/processes and mock terminal output.
final class WebsiteGalleryE2ETests: XCTestCase {
    private var config: IsolatedMuxtermConfig?
    private var controllers: [MainWindowController] = []
    private var sockets: [String] = []
    private var temporaryDirectories: [URL] = []
    private var savedEnvironment: [String: String?] = [:]

    override func tearDown() {
        for controller in controllers {
            controller.testShutdown()
        }
        controllers = []
        for socket in sockets {
            Tmux.killServer(socket)
        }
        sockets = []
        for directory in temporaryDirectories {
            try? FileManager.default.removeItem(at: directory)
        }
        temporaryDirectories = []
        for (key, value) in savedEnvironment {
            if let value {
                setenv(key, value, 1)
            } else {
                unsetenv(key)
            }
        }
        savedEnvironment = [:]
        config?.restore()
        config = nil
        NSApplication.shared.appearance = nil
        super.tearDown()
    }

    func testAgentAggregateSnapshot() throws {
        AppE2E.ensureApp()
        AppE2E.requireTmux()
        let appearance = try prepare(label: "gallery-agents")

        let socket = Tmux.uniqueSocket("gallery-agents")
        sockets.append(socket)
        let session = "agent-review"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-n", "codex", "-x", "120", "-y", "40", "--", "/bin/cat",
        ])
        Tmux.ok(socket: socket, args: ["new-window", "-t", session, "-n", "claude", "/bin/cat"])
        let panes = Tmux.out(socket: socket, args: ["list-panes", "-a", "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertEqual(panes.count, 2)

        let esc = "\u{001B}["
        let reset = "\(esc)0m"
        let green = "\(esc)32m"
        let dim = "\(esc)90m"
        let bold = "\(esc)1m"

        Tmux.sendLiteral(socket: socket, target: panes[0], text: """
        \(esc)2J\(esc)H\(green)$\(reset) codex review src/core
        \(dim)reading runtime and workspace boundaries\(reset)

        \(green)Codex\(reset)  reviewing the port forwarding diff
          src/core/ports/forward.rs
          src/core/runtime/ssh/target.rs
          src/frontend/macos/StatusBar/PortPicker.swift

          reuse the existing TargetConnection
          keep the scan off the main thread
          expose the local address after forwarding

        \(green)$\(reset) cargo test -p muxterm ports
        test ports::forward::localhost ... ok
        test ports::forward::lan ... ok
        test ports::forward::stop ... ok

        \(bold)test result: ok.\(reset)
        \(dim)next: add a reconnect regression test\(reset)
        """)
        Tmux.sendLiteral(socket: socket, target: panes[1], text: """
        \(esc)2J\(esc)H\(green)$\(reset) claude
        \(dim)drafting release notes\(reset)

        \(green)Claude\(reset)  writing the workbench summary
          workspaces from tmux and Herdr
          native tabs and panes
          ports and forwarding
          attention and agents

        \(green)$\(reset) git diff --stat
         docs/USER-GUIDE.md   | 120 ++++++++++++
         README.md            |  34 ++++--
         website/index.html   |  80 +++++++++----

        \(dim)waiting for review\(reset)
        """)
        Tmux.ok(socket: socket, args: ["select-window", "-t", "\(session):0"])
        Tmux.ok(socket: socket, args: ["select-pane", "-t", panes[0]])

        let bridge = try CoreBridge(backendType: "tmux", socket: socket, session: session)
        let controller = MainWindowController(bridge: bridge, debug: false)
        controllers.append(controller)
        guard let window = controller.window else {
            return XCTFail("main window must exist")
        }
        window.setFrame(AppE2E.fixedWindowFrame(width: 1280, height: 760), display: true)
        window.orderFront(nil)
        AppE2E.pump(200)
        XCTAssertTrue(controller.waitReady(minTabs: 2, minLeaves: 1))

        let paneIDs = panes.compactMap { UInt32($0.dropFirst()) }
        XCTAssertEqual(paneIDs.count, 2)
        controller.testInjectAgent(
            paneId: paneIDs[0],
            name: "Codex",
            title: "Review port forwarding"
        )
        controller.testInjectAgent(
            paneId: paneIDs[1],
            name: "Claude",
            title: "Write release notes"
        )
        controller.testOpenAgents()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            controller.testPollOnce()
            controller.testFlushFeeds()
            return controller.testPresentedTabIDs().count == 2
        })
        controller.testSetSidebarOpen(true)
        controller.workspaceSidebar.testSetSectionExpanded(.agents, true)
        controller.workspaceSidebar.testSetSectionExpanded(.commands, false)
        controller.workspaceSidebar.testSetSectionExpanded(.hiddenCommands, false)
        AppE2E.pump(500)
        controller.testFlushFeeds()

        XCTAssertEqual(controller.testSelectedSidebarWorkspaceID(), AggregateWorkspaceIdentity.agents)
        XCTAssertGreaterThanOrEqual(controller.testSidebarAgentCount(), 2)
        try capture(controller, scene: "agents", appearance: appearance)
    }

    func testShellWorkspaceSnapshot() throws {
        AppE2E.ensureApp()
        let appearance = try prepare(label: "gallery-shells")

        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-website-shell-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        temporaryDirectories.append(directory)
        try """
        unsetopt rcs
        PROMPT='$ '
        RPROMPT=''
        printf '\\033]0;local shell\\007'
        printf '\\033[2J\\033[H'
        printf 'local shell\\n\\n'
        printf '$ muxterm list-workspaces\\n'
        printf '[1] workbench   @ local (tmux)\\n'
        printf '[2] yaklang     @ remote (herdr)\\n'
        printf '[3] staging     @ aws (ssh+tmux)\\n\\n'
        printf '$ muxterm config doctor\\n'
        printf 'config      ok\\n'
        printf 'runtime     shell, tmux, herdr\\n'
        printf 'transport   local, ssh\\n\\n'
        printf '$ cargo check --workspace\\n'
        printf '    Checking muxterm\\n'
        printf '    Finished dev profile\\n\\n'
        printf '$ git status --short\\n'
        printf ' M docs/USER-GUIDE.md\\n'
        printf ' M website/index.html\\n\\n'
        """.write(to: directory.appendingPathComponent(".zshenv"), atomically: true, encoding: .utf8)

        for (key, value) in [
            ("SHELL", "/bin/zsh"),
            ("ZDOTDIR", directory.path),
        ] {
            savedEnvironment[key] = getenv(key).map { String(cString: $0) }
            setenv(key, value, 1)
        }
        for key in ["TERM", "LANG", "LC_ALL", "LC_CTYPE"] {
            savedEnvironment[key] = getenv(key).map { String(cString: $0) }
            unsetenv(key)
        }

        let bridge = try CoreBridge(backendType: "local")
        let controller = MainWindowController(bridge: bridge, debug: false)
        controllers.append(controller)
        guard let window = controller.window else {
            return XCTFail("main window must exist")
        }
        window.setFrame(AppE2E.fixedWindowFrame(width: 1280, height: 760), display: true)
        window.orderFront(nil)
        AppE2E.pump(200)
        XCTAssertTrue(controller.waitReady())
        controller.testOpenShells()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            controller.testPollOnce()
            controller.testFlushFeeds()
            return controller.testAllVisibleTerminalText().contains("local shell")
        })
        controller.testSetSidebarOpen(true)
        controller.workspaceSidebar.testSetSectionExpanded(.agents, false)
        controller.workspaceSidebar.testSetSectionExpanded(.commands, false)
        controller.workspaceSidebar.testSetSectionExpanded(.hiddenCommands, false)
        AppE2E.pump(500)
        controller.testFlushFeeds()

        try capture(controller, scene: "shells", appearance: appearance)
    }

    private func prepare(label: String) throws -> String {
        let appearance = ProcessInfo.processInfo.environment["MUXTERM_SCREENSHOT_APPEARANCE"] == "light"
            ? "light"
            : "dark"
        let themeName = appearance == "light" ? "white" : "black"
        config = try IsolatedMuxtermConfig(
            label: label,
            toml: """
            config_version = 1

            [theme]
            name = "\(themeName)"
            light = "white"
            dark = "black"

            [statusbar]
            mode = "theme"

            [update]
            auto_check = false
            """
        )
        NSApplication.shared.appearance = NSAppearance(
            named: appearance == "light" ? .aqua : .darkAqua
        )
        return appearance
    }

    private func capture(
        _ controller: MainWindowController,
        scene: String,
        appearance: String
    ) throws {
        guard let directory = ProcessInfo.processInfo.environment["MUXTERM_UI_SNAPSHOT_DIR"],
              !directory.isEmpty
        else {
            return
        }
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let view = try XCTUnwrap(controller.window?.contentView)
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        try png.write(
            to: root.appendingPathComponent("app-\(scene)-\(appearance).png"),
            options: .atomic
        )
    }
}
