import AppKit
import Foundation
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

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

    static func attachWindow(socket: String, session: String) throws -> MainWindowController {
        ensureApp()
        let bridge = try CoreBridge(backendType: "tmux", socket: socket, session: session)
        let wc = MainWindowController(bridge: bridge, debug: true)
        wc.window?.setFrame(NSRect(x: 40, y: 40, width: 1280, height: 800), display: true)
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
        wc.window?.setFrame(NSRect(x: 40, y: 40, width: 1280, height: 800), display: true)
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
            AppE2E.pump(30)
            let counts = testTabAndPaneCounts()
            return counts.tabs >= minTabs && testLayoutLeafIDs().count >= minLeaves
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
