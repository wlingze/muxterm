import Foundation

/// 快速连接目标的运行时（由 Core Catalog 内置 Driver 映射）。
public enum TargetRuntime: String, Equatable, Sendable, CaseIterable {
    case shell
    case tmux
    case herdr

    public var label: String { rawValue }
}

/// 连接传输（ssh 需要 name；local 不需要）。
public enum TargetTransport: Equatable, Sendable {
    case local
    case ssh(name: String)

    public var label: String {
        switch self {
        case .local: return "local"
        case .ssh(let name): return name
        }
    }

    public var isSSH: Bool {
        if case .ssh = self { return true }
        return false
    }

    /// SSH attach 的 FFI 参数：Host 走 `sshAlias`，`socket` 只给真正的远端 `-L`。
    /// 禁止把 alias 塞进 socket（否则远端变成 `tmux -L ryzen`）。
    public var attachBackend: (type: String, socket: String?, sshAlias: String?) {
        switch self {
        case .local:
            return ("tmux", nil, nil)
        case .ssh(let name):
            return ("ssh", nil, name)
        }
    }
}

/// 快速连接条目上的小标记：标识该目标同时是 Recent 和/或 Project。
/// 一个目标可以同时命中两者（此时两个标记都显示）。
public enum QuickBadge: Equatable, Sendable, CaseIterable {
    case recent
    case project

    /// 标记的展示文本（本地化友好的短标签）。
    public var label: String {
        switch self {
        case .recent: return "Recent"
        case .project: return "Project"
        }
    }
}

/// 面板中的一行：目标 + 其应显示的标记（Recent / Project 可同时出现）。
public struct QuickConnectEntry: Equatable, Sendable {
    public let config: TargetConfig
    public let badges: [QuickBadge]

    public init(config: TargetConfig, badges: [QuickBadge]) {
        self.config = config
        self.badges = badges
    }
}

/// 一个可快速连接的目标（Recent / Project 共用）。
public struct TargetConfig: Equatable, Sendable {
    public var name: String
    public var runtime: TargetRuntime
    public var transport: TargetTransport
    /// 用户项目目录；Herdr 的 workspace id 必须独立保存在 `workspaceID`。
    public var path: String
    /// Runtime namespace：Herdr named session；tmux Existing 为 session 名。
    public var session: String?
    /// Target-side socket。SSH Herdr 保存远端路径，不保存本地临时 forward。
    public var socket: String?
    /// Herdr workspace id（`wN`）；tmux/shell 为 nil。
    public var workspaceID: String?

    public init(
        name: String,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: String,
        session: String? = nil,
        socket: String? = nil,
        workspaceID: String? = nil
    ) {
        self.name = name
        self.runtime = runtime
        self.transport = transport
        self.path = path
        self.session = session
        self.socket = socket
        self.workspaceID = workspaceID
    }
}

