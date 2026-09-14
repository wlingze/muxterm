import Foundation

/// Core `WorkspaceId::replica_id()` 的 Swift 对照：Attention / Agents / Commands
/// 都按这个键聚合。Catalog `as_str()`（`ssh/ryzen/session/tmux/path`）不能拿来匹配。
public enum WorkspaceReplicaID {
    public static func from(session: String?, path: String, transport: String) -> String {
        let trimmedPath = path.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedSession = session?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let name = trimmedSession.isEmpty
            ? QuickConnect.defaultName(for: trimmedPath)
            : trimmedSession
        let transport = transport.trimmingCharacters(in: .whitespacesAndNewlines)
        let transportLabel = transport.isEmpty ? "local" : transport
        if !trimmedSession.isEmpty, !trimmedPath.isEmpty {
            return "\(name):\(trimmedPath)@\(transportLabel)"
        }
        return "\(name)@\(transportLabel)"
    }

    public static func from(_ config: TargetConfig) -> String {
        from(session: config.session, path: config.path, transport: config.transport.label)
    }
}
