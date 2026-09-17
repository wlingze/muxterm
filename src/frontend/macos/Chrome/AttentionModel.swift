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
        transport: String = "local",
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
                    transport: (ws["transport"] as? String) ?? "local",
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
    public let projectedAgent: AgentSidebarItem?

    public init(
        workspaceId: String,
        transport: String,
        path: String,
        pane: PaneAttention,
        workspaceName: String = "",
        agentName: String = "",
        tabNumber: Int? = nil,
        projectedAgent: AgentSidebarItem? = nil
    ) {
        self.workspaceId = workspaceId
        self.transport = transport
        self.path = path
        self.pane = pane
        self.workspaceName = workspaceName
        self.agentName = agentName
        self.tabNumber = tabNumber
        self.projectedAgent = projectedAgent
    }

    /// 第一行：工作区名 + 进程 + 机器/路径身份。
    public var title: String {
        let identity = AttentionRowLabel.display(
            process: pane.processName,
            transport: transport,
            path: path
        )
        return workspaceName.isEmpty ? identity : "\(workspaceName)  \(identity)"
    }

    public var detail: String {
        if let projectedAgent {
            return projectedAgent.detail
        }
        return AttentionRowLabel.detail(
            status: pane.status,
            agentName: agentName.isEmpty
                ? (AttentionRowLabel.normalizedProcess(pane.processName) ?? pane.processName ?? "")
                : agentName,
            tabNumber: tabNumber
        )
    }

    public var indicator: AgentSidebarIndicator {
        if let projectedAgent {
            return projectedAgent.indicator
        }
        switch pane.status {
        case .working:
            return .working
        case .blocked:
            return .blocked
        case .done:
            return pane.acknowledged ? .idle : .done
        case .idle, .unknown:
            return .idle
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
        let aliases = [
            "agent": "cursor",
            "cursor-agent": "cursor",
        ]
        let commandTokens = value.split { character in
            character.isWhitespace || ";|&<>\"'`()".contains(character)
        }
        // 先看 argv basename，避免路径中间的 cursor/codex 目录互相抢身份。
        for token in commandTokens {
            let raw = String(token).trimmingCharacters(in: CharacterSet(charactersIn: "'\""))
            guard !raw.isEmpty, !raw.hasPrefix("-") else { continue }
            let basename = raw.split(whereSeparator: { $0 == "/" || $0 == "\\" })
                .last
                .map(String.init) ?? raw
            let package = basename.split(separator: "/").last.map(String.init) ?? basename
            let lower = package.lowercased()
            if let alias = aliases[lower] {
                return alias
            }
            if let match = known.first(where: {
                lower == $0 || lower.hasPrefix($0 + "-") || lower.hasPrefix($0 + "_")
            }) {
                return match
            }
        }
        // 路径段从右往左：.../codex/.../cursor-agent/... 必须认 cursor。
        for token in commandTokens {
            let segments = String(token).split(whereSeparator: { $0 == "/" || $0 == "\\" })
                .map(String.init)
                .filter { !$0.isEmpty }
            for segment in segments.reversed() {
                let lower = segment.lowercased()
                if let alias = aliases[lower] {
                    return alias
                }
                if let match = known.first(where: { lower == $0 }) {
                    return match
                }
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

    public static func detail(
        status: PaneAttentionStatus,
        agentName: String,
        tabNumber: Int?
    ) -> String {
        let statusText: String
        switch status {
        case .working: statusText = "working"
        case .blocked: statusText = "blocked"
        case .done: statusText = "done"
        case .idle: statusText = "idle"
        case .unknown: statusText = "idle"
        }
        var values = [statusText]
        if !agentName.isEmpty {
            values.append(agentName)
        }
        if let tabNumber, tabNumber > 0 {
            values.append("Tab \(tabNumber)")
        }
        return values.joined(separator: " · ")
    }

    public static func sidebarAligned(
        workspaceName: String,
        status: PaneAttentionStatus,
        agentName: String,
        tabNumber: Int?
    ) -> String {
        let parts = [workspaceName, detail(status: status, agentName: agentName, tabNumber: tabNumber)]
        return parts.filter { !$0.isEmpty }.joined(separator: " · ")
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
        let projectedAgents = WorkspaceSidebarProjection.agents(
            workspaces: workspaces,
            attention: snapshot
        )
        let projectedByPane = Dictionary(
            projectedAgents.map { (PaneProjectionKey(workspaceId: $0.workspaceId, paneId: $0.paneId), $0) },
            uniquingKeysWith: { first, _ in first }
        )
        var rows: [AttentionRow] = []
        for ws in snapshot.workspaces {
            let chrome = workspaces.first { $0.workspaceId == ws.workspaceId }
            if chrome?.isAggregate == true || chrome?.isOpening == true {
                continue
            }
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
                let projectedAgent = projectedByPane[
                    PaneProjectionKey(workspaceId: ws.workspaceId, paneId: pane.paneId)
                ]
                let agentName = firstNonempty([
                    chrome?.structuredAgents.first(where: { $0.paneId == pane.paneId }).flatMap {
                        firstNonempty([$0.displayName, $0.name, $0.kind, $0.title])
                    },
                    pane.agentName,
                    AttentionRowLabel.normalizedProcess(pane.processName),
                    pane.processName,
                ]) ?? ""
                guard q.isEmpty
                    || ws.workspaceId.lowercased().contains(q)
                    || workspaceName.lowercased().contains(q)
                    || agentName.lowercased().contains(q)
                    || (pane.processName ?? "").lowercased().contains(q)
                    || pane.lastLine.lowercased().contains(q)
                    || transport.lowercased().contains(q)
                else {
                    continue
                }
                rows.append(AttentionRow(
                    workspaceId: ws.workspaceId,
                    transport: transport,
                    // Workspace chrome 可以补名称与 transport，但不能用
                    // `runtime @ transport` 副标题覆盖 Core 提供的真实路径。
                    path: ws.path,
                    pane: pane,
                    workspaceName: workspaceName,
                    agentName: agentName,
                    tabNumber: chrome?.tabNumberByPane[pane.paneId],
                    projectedAgent: projectedAgent
                ))
            }
        }
        rows.sort { a, b in
            func rank(_ indicator: AgentSidebarIndicator) -> Int {
                switch indicator {
                case .done: 0
                case .blocked: 1
                case .working: 2
                case .idle: 3
                }
            }
            let aRank = rank(a.indicator)
            let bRank = rank(b.indicator)
            if aRank != bRank {
                return aRank < bRank
            }
            return a.pane.seq > b.pane.seq
        }
        return rows
    }

    private struct PaneProjectionKey: Hashable {
        let workspaceId: String
        let paneId: UInt32
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
