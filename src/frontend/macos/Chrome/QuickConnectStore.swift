import Foundation

/// QuickConnect 数据（Recent + Project）的列表管理（纯逻辑，便于单测）。
///
/// - Recent：最近连接过的目标（最多 N 条），去重，最近的在最前；由连接池
///   在运行时派生，不落盘。
/// - Project：用户配置的预设目标，保存在统一 `config.toml` 的 `[[projects]]`。
///   本类不再编码 TOML；持久化通过注入的 `persistProjects` 闭包交给调用方
///   （macOS App 使用 CoreBridge 事务写 Core）。
public final class QuickConnectStore {
    /// 最近连接记录条数上限。
    public static let maxRecent = 20

    public private(set) var recents: [TargetConfig]
    public private(set) var projects: [TargetConfig]

    /// 项目变更时的持久化回调；nil 表示不落盘（纯内存/测试）。
    private let persistProjects: (([TargetConfig]) -> Void)?

    /// 遗留 `quickconnect.toml` 文件注入点（测试/迁移用）。
    private let fileURL: URL?

    /// Core-backed store：初始 projects 来自 `configDescribeJSON` 快照，变更
    /// 通过 `persistProjects` 写回统一配置。
    public init(
        projects initial: [TargetConfig] = [],
        persistProjects: @escaping ([TargetConfig]) -> Void
    ) {
        self.recents = []
        self.projects = initial
        self.persistProjects = persistProjects
        self.fileURL = nil
    }

    /// 纯内存 store（测试用）。
    public init(fileURL: URL? = nil) {
        self.fileURL = fileURL
        self.persistProjects = nil
        self.recents = []
        self.projects = []
        if let fileURL {
            load(from: fileURL)
        }
    }

    /// 记录一次连接：把目标放进 recents 最前，并按唯一 ID（name+transport）去重。
    /// 仅内存态；recent 不落盘（连接池才是持久来源）。
    public func recordRecent(_ config: TargetConfig) {
        let id = QuickConnect.uniqueID(for: config)
        recents.removeAll { QuickConnect.uniqueID(for: $0) == id }
        recents.insert(config, at: 0)
        if recents.count > Self.maxRecent {
            recents.removeLast(recents.count - Self.maxRecent)
        }
    }

    /// 用连接池派生的 recents 替换内存态（不触发落盘）。
    public func replaceRecents(_ newRecents: [TargetConfig]) {
        recents = Array(newRecents.prefix(Self.maxRecent))
    }

    /// 用连接池的完整 Workspace 快照替换内存态（不触发落盘）。
    /// 连接池的容量是软提醒阈值，可能合法地超过 20；快速面板搜索必须
    /// 能命中第 21 个及之后的 Workspace，因此这里不套用手动 Recent 历史上限。
    public func replaceAllRecents(_ newRecents: [TargetConfig]) {
        recents = newRecents
    }

    /// 新增或更新一个 project。返回是否新增。
    ///
    /// Herdr Project 第一次保存时还没有 workspace identity；连接成功后 Core 会返回
    /// canonical identity，此时用它替换同一个 Project 的占位记录，避免列表出现两条。
    @discardableResult
    public func upsertProject(_ config: TargetConfig) -> Bool {
        let id = QuickConnect.uniqueID(for: config)
        if let idx = projects.firstIndex(where: {
            QuickConnect.uniqueID(for: $0) == id
                || Self.isMatchingHerdrProvisional($0, resolved: config)
        }) {
            projects[idx] = config
            persistProjects?(projects)
            return false
        }
        projects.append(config)
        persistProjects?(projects)
        return true
    }

    /// Replace an existing project while allowing its attach identity to change.
    ///
    /// Editing a shell path or a tmux session changes `uniqueID`; matching the
    /// updated value with `upsertProject` would append a second project instead
    /// of updating the row the user edited.
    @discardableResult
    public func updateProject(_ config: TargetConfig, replacing original: TargetConfig) -> Bool {
        let originalID = QuickConnect.uniqueID(for: original)
        guard let index = projects.firstIndex(where: {
            QuickConnect.uniqueID(for: $0) == originalID
        }) else {
            return upsertProject(config)
        }
        projects[index] = config
        persistProjects?(projects)
        return true
    }

