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
    /// Core's authoritative classification. A process can be called `node` by
    /// tmux but still be an agent when the resolved foreground argv is Codex.
    public let processIsAgent: Bool
    public let agentName: String?

    public init(
        paneId: UInt32,
        status: PaneAttentionStatus,
        acknowledged: Bool = false,
        lastLine: String,
        seq: UInt64,
        processName: String?,
        processIsAgent: Bool = false,
        agentName: String? = nil
    ) {
        self.paneId = paneId
        self.status = status
        self.acknowledged = acknowledged
        self.lastLine = lastLine
        self.seq = seq
        self.processName = processName
        self.processIsAgent = processIsAgent
        self.agentName = agentName
    }
}

/// 工作区聚合注意力视图。
public struct WorkspaceAttention: Equatable, Sendable {
    public let workspaceId: String
    public let name: String
    public let transport: String
    public let path: String
    public let blocked: Int
    public let done: Int
    public let working: Int
    public let panes: [PaneAttention]

    public init(
        workspaceId: String,
        name: String = "",
        transport: String = "",
        path: String = "~",
        blocked: Int,
        done: Int,
        working: Int,
        panes: [PaneAttention]
    ) {
        self.workspaceId = workspaceId
        self.name = name
        self.transport = transport
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
                            processName: p["process_name"] as? String,
                            processIsAgent: (p["process_is_agent"] as? Bool) ?? false,
                            agentName: p["agent_name"] as? String
                        )
                    } ?? []
                return WorkspaceAttention(
                    workspaceId: workspaceId,
                    name: (ws["name"] as? String) ?? "",
                    transport: (ws["transport"] as? String) ?? "",
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
    public let workspaceName: String
    public let agentName: String
    public let tabNumber: Int?

    public init(
        workspaceId: String,
        transport: String,
        path: String,
        pane: PaneAttention,
        workspaceName: String = "",
        agentName: String = "",
        tabNumber: Int? = nil
    ) {
        self.workspaceId = workspaceId
        self.transport = transport
        self.path = path
        self.pane = pane
        self.workspaceName = workspaceName
        self.agentName = agentName
        self.tabNumber = tabNumber
    }

    /// 第一行：工作区名，和侧栏 Agents / Workspaces 的 title 一致。
    public var title: String {
        let workspace = workspaceName.trimmingCharacters(in: .whitespacesAndNewlines)
        return workspace.isEmpty ? workspaceId : workspace
    }

    /// 第二行：状态 · agent · Tab N，和侧栏 Agents 的 detail 一致。
    public var detail: String {
        AttentionRowLabel.detail(
            status: pane.status,
            agentName: agentName,
            tabNumber: tabNumber
        )
    }

    /// 与侧栏 Agents/Commands 同一套色：运行绿、未读完成/阻塞橙、已读灰。
    public var indicator: AgentSidebarIndicator {
        switch pane.status {
        case .working:
            return .running
        case .blocked, .done:
            return pane.acknowledged ? .read : .done
        case .unknown, .idle:
            return .read
        }
    }
}

/// 注意力行标题：进程名 + transport + path，不用 last_line 片段。
public enum AttentionRowLabel {
    /// 将 pane-cmd/旧快照里的 wrapper 名称收敛成用户真正关心的 agent 名称。
    /// 例如 `npx @openai/codex`、`/opt/cursor-agent` 都应显示为 codex/cursor，
    /// 而不是路径或 node wrapper。未知命令只保留第一个可执行词的 basename，
    /// 避免把 `sleep 1 && echo output` 这样的命令行/输出混进标题。
    public static func normalizedProcess(_ process: String?) -> String? {
        guard let process else { return nil }
        let value = stripTerminalSequences(process)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        let known = [
            "codex", "cursor", "claude", "gemini", "aider", "opencode",
            "copilot", "cline", "goose", "amp", "grok", "windsurf", "kiro",
            "pi", "hermes", "droid",
        ]
        let commandTokens = value.split { character in
            character.isWhitespace || ";|&<>\"'`()".contains(character)
        }
        let identifierTokens = value.lowercased().split { character in
            !(character.isLetter || character.isNumber || character == "-" || character == "_")
        }
        for token in identifierTokens {
            let token = String(token)
            if let match = known.first(where: {
                token == $0 || token.hasPrefix($0 + "-") || token.hasPrefix($0 + "_")
            }) {
                return match
            }
        }
        guard let firstToken = commandTokens.first else { return nil }
        let basename = String(firstToken)
            .split(whereSeparator: { $0 == "/" || $0 == "\\" })
            .last
            .map(String.init) ?? value
        return basename
    }

