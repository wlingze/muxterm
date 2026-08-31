import AppKit
import Foundation
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 为会写 Core 配置的 E2E 提供独立 XDG 根目录，避免测试改动真实用户配置。
final class IsolatedMuxtermConfig {
    let root: URL
    let configURL: URL
    private let previousConfigHome: String?
    private var restored = false

    init(label: String, toml: String) throws {
        previousConfigHome = getenv("XDG_CONFIG_HOME").map { String(cString: $0) }
        root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "muxterm-config-\(label)-\(ProcessInfo.processInfo.processIdentifier)-\(UUID().uuidString)"
        )
        let directory = root.appendingPathComponent("muxterm")
        configURL = directory.appendingPathComponent("config.toml")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try toml.write(to: configURL, atomically: true, encoding: .utf8)
        setenv("XDG_CONFIG_HOME", root.path, 1)
    }

    func restore() {
        guard !restored else { return }
        restored = true
        if let previousConfigHome {
            setenv("XDG_CONFIG_HOME", previousConfigHome, 1)
        } else {
            unsetenv("XDG_CONFIG_HOME")
        }
        try? FileManager.default.removeItem(at: root)
    }

    deinit {
        restore()
    }
}

/// in-process AppKit e2e 夹具（对标 `tests/support/tmux_test_support.rs`）。
///
/// tmux **只** `-L muxterm-test-*`。Drop 时同一 `-L` 的 `kill-server`。
enum AppE2E {
    static let minPanePx: CGFloat = 40
    static let maxOutputEventsPerSec = 400
    static let cupFloodFrames: UInt32 = 400
    static let attachTimeout: TimeInterval = 8
    static let featureTimeout: TimeInterval = 10

    static var repoRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // AppE2ETests
            .deletingLastPathComponent() // macos
            .deletingLastPathComponent() // platform
            .deletingLastPathComponent() // src
            .deletingLastPathComponent() // repo
    }

    static func ensureApp() {
        // XCTest 的 memory checker 在 SwiftPM 测试进程里 dealloc NSWindow 会
        // 过度 release 崩溃（objc_release → EXC_BAD_ACCESS）。AppKit e2e 必须
        // 关掉它，否则每个建窗口的用例都 segfault。
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
    }

    static func pump(_ milliseconds: Int) {
        let end = Date().addingTimeInterval(Double(milliseconds) / 1000.0)
        while Date() < end {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.016))
        }
    }

    @discardableResult
    static func wait(timeout: TimeInterval, _ predicate: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if predicate() {
                return true
            }
            pump(30)
        }
        return predicate()
    }

    static func requireTmux() {
        XCTAssertTrue(Tmux.available, "需要本机 tmux（禁止 skip 冒充绿）")
    }

    /// macOS 26 会把压到 visibleFrame 之外的窗口上移。测试关心的是
    /// “attach 后不再改 frame”，不是绝对 (40, 40)，因此先选一个屏幕内
    /// 的固定 frame，再全程断言它不变。
    static func fixedWindowFrame(width: CGFloat, height: CGFloat) -> NSRect {
        let visible = NSScreen.main?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: width + 80, height: height + 80)
        let horizontalMargin = min(40, max(0, (visible.width - width) / 2))
        let verticalMargin = min(40, max(0, (visible.height - height) / 2))
        return NSRect(
            x: visible.minX + horizontalMargin,
            y: visible.minY + verticalMargin,
            width: width,
            height: height
        )
    }

    static func attachWindow(socket: String, session: String) throws -> MainWindowController {
        ensureApp()
        let bridge = try CoreBridge(backendType: "tmux", socket: socket, session: session)
        let wc = MainWindowController(bridge: bridge, debug: true)
        wc.window?.setFrame(fixedWindowFrame(width: 1280, height: 800), display: true)
        wc.window?.orderFront(nil)
        pump(200)
        FileHandle.standardError.write(Data("DEBUG FRAME after attach: \(wc.window?.frame.size ?? .zero)\n".utf8))
        return wc
    }

    /// SSH attach：alias 走 `sshAlias`，`socket` 只给隔离远端 `-L`。
    static func attachSshWindow(
        alias: String,
        remoteSocket: String,
        session: String
    ) throws -> MainWindowController {
        ensureApp()
        let bridge = try CoreBridge.connect(
            backendType: "ssh",
            socket: remoteSocket,
            session: session,
            sshAlias: alias
        )
        let wc = MainWindowController(bridge: bridge, debug: true)
        wc.window?.setFrame(fixedWindowFrame(width: 1280, height: 800), display: true)
        wc.window?.orderFront(nil)
        pump(200)
        return wc
    }
}