    private static func isMatchingHerdrProvisional(
        _ candidate: TargetConfig,
        resolved config: TargetConfig
    ) -> Bool {
        guard hasResolvedHerdrIdentity(config),
              candidate.runtime == .herdr,
              !hasResolvedHerdrIdentity(candidate)
        else {
            return false
        }

        return candidate.name == config.name
            && candidate.transport == config.transport
            && candidate.path == config.path
    }

    private static func hasResolvedHerdrIdentity(_ config: TargetConfig) -> Bool {
        config.runtime == .herdr
            && !(config.session?.isEmpty ?? true)
            && !(config.socket?.isEmpty ?? true)
            && !(config.workspaceID?.isEmpty ?? true)
    }

    /// 删除 project（按唯一 ID name+transport）。
    public func removeProject(config: TargetConfig) {
        let id = QuickConnect.uniqueID(for: config)
        projects.removeAll { QuickConnect.uniqueID(for: $0) == id }
        persistProjects?(projects)
    }

    /// 清空 recent（recent 不落盘；项目原样保留）。
    public func clearRecents() {
        recents.removeAll()
    }

    /// 序列化（TOML）：只写 projects，recent 不持久化。
    public func encode() -> Data {
        var out = "# Muxterm QuickConnect 配置（TOML）\n"
        out += "# 只保存 projects；recents 由连接池在运行时派生，不落盘。\n\n"
        out += encodeSection("projects", projects)
        return Data(out.utf8)
    }

