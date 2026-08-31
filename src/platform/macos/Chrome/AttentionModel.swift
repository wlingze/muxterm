import Foundation

/// 注意力 pane 状态（与 core `PaneStatus` 对齐）。
public enum PaneAttentionStatus: String, Sendable, Equatable {
    case unknown
    case working
    case done
    case blocked
    case idle

    /// 是否属于 Attention 可展示状态。已读过滤由 `PaneAttention` 决定。
    public var isListed: Bool {
        self == .working || self == .blocked || self == .done
    }
}

/// 单个 pane 的注意力条目。
public struct PaneAttention: Equatable, Sendable {
    public let paneId: UInt32
    public let status: PaneAttentionStatus
    public let acknowledged: Bool
    public let lastLine: String
    public let seq: UInt64
    public let processName: String?

    public init(
        paneId: UInt32,
        status: PaneAttentionStatus,
        acknowledged: Bool = false,
        lastLine: String,
        seq: UInt64,
        processName: String?
    ) {
        self.paneId = paneId
        self.status = status
        self.acknowledged = acknowledged
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
                            acknowledged: (p["acknowledged"] as? Bool) ?? false,
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
    /// 将 pane-cmd/旧快照里的 wrapper 名称收敛成用户真正关心的 agent 名称。
    /// 例如 `npx @openai/codex`、`/opt/cursor-agent` 都应显示为 codex/cursor，
    /// 而不是路径或 node wrapper。未知命令仍保留 basename。
    public static func normalizedProcess(_ process: String?) -> String? {
        guard let process else { return nil }
        let value = process.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        let known = [
            "codex", "cursor", "claude", "gemini", "aider", "opencode",
            "copilot", "cline", "goose", "amp", "grok", "windsurf", "kiro",
            "pi", "hermes",
        ]
        let tokens = value.lowercased().split { character in
            !(character.isLetter || character.isNumber || character == "-" || character == "_")
        }
        for token in tokens {
            let token = String(token)
            if let match = known.first(where: {
                token == $0 || token.hasPrefix($0 + "-") || token.hasPrefix($0 + "_")
            }) {
                return match
            }
        }
        let basename = value
            .split(whereSeparator: { $0 == "/" || $0 == "\\" })
            .last
            .map(String.init) ?? value
        return basename
    }

    public static func display(process: String?, transport: String, path: String) -> String {
        let trimmed = normalizedProcess(process) ?? ""
        let name = trimmed.isEmpty ? "?" : trimmed
        return "\(name)  \(transport)  \(path)"
    }
}

/// 注意力列表纯逻辑：保留 running 与未读 done/blocked；已读完成项不再出现。
public enum AttentionList {
    public static func rows(from snapshot: AttentionSnapshot, query: String) -> [AttentionRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        var rows: [AttentionRow] = []
        for ws in snapshot.workspaces {
            let transport = ws.workspaceId.contains("@ssh")
                ? "ssh"
                : (ws.workspaceId.contains("@") ? "local" : "tmux")
            for pane in ws.panes where pane.status.isListed
                && (pane.status == .working || !pane.acknowledged)
            {
                let processText = AttentionRowLabel.normalizedProcess(pane.processName)
                    ?? pane.processName
                    ?? ""
                guard q.isEmpty
                    || ws.workspaceId.lowercased().contains(q)
                    || processText.lowercased().contains(q)
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
            func rank(_ status: PaneAttentionStatus) -> Int {
                switch status {
                case .blocked: 0
                case .done: 1
                case .working: 2
                case .unknown, .idle: 3
                }
            }
            let aRank = rank(a.pane.status)
            let bRank = rank(b.pane.status)
            if aRank != bRank {
                return aRank < bRank
            }
            return a.pane.seq > b.pane.seq
        }
        return rows
    }
}

/// 单条结构化通知；新版 FFI 提供 pane、进程名和最后一行，旧版可缺失 pane。
public struct AttentionNotification: Equatable, Sendable {
    public let workspaceId: String
    public let paneId: UInt32?
    public let kind: PaneAttentionStatus
    public let processName: String?
    public let lastLine: String
    public let seq: UInt64

    public init(
        workspaceId: String,
        paneId: UInt32?,
        kind: PaneAttentionStatus,
        processName: String?,
        lastLine: String,
        seq: UInt64
    ) {
        self.workspaceId = workspaceId
        self.paneId = paneId
        self.kind = kind
        self.processName = processName
        self.lastLine = lastLine
        self.seq = seq
    }

    public var displayProcessName: String? {
        AttentionRowLabel.normalizedProcess(processName)
    }
}

/// 通知记录（core `muxterm_attention_take_notifications` JSON 的 Swift 视图）。
public struct AttentionNotifications: Equatable, Sendable {
    public let blocked: [String]
    public let done: [String]
    /// 新版结构化记录，包含 pane、进程名和最后一行；旧 core 只提供 workspace 数组。
    public let notifications: [AttentionNotification]

    public init(
        blocked: [String],
        done: [String],
        notifications: [AttentionNotification] = []
    ) {
        self.blocked = blocked
        self.done = done
        self.notifications = notifications
    }

    public static func decode(_ data: Data) -> AttentionNotifications? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let ok = json["ok"] as? Bool, ok
        else {
            return nil
        }
        let blocked = (json["blocked"] as? [String]) ?? []
        let done = (json["done"] as? [String]) ?? []
        let structured: [AttentionNotification] = (json["notifications"] as? [[String: Any]])?
            .compactMap { item in
                guard let workspaceId = item["workspace_id"] as? String,
                      let kindRaw = item["kind"] as? String,
                      let kind = PaneAttentionStatus(rawValue: kindRaw),
                      kind == .blocked || kind == .done
                else {
                    return nil
                }
                let paneId = (item["pane_id"] as? UInt32)
                    ?? (item["pane_id"] as? NSNumber)?.uint32Value
                let seq = (item["seq"] as? UInt64)
                    ?? (item["seq"] as? NSNumber)?.uint64Value
                    ?? 0
                return AttentionNotification(
                    workspaceId: workspaceId,
                    paneId: paneId,
                    kind: kind,
                    processName: item["process_name"] as? String,
                    lastLine: (item["last_line"] as? String) ?? "",
                    seq: seq
                )
            } ?? []
        let notifications: [AttentionNotification]
        if structured.isEmpty {
            // 与旧 FFI 的 workspace-only 响应兼容；paneId 缺失时前端只做系统通知，
            // 不会尝试 acknowledge 一个不确定的 pane。
            notifications = blocked.map {
                AttentionNotification(
                    workspaceId: $0,
                    paneId: nil,
                    kind: .blocked,
                    processName: nil,
                    lastLine: "",
                    seq: 0
                )
            } + done.map {
                AttentionNotification(
                    workspaceId: $0,
                    paneId: nil,
                    kind: .done,
                    processName: nil,
                    lastLine: "",
                    seq: 0
                )
            }
        } else {
            notifications = structured
        }
        return AttentionNotifications(
            blocked: blocked.isEmpty
                ? structured.filter { $0.kind == .blocked }.map(\.workspaceId)
                : blocked,
            done: done.isEmpty
                ? structured.filter { $0.kind == .done }.map(\.workspaceId)
                : done,
            notifications: notifications
        )
    }
}
