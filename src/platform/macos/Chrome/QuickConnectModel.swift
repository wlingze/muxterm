import Foundation

/// 快速连接目标的运行时（shell / tmux）。
public enum TargetRuntime: String, Equatable, Sendable, CaseIterable {
    case shell
    case tmux

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
    public var path: String

    public init(name: String, runtime: TargetRuntime, transport: TargetTransport, path: String) {
        self.name = name
        self.runtime = runtime
        self.transport = transport
        self.path = path
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

    /// 目标的唯一 ID：`name @ transport`。
    /// 同一个机器上同名的目标视为同一个，Recent 与 Project 共用该 ID 去重。
    public static func uniqueID(for config: TargetConfig) -> String {
        "\(config.name)@\(config.transport.label)"
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