    /// 从 TOML 解析并替换当前状态；非法/未知条目跳过，合法条目保留。
    /// recents 段落兼容读取（旧版文件），但不再写入。
    public func decode(_ data: Data) {
        guard let text = String(data: data, encoding: .utf8) else { return }
        var section: String?
        var fields: [String: String] = [:]
        var recentsBuf: [TargetConfig] = []
        var projectsBuf: [TargetConfig] = []

        func flush() {
            if let cfg = Self.config(from: fields) {
                switch section {
                case "recents": recentsBuf.append(cfg)
                case "projects": projectsBuf.append(cfg)
                default: break
                }
            }
            fields = [:]
        }

        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") {
                continue
            }
            if line.hasPrefix("[[") && line.hasSuffix("]]") {
                flush()
                let name = String(line.dropFirst(2).dropLast(2))
                section = (name == "recents" || name == "projects") ? name : nil
                continue
            }
            guard section != nil, let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            let value = String(line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces))
            guard let parsed = Self.parseTomlString(value) else { continue }
            fields[String(key)] = parsed
        }
        flush()
        if !recentsBuf.isEmpty {
            recents = recentsBuf
        }
        projects = projectsBuf
    }

    // MARK: - 持久化

    private func load(from url: URL) {
        guard let data = try? Data(contentsOf: url) else { return }
        decode(data)
    }

    /// Core project 数组 ↔ TargetConfig 转换（与 Rust `ProjectDocument` 对齐）。
    public static func targetConfigs(from projects: [[String: Any]]) -> [TargetConfig] {
        projects.compactMap { project in
            guard let name = project["name"] as? String,
                  let path = project["path"] as? String,
                  let runtimeInfo = project["runtime"] as? [String: Any],
                  let runtimeRaw = runtimeInfo["id"] as? String
            else { return nil }
            guard let runtime = TargetRuntime(rawValue: runtimeRaw) else { return nil }
            let transportInfo = project["transport"] as? [String: Any]
            let transportRaw = transportInfo?["id"] as? String ?? "local"
            let target = transportInfo?["target"] as? String ?? ""
            let transport: TargetTransport = transportRaw == "ssh" ? .ssh(name: target) : .local
            return TargetConfig(
                name: name,
                runtime: runtime,
                transport: transport,
                path: path,
                session: runtimeInfo["session"] as? String,
                socket: runtimeInfo["socket"] as? String,
                workspaceID: runtimeInfo["workspace_id"] as? String
            )
        }
    }

    /// TargetConfig 数组 → Core `[[projects]]` JSON（Rust `ProjectDocument` 形状）。
    public static func projectJSON(from projects: [TargetConfig]) -> [[String: Any]] {
        projects.map { project in
            let transport: [String: Any]
            switch project.transport {
            case .local:
                transport = ["id": "local", "target": ""]
            case .ssh(let name):
                transport = ["id": "ssh", "target": name]
            }
            var runtime: [String: Any] = ["id": project.runtime.rawValue]
            if let session = project.session {
                runtime["session"] = session
            }
            if let socket = project.socket {
                runtime["socket"] = socket
            }
            if let workspaceID = project.workspaceID {
                runtime["workspace_id"] = workspaceID
            }
            return [
                "id": QuickConnect.uniqueID(for: project),
                "name": project.name,
                "path": project.path,
                "runtime": runtime,
                "transport": transport,
                "command": [],
                "env": [:]
            ]
        }
    }

    // MARK: - TOML 编码

    private func encodeSection(_ name: String, _ items: [TargetConfig]) -> String {
        var out = ""
        for item in items {
            out += "[[\(name)]]\n"
            out += encodeConfig(item)
            out += "\n"
        }
        return out
    }

    private func encodeConfig(_ config: TargetConfig) -> String {
        var out = ""
        out += "name = \(Self.tomlString(config.name))\n"
        out += "runtime = \(Self.tomlString(config.runtime.rawValue))\n"
        switch config.transport {
        case .local:
            out += "transport = \"local\"\n"
        case .ssh(let name):
            out += "transport = \"ssh\"\n"
            out += "transport_name = \(Self.tomlString(name))\n"
        }
        out += "path = \(Self.tomlString(config.path))\n"
        if let session = config.session {
            out += "session = \(Self.tomlString(session))\n"
        }
        if let socket = config.socket {
            out += "socket = \(Self.tomlString(socket))\n"
        }
        if let workspaceID = config.workspaceID {
            out += "workspace_id = \(Self.tomlString(workspaceID))\n"
        }
        return out
    }

    private static func tomlString(_ s: String) -> String {
        var out = "\""
        for ch in s {
            switch ch {
            case "\\": out += "\\\\"
            case "\"": out += "\\\""
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default: out.append(ch)
            }
        }
        out += "\""
        return out
    }

    // MARK: - TOML 解码

    private static func parseTomlString(_ raw: String) -> String? {
        var s = raw
        guard s.hasPrefix("\""), s.hasSuffix("\"") else { return nil }
        s.removeFirst()
        s.removeLast()
        let chars = Array(s)
        var out = ""
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c == "\\" {
                i += 1
                guard i < chars.count else { return nil }
                switch chars[i] {
                case "\\": out.append("\\")
                case "\"": out.append("\"")
                case "n": out.append("\n")
                case "r": out.append("\r")
                case "t": out.append("\t")
                default: return nil
                }
            } else {
                out.append(c)
            }
            i += 1
        }
        return out
    }

    private static func config(from fields: [String: String]) -> TargetConfig? {
        guard let name = fields["name"],
              let runtimeRaw = fields["runtime"],
              let runtime = TargetRuntime(rawValue: runtimeRaw),
              let transportRaw = fields["transport"],
              let path = fields["path"]
        else {
            return nil
        }
        let transport: TargetTransport
        switch transportRaw {
        case "local":
            transport = .local
        case "ssh":
            guard let sshName = fields["transport_name"], !sshName.isEmpty else { return nil }
            transport = .ssh(name: sshName)
        default:
            return nil
        }
        return TargetConfig(
            name: name,
            runtime: runtime,
            transport: transport,
            path: path,
            session: fields["session"],
            socket: fields["socket"],
            workspaceID: fields["workspace_id"]
        )
    }
}
