import Foundation

/// 命令面板使用的 tmux session 摘要。
struct TmuxSessionInfo: Equatable {
    let name: String
    let windowCount: Int
    let attached: Bool
}

/// `~/.ssh/config` 中可供选择的 Host alias。
struct SSHHostInfo: Equatable {
    let alias: String
    let hostname: String
    let user: String?
    let port: Int?
}

enum ConnectionTarget: Equatable {
    case local
    case ssh(SSHHostInfo)

    var displayName: String {
        switch self {
        case .local:
            return "local"
        case .ssh(let host):
            return host.alias
        }
    }
}

enum ConnectionDiscoveryError: Error, LocalizedError {
    case commandFailed(String)
    case sshConfigUnreadable
    case noSSHHosts

    var errorDescription: String? {
        switch self {
        case .commandFailed(let detail):
            return detail
        case .sshConfigUnreadable:
            return "无法读取 ~/.ssh/config"
        case .noSSHHosts:
            return "~/.ssh/config 中没有可用的 Host"
        }
    }
}

/// 本地 / 远程连接发现与新建 session。
///
/// 所有外部命令都在后台队列执行，避免命令面板查询 SSH 时阻塞 AppKit 主线程。
final class ConnectionDiscovery {
    private static let tmuxSessionFormat = "#{session_name}\t#{session_windows}\t#{session_attached}"

    func listLocalSessions(completion: @escaping (Result<[TmuxSessionInfo], Error>) -> Void) {
        runAsync(program: "/usr/bin/env", arguments: ["tmux", "list-sessions", "-F", Self.tmuxSessionFormat]) { result in
            completion(Self.parseSessions(result))
        }
    }

    func listRemoteSessions(
        host: SSHHostInfo,
        completion: @escaping (Result<[TmuxSessionInfo], Error>) -> Void
    ) {
        var arguments = [
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=5",
        ]
        arguments.append(host.alias)
        arguments.append(contentsOf: [
            "--", "tmux", "list-sessions", "-F", Self.shellQuote(Self.tmuxSessionFormat),
        ])
        runAsync(program: "/usr/bin/ssh", arguments: arguments) { result in
            completion(Self.parseSessions(result))
        }
    }

    func sshHosts() -> Result<[SSHHostInfo], Error> {
        let url = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".ssh")
            .appendingPathComponent("config")
        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            return .failure(ConnectionDiscoveryError.sshConfigUnreadable)
        }

        var entries: [String: SSHHostInfo] = [:]
        var aliases: [String] = []
        var hostname: String?
        var user: String?
        var port: Int?

        func flush() {
            guard !aliases.isEmpty else { return }
            for alias in aliases {
                entries[alias] = SSHHostInfo(
                    alias: alias,
                    hostname: hostname ?? alias,
                    user: user,
                    port: port
                )
            }
        }

        for rawLine in text.components(separatedBy: .newlines) {
            let line = rawLine.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !line.isEmpty, !line.hasPrefix("#") else { continue }
            let fields = line.split(maxSplits: 1, whereSeparator: { $0 == " " || $0 == "\t" })
            guard fields.count == 2 else { continue }
            let key = fields[0].lowercased()
            let rawValue = String(fields[1])
            let value = rawValue
                .trimmingCharacters(in: CharacterSet.whitespacesAndNewlines)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))

            if key == "host" {
                flush()
                aliases = value.split(whereSeparator: { $0 == " " || $0 == "\t" })
                    .map(String.init)
                    .filter { !$0.contains("*") && !$0.contains("?") && !$0.hasPrefix("!") }
                hostname = nil
                user = nil
                port = nil
            } else if !aliases.isEmpty {
                switch key {
                case "hostname": hostname = value
                case "user": user = value
                case "port": port = Int(value)
                default: break
                }
            }
        }
        flush()

        let hosts = entries.values.sorted { $0.alias.localizedCaseInsensitiveCompare($1.alias) == .orderedAscending }
        return hosts.isEmpty
            ? .failure(ConnectionDiscoveryError.noSSHHosts)
            : .success(hosts)
    }

    func createSession(
        target: ConnectionTarget,
        directory: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
        let sessionName = Self.makeSessionName(directory: directory)
        switch target {
        case .local:
            runAsync(
                program: "/usr/bin/env",
                arguments: ["tmux", "new-session", "-d", "-s", sessionName, "-c", directory]
            ) { result in
                completion(Self.requireSuccess(result, success: sessionName))
            }
        case .ssh(let host):
            let remoteCommand = [
                "tmux", "new-session", "-d",
                "-s", Self.shellQuote(sessionName),
                "-c", Self.shellQuote(directory),
            ].joined(separator: " ")
            let arguments = [
                "-o", "BatchMode=yes",
                "-o", "ConnectTimeout=10",
                host.alias,
                "--",
                remoteCommand,
            ]
            runAsync(program: "/usr/bin/ssh", arguments: arguments) { result in
                completion(Self.requireSuccess(result, success: sessionName))
            }
        }
    }

    // MARK: - Process helpers

    private struct CommandResult {
        let status: Int32
        let stdout: String
        let stderr: String
    }

    private func runAsync(
        program: String,
        arguments: [String],
        completion: @escaping (CommandResult) -> Void
    ) {
        DispatchQueue.global(qos: .userInitiated).async {
            let result = Self.run(program: program, arguments: arguments)
            DispatchQueue.main.async {
                completion(result)
            }
        }
    }

    private static func run(program: String, arguments: [String]) -> CommandResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: program)
        process.arguments = arguments

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return CommandResult(status: -1, stdout: "", stderr: error.localizedDescription)
        }

        let out = String(data: stdout.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let err = String(data: stderr.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return CommandResult(status: process.terminationStatus, stdout: out, stderr: err)
    }

    private static func parseSessions(_ result: CommandResult) -> Result<[TmuxSessionInfo], Error> {
        if result.status != 0 {
            let detail = result.stderr.lowercased()
            if detail.contains("no server running") || detail.contains("failed to connect") {
                return .success([])
            }
            return .failure(ConnectionDiscoveryError.commandFailed(result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)))
        }

        let sessions = result.stdout
            .components(separatedBy: .newlines)
            .compactMap { line -> TmuxSessionInfo? in
                let parts = line.split(separator: "\t", omittingEmptySubsequences: false)
                guard parts.count >= 3, !parts[0].isEmpty else { return nil }
                return TmuxSessionInfo(
                    name: String(parts[0]),
                    windowCount: Int(parts[1]) ?? 0,
                    attached: parts[2] != "0"
                )
            }
        return .success(sessions)
    }

    private static func requireSuccess(
        _ result: CommandResult,
        success: String
    ) -> Result<String, Error> {
        guard result.status == 0 else {
            let detail = result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            return .failure(ConnectionDiscoveryError.commandFailed(detail.isEmpty ? "命令执行失败" : detail))
        }
        return .success(success)
    }

    private static func makeSessionName(directory: String) -> String {
        let base = URL(fileURLWithPath: directory).lastPathComponent
            .replacingOccurrences(of: "[^A-Za-z0-9_-]", with: "-", options: .regularExpression)
        let stem = base.isEmpty ? "workspace" : base
        let suffix = String(UUID().uuidString.prefix(6)).lowercased()
        return "muxterm-\(stem)-\(suffix)"
    }

    private static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
