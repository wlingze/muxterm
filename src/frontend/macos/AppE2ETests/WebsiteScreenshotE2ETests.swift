import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Renders a real macOS workbench window against an isolated tmux server with
/// mock terminal content. With `MUXTERM_UI_SNAPSHOT_DIR` set, the window is
/// written as a PNG for the official website and visual QA.
final class WebsiteScreenshotE2ETests: XCTestCase {
    private var config: IsolatedMuxtermConfig?
    private var windowController: MainWindowController?
    private var socket: String?

    override func tearDown() {
        windowController?.testShutdown()
        windowController = nil
        if let socket {
            Tmux.killServer(socket)
        }
        socket = nil
        config?.restore()
        config = nil
        NSApplication.shared.appearance = nil
        super.tearDown()
    }

    func testWorkbenchWindowSnapshot() throws {
        AppE2E.ensureApp()
        AppE2E.requireTmux()

        // Keep the screenshot deterministic and do not read the user's config.
        let appearance = ProcessInfo.processInfo.environment["MUXTERM_SCREENSHOT_APPEARANCE"] == "light"
            ? "light"
            : "dark"
        let themeName = appearance == "light" ? "white" : "black"
        config = try IsolatedMuxtermConfig(
            label: "website-screenshot-\(appearance)",
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

        let socket = Tmux.uniqueSocket("website")
        self.socket = socket
        let session = "workbench"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "120", "-y", "40", "--", "/bin/cat",
        ])

        let window0 = Tmux.out(socket: socket, args: ["list-windows", "-t", session, "-F", "#{window_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        Tmux.ok(socket: socket, args: ["rename-window", "-t", window0, "build"])
        Tmux.ok(socket: socket, args: ["split-window", "-h", "-t", window0, "/bin/cat"])
        let firstPanes = Tmux.out(socket: socket, args: ["list-panes", "-t", window0, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertEqual(firstPanes.count, 2)
        Tmux.ok(socket: socket, args: ["split-window", "-v", "-t", firstPanes[0], "/bin/cat"])
        Tmux.ok(socket: socket, args: ["split-window", "-v", "-t", firstPanes[1], "/bin/cat"])
        let panes = Tmux.out(socket: socket, args: ["list-panes", "-t", window0, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertEqual(panes.count, 4)

        Tmux.ok(socket: socket, args: ["new-window", "-t", session, "-n", "agent", "/bin/cat"])
        let windows = Tmux.out(socket: socket, args: ["list-windows", "-t", session, "-F", "#{window_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        let window1 = try XCTUnwrap(windows.last)
        let agentPane = Tmux.out(socket: socket, args: ["list-panes", "-t", window1, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""

        let esc = "\u{001B}["
        let reset = "\(esc)0m"
        let green = "\(esc)32m"
        let dim = "\(esc)90m"
        let bold = "\(esc)1m"

        let buildPane = panes[0]
        let testsPane = panes[1]
        let statusPane = panes[2]
        let agentPaneSmall = panes[3]

        send(socket: socket, pane: buildPane, text: """
        \(esc)2J\(esc)H\(green)$\(reset) cargo test --workspace
        \(dim)   Compiling muxterm\(reset)
        \(dim)    Finished test profile\(reset)
        \(dim)     Running unittests src/lib.rs\(reset)

        test config::parses_workspace ... ok
        test runtime::tmux::attaches ... ok
        test workspace::switches_scene ... ok
        test activity::attention::blocked ... ok
        test ports::forward::localhost ... ok

        \(bold)test result: ok.\(reset)

        \(green)$\(reset) cargo clippy --workspace -- -D warnings
        \(dim)    Checking muxterm\(reset)
        \(dim)    Finished dev profile\(reset)
        """)

        send(socket: socket, pane: testsPane, text: """
        \(esc)2J\(esc)H\(green)$\(reset) muxterm list-workspaces
        [1] workbench @ local (tmux, active)
        [2] yaklang @ remote (herdr)
        [3] staging-node @ aws (ssh+tmux)

        \(dim)workspace = runtime(transport) + path\(reset)
        \(dim)tabs       native\(reset)
        \(dim)panes      native\(reset)

        \(green)$\(reset) git worktree list
        ~/src/muxterm          a1b2c3d [main]
        ~/src/muxterm-docs     d4e5f6a [docs-site]
        ~/src/muxterm-ports    b7c8d9e [ports]
        """)

        send(socket: socket, pane: statusPane, text: """
        \(esc)2J\(esc)H\(green)$\(reset) git status
        On branch main
        nothing to commit, working tree clean

        \(dim)Runtime    tmux\(reset)
        \(dim)Transport  local\(reset)
        \(dim)Scene      resident\(reset)
        \(dim)Ports      3000 5432 8080\(reset)

        \(green)$\(reset) git log --oneline
        a1b2c3d feat: workbench
        d4e5f6a fix: port forwarding
        b7c8d9e docs: runtime model
        e9f0a1b test: scene switch
        """)

        send(socket: socket, pane: agentPaneSmall, text: """
        \(esc)2J\(esc)H\(green)agent\(reset)
        reviewing the diff
        \(dim)waiting for input\(reset)

        \(green)$\(reset) cargo run --bin server
        \(dim)listening on 127.0.0.1:3000\(reset)
        \(dim)TargetConnection reused over ssh\(reset)

        \(dim)agent: reviewing the diff\(reset)
        \(dim)agent: waiting for input\(reset)
        """)

        Tmux.ok(socket: socket, args: ["select-window", "-t", "\(session):0"])
        Tmux.ok(socket: socket, args: ["select-pane", "-t", buildPane])

        let bridge = try CoreBridge(backendType: "tmux", socket: socket, session: session)
        let controller = MainWindowController(bridge: bridge, debug: false)
        windowController = controller
        guard let window = controller.window else {
            return XCTFail("main window must exist")
        }
        window.setFrame(AppE2E.fixedWindowFrame(width: 1280, height: 760), display: true)
        window.orderFront(nil)
        AppE2E.pump(200)
        controller.testSetSidebarOpen(true)
        XCTAssertTrue(controller.waitReady(minTabs: 2, minLeaves: 4))
        controller.testSetSidebarOpen(true)
        controller.workspaceSidebar.testSetSectionExpanded(.agents, false)
        controller.workspaceSidebar.testSetSectionExpanded(.commands, false)
        controller.workspaceSidebar.testSetSectionExpanded(.hiddenCommands, false)
        AppE2E.pump(600)
        controller.testFlushFeeds()

        let visible = controller.testAllVisibleTerminalText()
        XCTAssertTrue(visible.contains("muxterm list-workspaces"), "mock build pane must render")
        XCTAssertTrue(visible.contains("Runtime    tmux"), "mock status pane must render")
        XCTAssertTrue(visible.contains("agent"), "mock agent pane must render")

        guard let directory = ProcessInfo.processInfo.environment["MUXTERM_UI_SNAPSHOT_DIR"],
              !directory.isEmpty
        else {
            return
        }
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let view = try XCTUnwrap(window.contentView)
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        try png.write(
            to: root.appendingPathComponent("app-workspace-\(appearance).png"),
            options: .atomic
        )
    }

    private func send(socket: String, pane: String, text: String) {
        Tmux.sendLiteral(socket: socket, target: pane, text: text)
    }
}
