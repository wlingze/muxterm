import Foundation

/// 命令面板使用的 tmux session 摘要。
struct TmuxSessionInfo: Equatable {
    let name: String
    let windowCount: Int
    let attached: Bool
}

/// Unified Quick Panel 中一条可直接 attach 的已有连接。
/// socket 在发现时固化，避免用户从隔离 `-L` server 选中后因当前 Workspace
/// 变化而误连默认 server。
struct ExistingConnectionChoice: Equatable {
    let target: ConnectionTarget
    let session: TmuxSessionInfo
    let socket: String?
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

/// 只有查询目标就是当前连接时才能复用其 tmux `-L` socket。
/// 本地 socket 绝不能泄漏到 SSH host，不同 SSH host 之间也不能串用。
enum ConnectionDiscoverySocketPolicy {
    static func socket(
        for target: ConnectionTarget,
        currentSSHHost: String?,
        currentSocket: String?
    ) -> String? {
        switch target {
        case .local:
            return currentSSHHost == nil ? currentSocket : nil
        case .ssh(let host):
            return currentSSHHost == host.alias ? currentSocket : nil
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
    /// 已 attach 的 tmux `-L` socket（本地）或远端 `-L`（SSH）。
    /// 传进 discover 才能列出隔离 socket 上的 session。
    var attachedLocalSocket: String?
    var attachedRemoteSocket: String?
    /// 测试/显式启动可指定 SSH config；普通应用启动保持 nil，继续用系统默认配置。
    private let sshConfigPath: String?
    /// 仅最后一次 session-list 请求可以更新 palette。 选择 target 后再
    /// 发起的请求不能被旧的 local/SSH completion 覆盖。
    private var requestGeneration: UInt64 = 0

    init(sshConfigPath: String? = ProcessInfo.processInfo.environment["MUXTERM_SSH_CONFIG_PATH"]) {
        self.sshConfigPath = sshConfigPath
    }

    func listLocalSessions(completion: @escaping (Result<[TmuxSessionInfo], Error>) -> Void) {
        let generation = beginRequest()
        let socket = attachedLocalSocket
        runAsync({
            try CoreBridge.discoverTmuxSessions(
                backendType: "local",
                socket: socket
            ).map(Self.sessionInfo)
        }) { [weak self] result in
            guard let self, self.isCurrent(generation) else { return }
            completion(result)
        }
    }

    func listRemoteSessions(
        host: SSHHostInfo,
        completion: @escaping (Result<[TmuxSessionInfo], Error>) -> Void
    ) {
        let generation = beginRequest()
        runAsync({
            try CoreBridge.discoverTmuxSessions(
                backendType: "ssh",
                target: host.alias,
                socket: self.attachedRemoteSocket,
                configPath: self.sshConfigPath
            ).map(Self.sessionInfo)
        }) { [weak self] result in
            guard let self, self.isCurrent(generation) else { return }
            completion(result)
        }
    }

    private func beginRequest() -> UInt64 {
        requestGeneration &+= 1
        return requestGeneration
    }

    private func isCurrent(_ generation: UInt64) -> Bool {
        requestGeneration == generation
    }

    /// 列出当前用户已有 SSH 配置中的 alias。
    func sshHosts() -> Result<[SSHHostInfo], Error> {
        do {
            let hosts = try CoreBridge.discoverSSHHosts(configPath: sshConfigPath)
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
        createSession(named: sessionName, target: target, directory: directory, completion: completion)
    }

    /// 以显式 session 名创建 detached tmux session（Project fallback 用，
    /// 与 twork 的 basename/显式 name 语义一致，不生成随机后缀）。
    func createSession(
        named session: String,
        target: ConnectionTarget,
        directory: String,
        completion: @escaping (Result<String, Error>) -> Void
    ) {
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
                configPath: self.sshConfigPath,
                session: session,
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
