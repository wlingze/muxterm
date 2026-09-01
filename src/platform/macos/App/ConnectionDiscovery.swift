import Foundation
import MuxtermChrome

/// 命令面板使用的 tmux session 摘要。
struct TmuxSessionInfo: Equatable {
    let name: String
    let windowCount: Int
    let attached: Bool
}

/// Unified Quick Panel 中一条可直接 attach 的已有 Runtime workspace。
/// Herdr 的 named session/socket/workspace_id 与 tmux 的 session/socket 都在
/// `TargetConfig` 中固化，选择后不从当前 Workspace 反推。
struct ExistingConnectionChoice: Equatable {
    let config: TargetConfig
    let windowCount: Int?
    let attached: Bool?

    init(config: TargetConfig, windowCount: Int? = nil, attached: Bool? = nil) {
        self.config = config
        self.windowCount = windowCount
        self.attached = attached
    }

    /// 兼容 tmux 命令面板与既有测试的构造入口。
    init(target: ConnectionTarget, session: TmuxSessionInfo, socket: String?) {
        let transport: TargetTransport
        switch target {
        case .local:
            transport = .local
        case .ssh(let host):
            transport = .ssh(name: host.alias)
        }
        self.init(
            config: TargetConfig(
                name: session.name,
                runtime: .tmux,
                transport: transport,
                path: "",
                session: session.name,
                socket: socket
            ),
            windowCount: session.windowCount,
            attached: session.attached
        )
    }

    var target: ConnectionTarget {
        switch config.transport {
        case .local:
            return .local
        case .ssh(let alias):
            return .ssh(SSHHostInfo(alias: alias, hostname: "", user: nil, port: nil))
        }
    }

    var session: TmuxSessionInfo {
        TmuxSessionInfo(
            name: config.session ?? config.name,
            windowCount: windowCount ?? 0,
            attached: attached ?? false
        )
    }

    var socket: String? { config.socket }
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

    /// Workspaces 顶层的扁平 Existing 列表：Catalog 扇出 tmux + Herdr ×
    /// local/SSH。若当前 tmux 使用显式隔离 socket，再把该 socket 的候选合并
    /// 进来，不能因通用 discovery 回退到默认 server 而丢失当前连接。
    func listExistingConnections(
        currentTarget: ConnectionTarget,
        currentSocket: String?,
        completion: @escaping (Result<[ExistingConnectionChoice], Error>) -> Void
    ) {
        let generation = beginRequest()
        runAsync({
            var choices = try CoreBridge.discoverCatalogSessions()
                .compactMap(\.targetConfig)
                .map { ExistingConnectionChoice(config: $0) }

            if let currentSocket {
                let backend: String
                let alias: String?
                switch currentTarget {
                case .local:
                    backend = "local"
                    alias = nil
                case .ssh(let host):
                    backend = "ssh"
                    alias = host.alias
                }
                let sessions = try CoreBridge.discoverTmuxSessions(
                    backendType: backend,
                    target: alias,
                    socket: currentSocket,
                    configPath: self.sshConfigPath
                )
                choices.append(contentsOf: sessions.map {
                    ExistingConnectionChoice(
                        target: currentTarget,
                        session: Self.sessionInfo($0),
                        socket: currentSocket
                    )
                })
            }

            var seen = Set<String>()
            return choices.filter { choice in
                let config = choice.config
                let transportIdentity: String
                switch config.transport {
                case .local:
                    transportIdentity = "local"
                case .ssh(let alias):
                    transportIdentity = "ssh:\(alias)"
                }
                let key = [
                    transportIdentity,
                    config.runtime.rawValue,
                    config.session ?? "",
                    config.socket ?? "",
                    config.workspaceID ?? "",
                    config.name,
                ].joined(separator: "|")
                return seen.insert(key).inserted
            }
        }) { [weak self] result in
            guard let self, self.isCurrent(generation) else { return }
            completion(result)
        }
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

    /// 异步读取全部 SSH alias，供 Quick Panel 的 `@alias` 补全使用。
    /// 补全不应因为 SSH 配置解析或慢磁盘读取阻塞主线程。
    func listSSHAliases(completion: @escaping (Result<[String], Error>) -> Void) {
        let configPath = sshConfigPath
        runAsync({
            try CoreBridge.discoverSSHHosts(configPath: configPath).map(\.alias)
        }, completion: completion)
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