enum Tmux {
    static var bin: String {
        let candidates = [
            "/opt/homebrew/bin/tmux",
            "/usr/local/bin/tmux",
            "/usr/bin/tmux",
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) } ?? "tmux"
    }

    static var available: Bool {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: bin)
        proc.arguments = ["-V"]
        proc.standardOutput = FileHandle.nullDevice
        proc.standardError = FileHandle.nullDevice
        do {
            try proc.run()
            proc.waitUntilExit()
            return proc.terminationStatus == 0
        } catch {
            return false
        }
    }

    static func uniqueSocket(_ label: String) -> String {
        let nanos = UInt64(Date().timeIntervalSince1970 * 1_000_000_000)
        return "muxterm-test-\(label)-\(ProcessInfo.processInfo.processIdentifier)-\(nanos % 1_000_000_000)"
    }

    @discardableResult
    static func run(socket: String, args: [String]) -> (status: Int32, stdout: String, stderr: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: bin)
        proc.arguments = ["-L", socket] + args
        let out = Pipe()
        let err = Pipe()
        proc.standardOutput = out
        proc.standardError = err
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            return (1, "", error.localizedDescription)
        }
        let stdout = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let stderr = String(data: err.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return (proc.terminationStatus, stdout, stderr)
    }

    static func ok(socket: String, args: [String], file: StaticString = #filePath, line: UInt = #line) {
        let r = run(socket: socket, args: args)
        XCTAssertEqual(r.status, 0, "tmux \(args) 失败: \(r.stderr)", file: file, line: line)
    }

    static func out(socket: String, args: [String]) -> String {
        run(socket: socket, args: args).stdout.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func killServer(_ socket: String) {
        _ = run(socket: socket, args: ["kill-server"])
    }

    static func sendLiteral(socket: String, target: String, text: String) {
        ok(socket: socket, args: ["send-keys", "-t", target, "-l", text])
    }

    static func sendHex(socket: String, target: String, bytes: [UInt8]) {
        let hex = bytes.map { String(format: "%02x", $0) }
        ok(socket: socket, args: ["send-keys", "-t", target, "-H"] + hex)
    }

    static func waitCapture(
        socket: String,
        target: String,
        needle: String,
        timeout: TimeInterval = 5,
        history: Bool = false
    ) {
        var args = ["capture-pane", "-p", "-t", target]
        if history {
            args = ["capture-pane", "-p", "-S", "-", "-t", target]
        }
        let ok = AppE2E.wait(timeout: timeout) {
            out(socket: socket, args: args).contains(needle)
        }
        XCTAssertTrue(ok, "capture \(target) 应含 \(needle)")
    }
}

/// 2tab / 3pane /bin/cat，每个 pane 先涂 token 再 attach。
final class PaintedWorkspace {
    let socket: String
    let session: String
    let tab1Panes: [String]
    let tab1Tokens: [String]
    let tab2Token: String
    let tab2Pane: String

