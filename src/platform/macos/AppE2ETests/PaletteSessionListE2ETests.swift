import AppKit
import XCTest
@testable import MuxtermAppLib

/// Cmd-Shift-P → Local/SSH → session 列表必须从「新建」刷出真实名字。
final class PaletteSessionListE2ETests: XCTestCase {
    func testLocalSessionListRefreshesBeyondNewSession() throws {
        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "pal-sess")
        let extra = "extra-\(fx.session)"
        Tmux.ok(socket: fx.socket, args: ["new-session", "-d", "-s", extra, "/bin/cat"])

        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        XCTAssertTrue(app.testPaletteIsPresented(), "Cmd-Shift-P 必须打开命令面板")
        app.testSelectPaletteTitle("local")
        AppE2E.pump(40)
        XCTAssertTrue(
            app.testPaletteIsPresented(),
            "列出 session 时面板必须还在，不能一进 Local 就关掉"
        )

        let found = AppE2E.wait(timeout: 12) {
            AppE2E.pump(50)
            return app.testPaletteTitles().contains(where: { $0 == extra || $0.contains(fx.session) })
        }
        XCTAssertTrue(
            found,
            "隔离 socket 上的 session 必须出现在列表里，不能停在 New session。titles=\(app.testPaletteTitles())"
        )
        XCTAssertTrue(app.testPaletteIsPresented(), "刷新完成后面板仍应打开")
    }

    func testSshSessionListRefreshesWhenLoopbackAvailable() throws {
        try XCTSkipUnless(
            SshPaletteProbe.envAvailable,
            "需要 loopback sshd：eval \"$(./scripts/ci/setup-sshd.sh)\""
        )
        let probe = try SshPaletteProbe()
        defer { probe.cleanup() }

        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "pal-ssh")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        app.testSelectPaletteTitle("ssh")
        AppE2E.pump(80)
        XCTAssertTrue(app.testPaletteIsPresented(), "SSH host 列表必须打开")
        app.testSelectPaletteTitle(probe.alias)
        AppE2E.pump(40)

        let found = AppE2E.wait(timeout: 12) {
            AppE2E.pump(50)
            return app.testPaletteTitles().contains(where: { $0.contains(probe.session) })
        }
        XCTAssertTrue(
            found,
            "SSH 选 host 后必须刷出远端 session 名 \(probe.session)。titles=\(app.testPaletteTitles())"
        )
        XCTAssertTrue(app.testPaletteIsPresented(), "列表刷新后不得因异步失败把面板关掉")
    }
}

/// loopback sshd 默认 socket 上建一个远端 session（只 kill-session，不 kill-server）。
/// ssh config 走 `MUXTERM_SSH_CONFIG_PATH`，不改进程 HOME。
private final class SshPaletteProbe {
    let alias: String
    let session: String
    private let configPath: String
    private let homeDir: URL

    static var envAvailable: Bool {
        let env = ProcessInfo.processInfo.environment
        return env["MUXTERM_TEST_SSH_PORT"] != nil && env["MUXTERM_TEST_SSH_KEY"] != nil
    }

    init() throws {
        let env = ProcessInfo.processInfo.environment
        guard let portValue = env["MUXTERM_TEST_SSH_PORT"], let keyPath = env["MUXTERM_TEST_SSH_KEY"] else {
            throw NSError(
                domain: "SshPaletteProbe",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "MUXTERM_TEST_SSH_* 未设置"]
            )
        }
        let host = env["MUXTERM_TEST_SSH_HOST"] ?? "127.0.0.1"
        let user = env["MUXTERM_TEST_SSH_USER"] ?? NSUserName()
        let nanos = UInt64(Date().timeIntervalSince1970 * 1_000_000_000)
        homeDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-ssh-palette-\(nanos)")
        let sshDir = homeDir.appendingPathComponent(".ssh")
        try FileManager.default.createDirectory(at: sshDir, withIntermediateDirectories: true)
        alias = "palette-\(nanos % 100_000)"
        session = "muxterm-test-pal-\(nanos % 100_000)"
        configPath = sshDir.appendingPathComponent("config").path
        let config = """
        Host \(alias)
            HostName \(host)
            Port \(portValue)
            User \(user)
            IdentityFile \(keyPath)
            IdentitiesOnly yes
            BatchMode yes
            StrictHostKeyChecking no
            UserKnownHostsFile /dev/null
            LogLevel ERROR
        """
        try config.write(toFile: configPath, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: configPath
        )
        setenv("MUXTERM_SSH_CONFIG_PATH", configPath, 1)

        let created = ssh("tmux new-session -d -s \(session) /bin/cat")
        if created.status != 0 {
            unsetenv("MUXTERM_SSH_CONFIG_PATH")
            try? FileManager.default.removeItem(at: homeDir)
            throw XCTSkip("loopback sshd 不可达: \(created.stderr)")
        }
    }

    func cleanup() {
        _ = ssh("tmux kill-session -t \(session)")
        unsetenv("MUXTERM_SSH_CONFIG_PATH")
        try? FileManager.default.removeItem(at: homeDir)
    }

    @discardableResult
    private func ssh(_ remote: String) -> (status: Int32, stdout: String, stderr: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        proc.arguments = ["-F", configPath, alias, remote]
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
}
