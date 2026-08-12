import Foundation

/// QuickConnect 数据（Recent + Project）的持久化与列表管理（纯逻辑，便于单测）。
///
/// - Recent：最近连接过的目标（最多 N 条），去重，最近的在最前。
/// - Project：用户配置的预设目标。
/// 二者共用同一个 [`TargetConfig`] 结构与显示逻辑。
public final class QuickConnectStore {
    /// 最近连接记录条数上限。
    public static let maxRecent = 20

    public private(set) var recents: [TargetConfig]
    public private(set) var projects: [TargetConfig]

    /// 文件 URL 注入点（测试用）；nil 时不落盘。
    private let fileURL: URL?

    public init(fileURL: URL? = nil) {
        self.fileURL = fileURL
        self.recents = []
        self.projects = []
        if let fileURL {
            load(from: fileURL)
        }
    }

    /// 记录一次连接：把目标放进 recents 最前，并按唯一 ID（name+transport）去重。
    public func recordRecent(_ config: TargetConfig) {
        let id = QuickConnect.uniqueID(for: config)
        recents.removeAll { QuickConnect.uniqueID(for: $0) == id }
        recents.insert(config, at: 0)
        if recents.count > Self.maxRecent {
            recents.removeLast(recents.count - Self.maxRecent)
        }
        persist()
    }

    /// 新增或更新一个 project（按唯一 ID name+transport 匹配）。返回是否新增。
    @discardableResult
    public func upsertProject(_ config: TargetConfig) -> Bool {
        let id = QuickConnect.uniqueID(for: config)
        if let idx = projects.firstIndex(where: { QuickConnect.uniqueID(for: $0) == id }) {
            projects[idx] = config
            persist()
            return false
        }
        projects.append(config)
        persist()
        return true
    }

    /// 删除 project（按唯一 ID name+transport）。
    public func removeProject(config: TargetConfig) {
        let id = QuickConnect.uniqueID(for: config)
        projects.removeAll { QuickConnect.uniqueID(for: $0) == id }
        persist()
    }

    /// 清空 recent。
    public func clearRecents() {
        recents.removeAll()
        persist()
    }

    /// 序列化（JSON），供测试与落盘复用。
    public func encode() -> Data {
        let payload = PersistedPayload(recents: recents, projects: projects)
        return (try? JSONEncoder().encode(payload)) ?? Data()
    }

    /// 从 Data 解析并替换当前状态。
    public func decode(_ data: Data) {
        guard let payload = try? JSONDecoder().decode(PersistedPayload.self, from: data) else {
            return
        }
        recents = payload.recents
        projects = payload.projects
    }

    // MARK: - 持久化

    private func load(from url: URL) {
        guard let data = try? Data(contentsOf: url) else { return }
        decode(data)
    }

    private func persist() {
        guard let fileURL else { return }
        try? FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try? encode().write(to: fileURL, options: .atomic)
    }
}

/// 落盘格式：recents + projects。
private struct PersistedPayload: Codable {
    var recents: [TargetConfig]
    var projects: [TargetConfig]
}

// MARK: - TargetConfig Codable（用于持久化）

extension TargetRuntime: Codable {}
extension TargetTransport: Codable {
    enum CodingKeys: String, CodingKey { case kind, name }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(String.self, forKey: .kind)
        if kind == "ssh" {
            let name = try c.decode(String.self, forKey: .name)
            self = .ssh(name: name)
        } else {
            self = .local
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .local:
            try c.encode("local", forKey: .kind)
        case .ssh(let name):
            try c.encode("ssh", forKey: .kind)
            try c.encode(name, forKey: .name)
        }
    }
}

extension TargetConfig: Codable {
    enum CodingKeys: String, CodingKey { case name, runtime, transport, path }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        runtime = try c.decode(TargetRuntime.self, forKey: .runtime)
        transport = try c.decode(TargetTransport.self, forKey: .transport)
        path = try c.decode(String.self, forKey: .path)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(name, forKey: .name)
        try c.encode(runtime, forKey: .runtime)
        try c.encode(transport, forKey: .transport)
        try c.encode(path, forKey: .path)
    }
}
