import Foundation

/// 命令面板使用的 tmux session 摘要。
struct TmuxSessionInfo: Equatable {
    let name: String
    let windowCount: Int
    let attached: Bool
}

/// core 从用户 SSH config 解析出的 Host alias。
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
            return MuxtermI18n.shared.tr(.errorSshConfig)
        case .noSSHHosts:
            return MuxtermI18n.shared.tr(.errorNoSshHosts)
        }
    }
}

/// 本地 / 远程连接发现与新建 session。
///
/// 该类型只负责把 core 的 owned 结果转换成 AppKit 命令面板模型。
/// SSH config、ssh 进程、tmux 查询和 session 创建均由 Rust core 完成。
final class ConnectionDiscovery {
    func listLocalSessions(completion: @escaping (Result<[TmuxSessionInfo], Error>) -> Void) {
        runAsync({
            try CoreBridge.discoverTmuxSessions(backendType: "local")
                .map(Self.sessionInfo)
        }, completion: completion)
    }

    func listRemoteSessions(
        host: SSHHostInfo,
        completion: @escaping (Result<[TmuxSessionInfo], Error>) -> Void
    ) {
        runAsync({
            try CoreBridge.discoverTmuxSessions(
                backendType: "ssh",
                target: host.alias
            ).map(Self.sessionInfo)
        }, completion: completion)
    }

    /// 列出当前用户已有 SSH 配置中的 alias。
    func sshHosts() -> Result<[SSHHostInfo], Error> {
        do {
            let hosts = try CoreBridge.discoverSSHHosts()
            let mapped = hosts.map {
                SSHHostInfo(
                    alias: $0.alias,
                    hostname: $0.hostname,
                    user: $0.user.isEmpty ? nil : $0.user,
                    port: $0.port == 22 ? nil : Int($0.port)
                )
            }
            return mapped.isEmpty
                ? .failure(ConnectionDiscoveryError.noSSHHosts)
                : .success(mapped)
        } catch {
            return .failure(error)
        }
    }

    func createSession(
        target: ConnectionTarget,
        directory: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
        let sessionName = Self.makeSessionName(directory: directory)
        let backend: String
        let alias: String?
        switch target {
        case .local:
            backend = "local"
            alias = nil
        case .ssh(let host):
            backend = "ssh"
            alias = host.alias
        }

        runAsync({
            try CoreBridge.createTmuxSession(
                backendType: backend,
                target: alias,
                session: sessionName,
                directory: directory
            )
        }, completion: completion)
    }

    // MARK: - Core result helpers

    private static func sessionInfo(_ session: CoreTmuxSession) -> TmuxSessionInfo {
        TmuxSessionInfo(
            name: session.name,
            windowCount: Int(session.windows),
            attached: session.attached
        )
    }

    private func runAsync<T>(
        _ operation: @escaping () throws -> T,
        completion: @escaping (Result<T, Error>) -> Void
    ) {
        DispatchQueue.global(qos: .userInitiated).async {
            let result: Result<T, Error>
            do {
                result = .success(try operation())
            } catch {
                result = .failure(error)
            }
            DispatchQueue.main.async {
                completion(result)
            }
        }
    }

    private static func makeSessionName(directory: String) -> String {
        let base = URL(fileURLWithPath: directory)
            .lastPathComponent
            .replacingOccurrences(of: "[^A-Za-z0-9_-]", with: "-", options: .regularExpression)
        let stem = base.isEmpty ? "workspace" : base
        let suffix = String(UUID().uuidString.prefix(6)).lowercased()
        return "muxterm-\(stem)-\(suffix)"
    }
}
