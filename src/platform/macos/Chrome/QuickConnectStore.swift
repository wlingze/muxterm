import Foundation

/// QuickConnect 数据（Recent + Project）的持久化与列表管理（纯逻辑，便于单测）。
///
/// - Recent：最近连接过的目标（最多 N 条），去重，最近的在最前。
/// - Project：用户配置的预设目标。
/// 二者共用同一个 [`TargetConfig`] 结构与显示逻辑。
///
/// 落盘格式为 TOML（`~/.config/muxterm/quickconnect.toml`），**只保存
/// projects**；recents 由连接池（ConnectionPool）在运行时派生，不落盘。
/// ```toml
/// [[projects]]
/// name = "yaklang"
/// runtime = "tmux"
/// transport = "ssh"
/// transport_name = "ryzen"
/// path = "~/Developer/yaklang-workspace"
/// ...
/// ```
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

    private func persist() {
        guard let fileURL else { return }
        try? FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try? encode().write(to: fileURL, options: .atomic)
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
        return TargetConfig(name: name, runtime: runtime, transport: transport, path: path)
    }
}