    init(label: String) {
        AppE2E.requireTmux()
        socket = Tmux.uniqueSocket(label)
        session = "att-\(label)"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        let w0 = Tmux.out(socket: socket, args: ["list-windows", "-t", session, "-F", "#{window_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        Tmux.ok(socket: socket, args: ["split-window", "-h", "-t", w0, "/bin/cat"])
        let firstPanes = Tmux.out(socket: socket, args: ["list-panes", "-t", w0, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertGreaterThanOrEqual(firstPanes.count, 2)
        Tmux.ok(socket: socket, args: ["split-window", "-v", "-t", firstPanes[1], "/bin/cat"])
        Tmux.ok(socket: socket, args: ["new-window", "-t", session, "/bin/cat"])
        // attach 必须落在 3 pane 那一页，而不是刚创建的 other（Linux 契约同款）。
        Tmux.ok(socket: socket, args: ["select-window", "-t", "\(session):0"])
        // 活跃 pane 固定为 pane0（ZoomE2ETests 断言 zoom 后 token0 还在）。
        Tmux.ok(socket: socket, args: ["select-pane", "-t", "\(session):0.0"])

        tab1Panes = Tmux.out(socket: socket, args: ["list-panes", "-t", w0, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertEqual(tab1Panes.count, 3, "tab1 应有 3 pane: \(tab1Panes)")
        tab1Tokens = (0..<3).map { "E2E_TAB1_TOKEN_\($0)_\(ProcessInfo.processInfo.processIdentifier)" }
        for (i, pane) in tab1Panes.enumerated() {
            Tmux.sendLiteral(socket: socket, target: pane, text: tab1Tokens[i])
            Tmux.waitCapture(socket: socket, target: pane, needle: tab1Tokens[i])
        }

        let windows = Tmux.out(socket: socket, args: ["list-windows", "-t", session, "-F", "#{window_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertGreaterThanOrEqual(windows.count, 2)
        let w1 = windows[1]
        tab2Pane = Tmux.out(socket: socket, args: ["list-panes", "-t", w1, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        tab2Token = "E2E_TAB2_TOKEN_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.sendLiteral(socket: socket, target: tab2Pane, text: tab2Token)
        Tmux.waitCapture(socket: socket, target: tab2Pane, needle: tab2Token)
    }

    deinit {
        Tmux.killServer(socket)
    }
}

/// 两 pane /bin/cat：pane0 搜索，pane1 后台 BEL/Done。
final class TwoPaneCat {
    let socket: String
    let session: String
    let panes: [String]
    let searchToken: String
    let bgToken: String

    init(label: String) {
        AppE2E.requireTmux()
        socket = Tmux.uniqueSocket(label)
        session = "feat-\(label)"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "100", "-y", "30", "--", "/bin/cat",
        ])
        Tmux.ok(socket: socket, args: ["split-window", "-h", "-t", session, "/bin/cat"])
        panes = Tmux.out(socket: socket, args: ["list-panes", "-t", session, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init)
        XCTAssertEqual(panes.count, 2, "应有 2 pane: \(panes)")
        searchToken = "E2E_SEARCH_\(ProcessInfo.processInfo.processIdentifier)"
        bgToken = "E2E_BG_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.sendLiteral(socket: socket, target: panes[0], text: searchToken)
        Tmux.sendLiteral(socket: socket, target: panes[1], text: bgToken)
        Tmux.waitCapture(socket: socket, target: panes[0], needle: searchToken)
        Tmux.waitCapture(socket: socket, target: panes[1], needle: bgToken)
    }

    func sendBelOnBackground() {
        Tmux.sendHex(socket: socket, target: panes[1], bytes: [0x07])
        Tmux.ok(socket: socket, args: ["send-keys", "-t", panes[1], "Enter"])
    }

    func sendOsc133DoneOnBackground() {
        let py = AppE2E.repoRoot.appendingPathComponent("tests/scripts/osc133_done.py")
        XCTAssertTrue(FileManager.default.isReadableFile(atPath: py.path), "缺少 \(py.path)")
        Tmux.ok(socket: socket, args: [
            "respawn-pane", "-k", "-t", panes[1], "python3 -u \(py.path)",
        ])
    }

    func respawnMockCodex(onPane index: Int) {
        let py = AppE2E.repoRoot.appendingPathComponent("tests/scripts/mock_codex.py")
        XCTAssertTrue(FileManager.default.isReadableFile(atPath: py.path), "缺少 \(py.path)")
        Tmux.ok(socket: socket, args: [
            "respawn-pane", "-k", "-t", panes[index],
            "MOCK_CODEX_FRAMES=6 MOCK_CODEX_SLEEP=0.03 python3 -u \(py.path)",
        ])
    }

    deinit {
        Tmux.killServer(socket)
    }
}

final class OnePaneCat {
    let socket: String
    let session: String
    let pane: String
    let token: String

    init(label: String) {
        AppE2E.requireTmux()
        socket = Tmux.uniqueSocket(label)
        session = "one-\(label)"
        token = "DISC_TOKEN_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        pane = Tmux.out(socket: socket, args: ["list-panes", "-t", session, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        Tmux.sendLiteral(socket: socket, target: pane, text: "\(token)\n")
        Tmux.waitCapture(socket: socket, target: pane, needle: token)
    }

    deinit {
        Tmux.killServer(socket)
    }
}

/// 含 attach 前历史、顶栏、正文和底部输入区的 Cursor/pi 风格画面夹具。
/// 它的真实 pane_current_command 是 python3，只验证通用 Surface/历史机制；
/// primary-screen/no-mouse 的真实 `pi` 身份由 PrimaryPiSplitWorkspace 覆盖。
/// 进程会在 tmux pane 尺寸变化后重画，不允许测试靠事后 resize 恢复。
final class AgentScreenWorkspace {
    let socket: String
    let session: String
    let pane: String
    let historyToken: String

    init(label: String) {
        AppE2E.requireTmux()
        socket = Tmux.uniqueSocket(label)
        session = "agent-\(label)"
        historyToken = "AGENT_HISTORY_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        pane = Tmux.out(socket: socket, args: ["list-panes", "-t", session, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""

        let py = AppE2E.repoRoot.appendingPathComponent("tests/scripts/mock_codex.py")
        XCTAssertTrue(FileManager.default.isReadableFile(atPath: py.path), "缺少 \(py.path)")
        Tmux.ok(socket: socket, args: [
            "respawn-pane", "-k", "-t", pane,
            "MOCK_CODEX_DYNAMIC_SIZE=1 MOCK_CODEX_HISTORY_LINES=72 "
                + "MOCK_CODEX_HISTORY_TOKEN=\(historyToken) "
                + "MOCK_CODEX_FRAMES=3 MOCK_CODEX_SLEEP=0.02 "
                + "python3 -u \(py.path)",
        ])

        for token in ["TOKEN_HEADER", "TOKEN_BODY", "TOKEN_PROMPT"] {
            Tmux.waitCapture(socket: socket, target: pane, needle: token)
        }
        Tmux.waitCapture(
            socket: socket,
            target: pane,
            needle: historyToken,
            history: true
        )
        let visible = Tmux.out(socket: socket, args: ["capture-pane", "-p", "-t", pane])
        XCTAssertFalse(
            visible.contains(historyToken),
            "夹具失败：attach 前历史 token 仍在可见屏。visible=\(visible)"
        )
    }

    deinit {
        Tmux.killServer(socket)
    }
}

/// 复现 1320：活动 tab 为上下分屏，上方是真正名为 `pi` 的 primary-screen
/// TUI；它不开 mouse/alternate mode，但有大量 OSC/CUP 历史。旧判断会把
/// 这些重绘网格误抓成 PaneHistory，导致上 pane 乱屏和数秒卡顿。
final class PrimaryPiSplitWorkspace {
    let socket: String
    let session: String
    let topPane: String
    let topPaneId: UInt32
    let bottomPane: String
    let bottomPaneId: UInt32
    let bottomToken: String
    let temporaryDirectory: URL

    init(label: String) throws {
        AppE2E.requireTmux()
        let fileManager = FileManager.default
        let source = AppE2E.repoRoot.appendingPathComponent("tests/scripts/pi_primary_tui.c")
        XCTAssertTrue(fileManager.isReadableFile(atPath: source.path), "缺少 \(source.path)")

        let temp = fileManager.temporaryDirectory.appendingPathComponent(
            "muxterm-pi-fixture-\(UUID().uuidString)",
            isDirectory: true
        )
        try fileManager.createDirectory(at: temp, withIntermediateDirectories: true)
        let executable = temp.appendingPathComponent("pi")
        let compiler = Process()
        compiler.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
        compiler.arguments = [
            "clang", "-std=c11", "-O0", source.path, "-o", executable.path,
        ]
        let compilerOutput = Pipe()
        compiler.standardOutput = compilerOutput
        compiler.standardError = compilerOutput
        try compiler.run()
        compiler.waitUntilExit()
        let diagnostics = String(
            data: compilerOutput.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        ) ?? ""
        guard compiler.terminationStatus == 0 else {
            try? fileManager.removeItem(at: temp)
            throw NSError(
                domain: "MuxtermAppE2ETests",
                code: Int(compiler.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: "编译 pi fixture 失败：\(diagnostics)"]
            )
        }

        let localSocket = Tmux.uniqueSocket(label)
        let localSession = "pi-\(label)"
        Tmux.killServer(localSocket)
        Tmux.ok(socket: localSocket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", localSession,
            "-x", "94", "-y", "51", "--", executable.path,
        ])
        let localTop = Tmux.out(socket: localSocket, args: [
            "list-panes", "-t", localSession, "-F", "#{pane_id}",
        ]).split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        guard let localTopId = UInt32(localTop.dropFirst()) else {
            Tmux.killServer(localSocket)
            try? fileManager.removeItem(at: temp)
            throw NSError(
                domain: "MuxtermAppE2ETests",
                code: 2,
                userInfo: [NSLocalizedDescriptionKey: "无效 pi pane id：\(localTop)"]
            )
        }
        Tmux.ok(socket: localSocket, args: [
            "split-window", "-v", "-t", localTop, "/bin/cat",
        ])
        let panes = Tmux.out(socket: localSocket, args: [
            "list-panes", "-t", localSession, "-F", "#{pane_id}",
        ]).split(whereSeparator: \.isNewline).map(String.init)
        let localBottom = panes.first(where: { $0 != localTop }) ?? ""
        guard let localBottomId = UInt32(localBottom.dropFirst()) else {
            Tmux.killServer(localSocket)
            try? fileManager.removeItem(at: temp)
            throw NSError(
                domain: "MuxtermAppE2ETests",
                code: 3,
                userInfo: [NSLocalizedDescriptionKey: "无效 bottom pane id：\(localBottom)"]
            )
        }
        let localBottomToken = "PI_E2E_BOTTOM_\(ProcessInfo.processInfo.processIdentifier)"
        let bottomHistory = (0..<80)
            .map { "BOTTOM_HISTORY_\(String(format: "%03d", $0))" }
            .joined(separator: "\n")
        Tmux.sendLiteral(
            socket: localSocket,
            target: localBottom,
            text: "\(bottomHistory)\n\(localBottomToken)\n"
        )
        Tmux.ok(socket: localSocket, args: ["select-pane", "-t", localTop])

        for token in ["PI_E2E_HEADER", "PI_E2E_BODY", "PI_E2E_PROMPT"] {
            Tmux.waitCapture(socket: localSocket, target: localTop, needle: token)
        }
        Tmux.waitCapture(
            socket: localSocket,
            target: localTop,
            needle: "PI_E2E_HISTORY_000",
            history: true
        )
        Tmux.waitCapture(
            socket: localSocket,
            target: localBottom,
            needle: "BOTTOM_HISTORY_000",
            history: true
        )
        Tmux.waitCapture(socket: localSocket, target: localBottom, needle: localBottomToken)

        let modes = Tmux.out(socket: localSocket, args: [
            "display-message", "-p", "-t", localTop,
            "#{pane_current_command}|#{alternate_on}|#{mouse_all_flag}|#{mouse_any_flag}|#{mouse_sgr_flag}|#{pane_top}",
        ])
        XCTAssertEqual(
            modes,
            "pi|0|0|0|0|0",
            "夹具必须命中上方真实 pi 的遗漏边界，而不是 python3/mock token：\(modes)"
        )

        socket = localSocket
        session = localSession
        topPane = localTop
        topPaneId = localTopId
        bottomPane = localBottom
        bottomPaneId = localBottomId
        bottomToken = localBottomToken
        temporaryDirectory = temp
    }

    deinit {
        Tmux.killServer(socket)
        try? FileManager.default.removeItem(at: temporaryDirectory)
    }
}

final class OffscreenHistory {
    let socket: String
    let session: String
    let pane: String
    let token: String
    let tailMark: String

    init(label: String) {
        AppE2E.requireTmux()
        socket = Tmux.uniqueSocket(label)
        session = "hist-\(label)"
        token = "HIST_OFFSCREEN_\(ProcessInfo.processInfo.processIdentifier)"
        tailMark = "HIST_TAIL_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.killServer(socket)
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        pane = Tmux.out(socket: socket, args: ["list-panes", "-t", session, "-F", "#{pane_id}"])
            .split(whereSeparator: \.isNewline).map(String.init).first ?? ""
        Tmux.sendLiteral(socket: socket, target: pane, text: "\(token)\n")
        for i in 1...40 {
            Tmux.sendLiteral(socket: socket, target: pane, text: String(format: "pad-%02d\n", i))
        }
        Tmux.sendLiteral(socket: socket, target: pane, text: "\(tailMark)\n")
        Tmux.waitCapture(socket: socket, target: pane, needle: token, history: true)
        Tmux.waitCapture(socket: socket, target: pane, needle: tailMark)
        let visible = Tmux.out(socket: socket, args: ["capture-pane", "-p", "-t", pane])
        XCTAssertFalse(visible.contains(token), "夹具失败：token 还在可见屏，无法证明历史。visible=\(visible)")
        XCTAssertTrue(visible.contains(tailMark), "可见屏应有尾标 \(tailMark)")
    }

    deinit {
        Tmux.killServer(socket)
    }
}

extension MainWindowController {
    func waitReady(minTabs: Int = 1, minLeaves: Int = 1) -> Bool {
        AppE2E.wait(timeout: AppE2E.attachTimeout) { [weak self] in
            guard let self else { return false }
            testPollOnce()
            // Surface seed/feed 在主线程异步完成；只看拓扑数量会让慢机器
            // 在 SwiftTerm 仍为空或 host 仍隐藏时就开始发送测试输入。
            testFlushFeeds()
            AppE2E.pump(30)
            testFlushFeeds()
            let counts = testTabAndPaneCounts()
            let leaves = testLayoutLeafIDs()
            guard counts.tabs >= minTabs, leaves.count >= minLeaves else {
                return false
            }
            return leaves.allSatisfy { testPaneSurfaceReady($0) }
        }
    }

    func waitTerminalContains(_ needle: String, timeout: TimeInterval = AppE2E.attachTimeout) -> Bool {
        AppE2E.wait(timeout: timeout) { [weak self] in
            guard let self else { return false }
            testPollOnce()
            testFlushFeeds()
            AppE2E.pump(30)
            return testAllVisibleTerminalText().contains(needle)
        }
    }
}
