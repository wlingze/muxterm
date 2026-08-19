import AppKit
import XCTest
@testable import MuxtermAppLib

/// Loopback SSH + 远端隔离 tmux attach（对标 `tmux_ssh_feature_contract`）。
///
/// 需要 `eval "$(./scripts/ci/setup-sshd.sh)"`。无环境变量时跳过，不算绿。
final class SshAttachE2ETests: XCTestCase {
    func testSshCreateThenAttachShowsToken() throws {
        try XCTSkipUnless(
            SshPaintedSession.envAvailable,
            "需要 loopback sshd：eval \"$(./scripts/ci/setup-sshd.sh)\""
        )
        let fx = try SshPaintedSession(label: "mac-ssh")
        defer { fx.cleanup() }

        let app = try AppE2E.attachSshWindow(
            alias: fx.alias,
            remoteSocket: fx.remoteSocket,
            session: fx.session
        )
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 1), "SSH attach 后应有 pane")
        XCTAssertTrue(
            app.waitTerminalContains(fx.token, timeout: AppE2E.featureTimeout),
            "SSH attach 后 SwiftTerm 必须含播种 token \(fx.token)。got=\(app.testAllVisibleTerminalText())"
        )
        XCTAssertEqual(app.bridge.sshAlias, fx.alias, "sshAlias 必须是 Host 名")
        XCTAssertEqual(
            app.bridge.socket,
            fx.remoteSocket,
            "显式 sshAlias 存在时必须保留真正的远端 -L socket"
        )
    }
}

/// 本机 loopback sshd + 远端 `-L muxterm-test-*` 夹具。
private final class SshPaintedSession {
    let alias: String
    let remoteSocket: String
    let session: String
    let token: String
    private let configPath: String
    private let homeDir: URL

    static var envAvailable: Bool {
        let env = ProcessInfo.processInfo.environment
        return env["MUXTERM_TEST_SSH_PORT"] != nil && env["MUXTERM_TEST_SSH_KEY"] != nil
    }

    init(label: String) throws {
        let env = ProcessInfo.processInfo.environment
        guard let portValue = env["MUXTERM_TEST_SSH_PORT"], let keyPath = env["MUXTERM_TEST_SSH_KEY"] else {
            throw NSError(
                domain: "SshPaintedSession",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "MUXTERM_TEST_SSH_* 未设置"]
            )
        }
        let host = env["MUXTERM_TEST_SSH_HOST"] ?? "127.0.0.1"
        let user = env["MUXTERM_TEST_SSH_USER"] ?? NSUserName()

        let nanos = UInt64(Date().timeIntervalSince1970 * 1_000_000_000)
        homeDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-ssh-mac-\(label)-\(nanos)")
        let sshDir = homeDir.appendingPathComponent(".ssh")
        try FileManager.default.createDirectory(at: sshDir, withIntermediateDirectories: true)
        alias = "test-\(label)"
        remoteSocket = Tmux.uniqueSocket("remote-\(label)")
        session = "ssh-\(label)"
        token = "SSH_MAC_\(ProcessInfo.processInfo.processIdentifier)_\(nanos % 100_000)"
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

        let created = ssh(
            "tmux -L \(remoteSocket) -f /dev/null new-session -d -s \(session) -x 80 -y 24 -- /bin/cat"
        )
        XCTAssertEqual(created.status, 0, "远端 new-session 失败: \(created.stderr)")
        _ = ssh("tmux -L \(remoteSocket) send-keys -t \(session) -l \(token)")
        let deadline = Date().addingTimeInterval(3)
        var painted = false
        while Date() < deadline {
            let cap = ssh("tmux -L \(remoteSocket) capture-pane -p -t \(session)")
            if cap.stdout.contains(token) {
                painted = true
                break
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        XCTAssertTrue(painted, "attach 前远端 capture-pane 必须已有 \(token)")
    }

    func cleanup() {
        _ = ssh("tmux -L \(remoteSocket) kill-server")
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
