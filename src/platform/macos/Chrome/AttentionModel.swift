import Foundation

/// 注意力 pane 状态（与 core `PaneStatus` 对齐）。
public enum PaneAttentionStatus: String, Sendable, Equatable {
    case unknown
    case working
    case done
    case blocked
    case idle

    /// 是否应出现在注意力列表（Blocked / Done）。
    public var isListed: Bool {
        self == .blocked || self == .done
    }
}

/// 单个 pane 的注意力条目。
public struct PaneAttention: Equatable, Sendable {
    public let paneId: UInt32
    public let status: PaneAttentionStatus
    public let lastLine: String
    public let seq: UInt64
    public let processName: String?

    public init(
        paneId: UInt32,
        status: PaneAttentionStatus,
        lastLine: String,
        seq: UInt64,
        processName: String?
    ) {
        self.paneId = paneId
        self.status = status
        self.lastLine = lastLine
        self.seq = seq
        self.processName = processName
    }
}

/// 工作区聚合注意力视图。
public struct WorkspaceAttention: Equatable, Sendable {
    public let workspaceId: String
    public let path: String
    public let blocked: Int
    public let done: Int
    public let working: Int
    public let panes: [PaneAttention]

    public init(
        workspaceId: String,
        path: String = "~",
        blocked: Int,
        done: Int,
        working: Int,
        panes: [PaneAttention]
    ) {
        self.workspaceId = workspaceId
        self.path = path
        self.blocked = blocked
        self.done = done
        self.working = working
        self.panes = panes
    }
}

/// 注意力快照（core `muxterm_attention_snapshot` JSON 的 Swift 视图）。
public struct AttentionSnapshot: Equatable, Sendable {
    public let blockedCount: Int
    public let workspaces: [WorkspaceAttention]

    public init(blockedCount: Int, workspaces: [WorkspaceAttention]) {
        self.blockedCount = blockedCount
        self.workspaces = workspaces
    }

    /// 解析 core JSON；失败返回 nil（不 panic）。
    public static func decode(_ data: Data) -> AttentionSnapshot? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let ok = json["ok"] as? Bool, ok
        else {
            return nil
        }
        let blockedCount = (json["blocked_count"] as? Int) ?? 0
        let workspaces: [WorkspaceAttention] = (json["workspaces"] as? [[String: Any]])?
            .compactMap { ws in
                guard let workspaceId = ws["workspace_id"] as? String else { return nil }
                let panes: [PaneAttention] = (ws["panes"] as? [[String: Any]])?
                    .compactMap { p in
                        guard let paneId = p["pane_id"] as? UInt32 ?? (p["pane_id"] as? NSNumber)?.uint32Value,
                              let statusRaw = p["status"] as? String,
                              let status = PaneAttentionStatus(rawValue: statusRaw)
                        else {
                            return nil
                        }
                        return PaneAttention(
                            paneId: paneId,
                            status: status,
                            lastLine: (p["last_line"] as? String) ?? "",
                            seq: (p["seq"] as? UInt64) ?? (p["seq"] as? NSNumber)?.uint64Value ?? 0,
                            processName: p["process_name"] as? String
                        )
                    } ?? []
                return WorkspaceAttention(
                    workspaceId: workspaceId,
                    path: (ws["path"] as? String) ?? "~",
                    blocked: (ws["blocked"] as? Int) ?? 0,
                    done: (ws["done"] as? Int) ?? 0,
                    working: (ws["working"] as? Int) ?? 0,
                    panes: panes
                )
            } ?? []
        return AttentionSnapshot(blockedCount: blockedCount, workspaces: workspaces)
    }
}

/// 注意力列表行（过滤 + 排序后的展示条目）。
public struct AttentionRow: Equatable, Sendable {
    public let workspaceId: String
    public let transport: String
    public let path: String
    public let pane: PaneAttention

    public init(workspaceId: String, transport: String, path: String, pane: PaneAttention) {
        self.workspaceId = workspaceId
        self.transport = transport
        self.path = path
        self.pane = pane
    }

    /// 行标题：进程名 + transport + path，不用 last_line 片段。
    public var title: String {
        AttentionRowLabel.display(
            process: pane.processName,
            transport: transport,
            path: path
        )
    }
}

/// 注意力行标题：进程名 + transport + path，不用 last_line 片段。
public enum AttentionRowLabel {
    public static func display(process: String?, transport: String, path: String) -> String {
        let trimmed = process?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let name = trimmed.isEmpty ? "?" : trimmed
        return "\(name)  \(transport)  \(path)"
    }
}

/// 注意力列表纯逻辑：只保留 Blocked/Done，blocked 先于 done，同状态按 seq 新者优先。
public enum AttentionList {
    public static func rows(from snapshot: AttentionSnapshot, query: String) -> [AttentionRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        var rows: [AttentionRow] = []
        for ws in snapshot.workspaces {
            let transport = ws.workspaceId.contains("@ssh")
                ? "ssh"
                : (ws.workspaceId.contains("@") ? "local" : "tmux")
            for pane in ws.panes where pane.status.isListed {
                guard q.isEmpty
                    || ws.workspaceId.lowercased().contains(q)
                    || (pane.processName ?? "").lowercased().contains(q)
                    || pane.lastLine.lowercased().contains(q)
                else {
                    continue
                }
                rows.append(AttentionRow(
                    workspaceId: ws.workspaceId,
                    transport: transport,
                    path: ws.path,
                    pane: pane
                ))
            }
        }
        rows.sort { a, b in
            let aBlocked = a.pane.status == .blocked
            let bBlocked = b.pane.status == .blocked
            if aBlocked != bBlocked {
                return aBlocked
            }
            return a.pane.seq > b.pane.seq
        }
        return rows
    }
}

/// 通知记录（core `muxterm_attention_take_notifications` JSON 的 Swift 视图）。
public struct AttentionNotifications: Equatable, Sendable {
    public let blocked: [String]
    public let done: [String]

    public init(blocked: [String], done: [String]) {
        self.blocked = blocked
        self.done = done
    }

    public static func decode(_ data: Data) -> AttentionNotifications? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let ok = json["ok"] as? Bool, ok
        else {
            return nil
        }
        return AttentionNotifications(
            blocked: (json["blocked"] as? [String]) ?? [],
            done: (json["done"] as? [String]) ?? []
        )
    }
}