/// 快速连接目标的展示与派生逻辑（纯函数，便于单测）。
public enum QuickConnect {
    /// 从 path 派生默认 name：取路径最后一段目录名（最小目录）。
    /// 根目录 / 空路径回退到 "workspace"。
    public static func defaultName(for path: String) -> String {
        let trimmed = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return "workspace" }
        let last = URL(fileURLWithPath: trimmed).lastPathComponent
        // "/" 的 lastPathComponent 返回 "/"（根目录），也视为无最小目录。
        if last.isEmpty || last == "/" { return "workspace" }
        return last
    }

    /// 面板副标题（小字）：`runtime @ transport`。ssh transport 显示为名字。
    public static func subtitle(for config: TargetConfig) -> String {
        "\(config.runtime.label) @ \(config.transport.label)"
    }

    /// 判断该目标是否需要 tmux 按 name attach（tmux 且 name 非空）。
    public static func shouldAttach(existingName: String?, config: TargetConfig) -> Bool {
        config.runtime == .tmux && !config.name.isEmpty
    }

    /// 展示文本（搜索用）：name + 副标题 + path。
    public static func searchText(for config: TargetConfig) -> String {
        "\(config.name) \(subtitle(for: config)) \(config.path)".lowercased()
    }

    /// 目标的唯一 ID。
    ///
    /// tmux/shell 保持历史语义（`name @ transport`）。Herdr 解析完成后必须按
    /// Runtime 返回的 attach identity 去重，不能用展示名或项目路径冒充 workspace id；
    /// 创建前尚无 identity 时，暂时用 transport + path 作为 Project 占位键。
    public static func uniqueID(for config: TargetConfig) -> String {
        guard config.runtime == .herdr else {
            return "\(config.name)@\(config.transport.label)"
        }

        if let session = nonEmpty(config.session),
           let socket = nonEmpty(config.socket),
           let workspaceID = nonEmpty(config.workspaceID)
        {
            return scopedID(
                "herdr",
                transportIdentity(config.transport) + [session, socket, workspaceID]
            )
        }

        return scopedID(
            "herdr-project",
            transportIdentity(config.transport) + [config.path]
        )
    }

    /// Local 的 target 是空串；SSH target 是 Host alias。kind 与 target 分开
    /// 编码，避免名为 `local` 的 SSH alias 与本机 identity 碰撞。
    private static func transportIdentity(_ transport: TargetTransport) -> [String] {
        switch transport {
        case .local:
            return ["local", ""]
        case .ssh(let name):
            return ["ssh", name]
        }
    }

    /// 长度前缀避免路径、session 或 socket 中的分隔符造成 identity 碰撞。
    private static func scopedID(_ scope: String, _ components: [String]) -> String {
        let encoded = components.map { "\($0.utf8.count):\($0)" }.joined(separator: "|")
        return "\(scope)|\(encoded)"
    }

    private static func nonEmpty(_ value: String?) -> String? {
        guard let value, !value.isEmpty else { return nil }
        return value
    }

    /// 同一 canonical identity 从 Existing 与 Project 两条入口复用时，
    /// Existing 可能没有权威项目目录；仅用非空 Project 元数据补全展示字段，
    /// attach identity 始终保留 `resolved` 的值。
    public static func mergingProjectMetadata(
        resolved: TargetConfig,
        requested: TargetConfig
    ) -> TargetConfig {
        guard !requested.path.isEmpty else { return resolved }
        var merged = resolved
        merged.path = requested.path
        if !requested.name.isEmpty {
            merged.name = requested.name
        }
        return merged
    }

    /// 计算目标应显示哪些标记。
    /// - recents / projects: 现有记录（按唯一 ID 匹配）。
    /// 返回顺序固定：Recent 在前、Project 在后（若都有则都返回）。
    public static func badges(
        for config: TargetConfig,
        recents: [TargetConfig],
        projects: [TargetConfig]
    ) -> [QuickBadge] {
        let id = uniqueID(for: config)
        var result: [QuickBadge] = []
        if recents.contains(where: { uniqueID(for: $0) == id }) {
            result.append(.recent)
        }
        if projects.contains(where: { uniqueID(for: $0) == id }) {
            result.append(.project)
        }
        return result
    }

    /// 面板条目：先展示最近的前 `recentLimit` 条（最新在前），
    /// 再补 Project 中未出现的目标。按唯一 ID（name+transport）去重。
    public static func entries(
        recents: [TargetConfig],
        projects: [TargetConfig],
        recentLimit: Int = 5
    ) -> [QuickConnectEntry] {
        var seen = Set<String>()
        var result: [QuickConnectEntry] = []
        for config in recents.prefix(recentLimit) {
            let id = uniqueID(for: config)
            guard seen.insert(id).inserted else { continue }
            result.append(QuickConnectEntry(
                config: config,
                badges: badges(for: config, recents: recents, projects: projects)
            ))
        }
        for config in projects {
            let id = uniqueID(for: config)
            guard seen.insert(id).inserted else { continue }
            result.append(QuickConnectEntry(
                config: config,
                badges: badges(for: config, recents: recents, projects: projects)
            ))
        }
        return result
    }
}