    /// `process_name` 可能来自 pane-cmd 或带 ANSI 的 shell 状态行；标题只需要
    /// 可执行名，因此在分词前丢弃终端控制序列和不可见控制字符。
    private static func stripTerminalSequences(_ value: String) -> String {
        let scalars = Array(value.unicodeScalars)
        var result = String.UnicodeScalarView()
        var index = 0

        while index < scalars.count {
            let scalar = scalars[index].value
            guard scalar == 0x1B else {
                if scalar == 0x08 || scalar == 0x7F {
                    // Shell 输入重绘常见 `s\b sleep...`：退格表示前一个
                    // 字符被重画，不能把它当成命令的第一个词。
                    if !result.isEmpty {
                        result.removeLast()
                    }
                } else if scalar < 0x20 {
                    result.append(" ")
                } else {
                    result.append(scalars[index])
                }
                index += 1
                continue
            }

            // CSI: ESC [ ... final byte.
            index += 1
            guard index < scalars.count else { break }
            if scalars[index].value == 0x5B {
                index += 1
                while index < scalars.count {
                    let final = scalars[index].value
                    index += 1
                    if (0x40...0x7E).contains(final) { break }
                }
                continue
            }

            // OSC: ESC ] ... BEL or ESC \\.
            if scalars[index].value == 0x5D {
                index += 1
                while index < scalars.count {
                    let current = scalars[index].value
                    if current == 0x07 {
                        index += 1
                        break
                    }
                    if current == 0x1B,
                       index + 1 < scalars.count,
                       scalars[index + 1].value == 0x5C
                    {
                        index += 2
                        break
                    }
                    index += 1
                }
                continue
            }

            // 其它 ESC 序列通常只有一个 final byte；跳过它，避免把控制码
            // 参数误识别成进程名。
            index += 1
        }
        return String(result)
    }

    public static func display(process: String?, transport: String, path: String) -> String {
        let trimmed = normalizedProcess(process) ?? ""
        let name = trimmed.isEmpty ? "?" : trimmed
        return "\(name)  \(transport)  \(path)"
    }

    public static func statusText(_ status: PaneAttentionStatus) -> String {
        switch status {
        case .idle: return "Idle"
        case .working: return "Working"
        case .blocked: return "Blocked"
        case .done: return "Done"
        case .unknown: return "Unknown"
        }
    }

    public static func detail(
        status: PaneAttentionStatus,
        agentName: String,
        tabNumber: Int?
    ) -> String {
        var parts = [statusText(status)]
        let agent = agentName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !agent.isEmpty {
            parts.append(agent)
        }
        if let tabNumber, tabNumber > 0 {
            parts.append("Tab \(tabNumber)")
        }
        return parts.joined(separator: " · ")
    }

    public static func sidebarAligned(
        workspaceName: String,
        status: PaneAttentionStatus,
        agentName: String,
        tabNumber: Int?
    ) -> String {
        var parts: [String] = []
        let workspace = workspaceName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !workspace.isEmpty {
            parts.append(workspace)
        }
        parts.append(detail(status: status, agentName: agentName, tabNumber: tabNumber))
        return parts.joined(separator: " · ")
    }
}

/// 注意力列表纯逻辑：保留 running 与未读 done/blocked；已读完成项不再出现。
public enum AttentionList {
    public static func rows(
        from snapshot: AttentionSnapshot,
        workspaces: [WorkspaceSidebarItem] = [],
        query: String
    ) -> [AttentionRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        var rows: [AttentionRow] = []
        for ws in snapshot.workspaces {
            let chrome = workspaces.first { $0.workspaceId == ws.workspaceId }
            let transport = !ws.transport.isEmpty
                ? ws.transport
                : (chrome?.transport
                    ?? (ws.workspaceId.contains("@ssh") ? "ssh" : "local"))
            let workspaceName = firstNonempty([
                chrome?.name,
                ws.name,
                ws.workspaceId.split(separator: "@").first.map(String.init),
            ]) ?? ws.workspaceId
            for pane in ws.panes where pane.status.isListed
                && (pane.status == .working || !pane.acknowledged)
            {
                let agentName = firstNonempty([
                    chrome?.structuredAgents.first(where: { $0.paneId == pane.paneId }).flatMap {
                        firstNonempty([$0.displayName, $0.name, $0.kind, $0.title])
                    },
                    pane.agentName,
                    AttentionRowLabel.normalizedProcess(pane.processName),
                    pane.processName,
                ]) ?? ""
                let processText = agentName
                guard q.isEmpty
                    || ws.workspaceId.lowercased().contains(q)
                    || workspaceName.lowercased().contains(q)
                    || processText.lowercased().contains(q)
                    || (pane.processName ?? "").lowercased().contains(q)
                    || pane.lastLine.lowercased().contains(q)
                    || transport.lowercased().contains(q)
                else {
                    continue
                }
                rows.append(AttentionRow(
                    workspaceId: ws.workspaceId,
                    transport: transport,
                    path: chrome.map { "\($0.runtime) @ \($0.transport)" } ?? ws.path,
                    pane: pane,
                    workspaceName: workspaceName,
                    agentName: agentName,
                    tabNumber: chrome?.tabNumberByPane[pane.paneId]
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

    private static func firstNonempty(_ values: [String?]) -> String? {
        values.compactMap { value in
            guard let value else { return nil }
            let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }.first
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
