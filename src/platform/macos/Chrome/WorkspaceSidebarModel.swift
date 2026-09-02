import Foundation

/// Runtime-neutral agent lifecycle emitted by Core.
public enum StructuredAgentStatus: String, Decodable, Sendable, Equatable {
    case idle
    case working
    case blocked
    case done
    case unknown
}

/// A pane agent snapshot. Runtime-specific protocol fields never reach this model.
public struct StructuredPaneAgent: Decodable, Sendable, Equatable {
    public let paneId: UInt32
    public let displayName: String?
    public let title: String?
    public let name: String?
    public let kind: String?
    public let status: StructuredAgentStatus
    public let stateChangeSeq: UInt64
    public let revision: UInt64

    public init(
        paneId: UInt32,
        displayName: String?,
        title: String?,
        name: String?,
        kind: String?,
        status: StructuredAgentStatus,
        stateChangeSeq: UInt64 = 0,
        revision: UInt64 = 0
    ) {
        self.paneId = paneId
        self.displayName = displayName
        self.title = title
        self.name = name
        self.kind = kind
        self.status = status
        self.stateChangeSeq = stateChangeSeq
        self.revision = revision
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        paneId = try values.decode(UInt32.self, forKey: .paneId)
        displayName = try values.decodeIfPresent(String.self, forKey: .displayName)
        title = try values.decodeIfPresent(String.self, forKey: .title)
        name = try values.decodeIfPresent(String.self, forKey: .name)
        kind = try values.decodeIfPresent(String.self, forKey: .kind)
        status = try values.decodeIfPresent(StructuredAgentStatus.self, forKey: .status) ?? .unknown
        stateChangeSeq = try values.decodeIfPresent(UInt64.self, forKey: .stateChangeSeq) ?? 0
        revision = try values.decodeIfPresent(UInt64.self, forKey: .revision) ?? 0
    }

    enum CodingKeys: String, CodingKey {
        case paneId = "pane_id"
        case displayName = "display_name"
        case title, name, kind, status
        case stateChangeSeq = "state_change_seq"
        case revision
    }

    fileprivate func replacingStatus(_ status: StructuredAgentStatus) -> StructuredPaneAgent {
        StructuredPaneAgent(
            paneId: paneId,
            displayName: displayName,
            title: title,
            name: name,
            kind: kind,
            status: status,
            stateChangeSeq: stateChangeSeq,
            revision: revision
        )
    }
}

private struct StructuredAgentVersion: Comparable, Sendable {
    let stateChangeSeq: UInt64
    let revision: UInt64

    var isKnown: Bool {
        stateChangeSeq != 0 || revision != 0
    }

    func accepts(_ current: StructuredAgentVersion) -> Bool {
        guard current.isKnown else { return true }
        guard isKnown else { return false }
        return stateChangeSeq > current.stateChangeSeq
            || (stateChangeSeq == current.stateChangeSeq && revision >= current.revision)
    }

    static func < (lhs: StructuredAgentVersion, rhs: StructuredAgentVersion) -> Bool {
        if lhs.stateChangeSeq != rhs.stateChangeSeq {
            return lhs.stateChangeSeq < rhs.stateChangeSeq
        }
        return lhs.revision < rhs.revision
    }
}

/// Pane-scoped agent identity cache. Runtime snapshots may stop reporting an
/// agent after it exits, but the Agents sidebar keeps the last identity until
/// the pane itself closes. A missing runtime agent is marked unknown until a
/// newer authoritative status or attention snapshot arrives.
public struct StructuredAgentRegistry: Sendable {
    private var agents: [UInt32: StructuredPaneAgent] = [:]
    private var versions: [UInt32: StructuredAgentVersion] = [:]

    public init() {}

    public mutating func observe(paneId: UInt32, agent: StructuredPaneAgent?) {
        if let agent {
            let incoming = StructuredAgentVersion(
                stateChangeSeq: agent.stateChangeSeq,
                revision: agent.revision
            )
            let current = versions[paneId] ?? agents[paneId].map {
                StructuredAgentVersion(
                    stateChangeSeq: $0.stateChangeSeq,
                    revision: $0.revision
                )
            }
            if let current, !incoming.accepts(current) {
                return
            }
            agents[paneId] = agent
            if incoming.isKnown {
                versions[paneId] = incoming
            }
        } else if let previous = agents[paneId] {
            // Runtime 的 agent authority 暂时缺失时不能猜成 idle；保留 identity，
            // 交给 attention snapshot 在可用时补充显示状态。
            agents[paneId] = previous.replacingStatus(.unknown)
        }
    }

    public mutating func removePane(_ paneId: UInt32) {
        agents.removeValue(forKey: paneId)
        versions.removeValue(forKey: paneId)
    }

    public var snapshot: [StructuredPaneAgent] {
        agents.values.sorted { $0.paneId < $1.paneId }
    }
}

/// One warm Workspace shown by the main-window sidebar.
public struct WorkspaceSidebarItem: Sendable, Equatable {
    public let workspaceId: String
    public let name: String
    public let runtime: String
    public let transport: String
    public let isActive: Bool
    public let shortcut: Int?
    public let structuredAgents: [StructuredPaneAgent]
    /// 以当前 Tab 排序生成的 1-based 编号；pane id 仍只用于内部跳转。
    public let tabNumberByPane: [UInt32: Int]
    /// Pane 所属的稳定 TabId；侧栏点击时直接复用，避免再次遍历 Core 拓扑。
    public let tabIdByPane: [UInt32: UInt32]

    public init(
        workspaceId: String,
        name: String,
        runtime: String,
        transport: String,
        isActive: Bool,
        shortcut: Int? = nil,
        structuredAgents: [StructuredPaneAgent] = [],
        tabNumberByPane: [UInt32: Int] = [:],
        tabIdByPane: [UInt32: UInt32] = [:]
    ) {
        self.workspaceId = workspaceId
        self.name = name
        self.runtime = runtime
        self.transport = transport
        self.isActive = isActive
        self.shortcut = shortcut
        self.structuredAgents = structuredAgents
        self.tabNumberByPane = tabNumberByPane
        self.tabIdByPane = tabIdByPane
    }
}

/// Three persistent sidebar colors: active, completed/unread, and read.
public enum AgentSidebarIndicator: Sendable, Equatable {
    case running
    case done
    case read
}

public struct AgentSidebarItem: Sendable, Equatable {
    public let workspaceId: String
    public let tabId: UInt32?
    public let paneId: UInt32
    public let title: String
    public let detail: String
    public let indicator: AgentSidebarIndicator
    /// The name used for sorting and for the second display line.
    public let agentName: String
    public let tabNumber: Int?

    public init(
        workspaceId: String,
        tabId: UInt32? = nil,
        paneId: UInt32,
        title: String,
        detail: String,
        indicator: AgentSidebarIndicator,
        agentName: String = "Agent",
        tabNumber: Int? = nil
    ) {
        self.workspaceId = workspaceId
        self.tabId = tabId
        self.paneId = paneId
        self.title = title
        self.detail = detail
        self.indicator = indicator
        self.agentName = agentName
        self.tabNumber = tabNumber
    }
}

/// A running or unread ordinary command. Agents remain in the Agents section.
public struct CommandSidebarItem: Sendable, Equatable {
    public let workspaceId: String
    public let tabId: UInt32?
    public let paneId: UInt32
    public let title: String
    public let detail: String
    public let indicator: AgentSidebarIndicator

    public init(
        workspaceId: String,
        tabId: UInt32? = nil,
        paneId: UInt32,
        title: String,
        detail: String,
        indicator: AgentSidebarIndicator
    ) {
        self.workspaceId = workspaceId
        self.tabId = tabId
        self.paneId = paneId
        self.title = title
        self.detail = detail
        self.indicator = indicator
    }
}

/// Pure projection shared by AppKit rendering and tests.
public enum WorkspaceSidebarProjection {
    private static let knownAgents = [
        "codex", "cursor", "claude", "gemini", "aider", "opencode",
        "copilot", "cline", "goose", "amp", "grok", "windsurf", "kiro",
        "pi", "hermes", "droid",
    ]

    public static func agents(
        workspaces: [WorkspaceSidebarItem],
        attention: AttentionSnapshot?
    ) -> [AgentSidebarItem] {
        var attentionByPane: [PaneKey: PaneAttention] = [:]
        for workspace in attention?.workspaces ?? [] {
            for pane in workspace.panes {
                attentionByPane[
                    PaneKey(workspaceId: workspace.workspaceId, paneId: pane.paneId)
                ] = pane
            }
        }
        var result: [AgentSidebarItem] = []

        for workspace in workspaces {
            var structuredPaneIDs = Set<UInt32>()
            for agent in workspace.structuredAgents {
                structuredPaneIDs.insert(agent.paneId)
                let attention = attentionByPane[
                    PaneKey(workspaceId: workspace.workspaceId, paneId: agent.paneId)
                ]
                let agentName = firstNonempty([
                    agent.displayName,
                    agent.name,
                    agent.kind,
                    agent.title,
                ]) ?? "Agent"
                let tabNumber = workspace.tabNumberByPane[agent.paneId]
                let tabId = workspace.tabIdByPane[agent.paneId]
                result.append(AgentSidebarItem(
                    workspaceId: workspace.workspaceId,
                    tabId: tabId,
                    paneId: agent.paneId,
                    title: workspace.name,
                    detail: agentDetail(
                        status: statusLabel(status: agent.status, attention: attention),
                        agentName: agentName,
                        tabNumber: tabNumber
                    ),
                    indicator: indicator(status: agent.status, attention: attention),
                    agentName: agentName,
                    tabNumber: tabNumber
                ))
            }

            let generic = attention?.workspaces
                .first(where: { $0.workspaceId == workspace.workspaceId })?
                .panes ?? []
            for pane in generic where !structuredPaneIDs.contains(pane.paneId) {
                let isAgent = pane.processIsAgent || knownAgentName(pane.processName) != nil
                guard isAgent else { continue }
                let name = knownAgentName(pane.processName)
                    ?? AttentionRowLabel.normalizedProcess(pane.processName)
                    ?? pane.processName
                    ?? "Agent"
                let tabNumber = workspace.tabNumberByPane[pane.paneId]
                let tabId = workspace.tabIdByPane[pane.paneId]
                result.append(AgentSidebarItem(
                    workspaceId: workspace.workspaceId,
                    tabId: tabId,
                    paneId: pane.paneId,
                    title: workspace.name,
                    detail: agentDetail(
                        status: statusLabel(status: pane.status),
                        agentName: name,
                        tabNumber: tabNumber
                    ),
                    indicator: indicator(attention: pane),
                    agentName: name,
                    tabNumber: tabNumber
                ))
            }
        }
        return result.sorted { lhs, rhs in
            let rank: (AgentSidebarIndicator) -> Int = { indicator in
                switch indicator {
                case .done: 0
                case .running: 1
                case .read: 2
                }
            }
            let lhsRank = rank(lhs.indicator)
            let rhsRank = rank(rhs.indicator)
            if lhsRank != rhsRank { return lhsRank < rhsRank }
            let nameOrder = lhs.agentName.localizedCaseInsensitiveCompare(rhs.agentName)
            if nameOrder != .orderedSame {
                return nameOrder == .orderedAscending
            }
            let workspaceOrder = lhs.title.localizedCaseInsensitiveCompare(rhs.title)
            if workspaceOrder != .orderedSame {
                return workspaceOrder == .orderedAscending
            }
            if lhs.tabNumber != rhs.tabNumber {
                return (lhs.tabNumber ?? Int.max) < (rhs.tabNumber ?? Int.max)
            }
            return lhs.paneId < rhs.paneId
        }
    }

    /// Project ordinary commands across warm workspaces.
    ///
    /// Agents (structured or known by process name) stay in Agents; commands are
    /// visible only while Running or while Blocked/Done remains unread.
    public static func commands(
        workspaces: [WorkspaceSidebarItem],
        attention: AttentionSnapshot?
    ) -> [CommandSidebarItem] {
        var agentPaneKeys = Set<PaneKey>()
        for workspace in workspaces {
            for agent in workspace.structuredAgents {
                agentPaneKeys.insert(PaneKey(workspaceId: workspace.workspaceId, paneId: agent.paneId))
            }
        }
        for workspace in workspaces {
            for pane in attention?.workspaces.first(where: {
                $0.workspaceId == workspace.workspaceId
            })?.panes ?? [] where pane.processIsAgent || knownAgentName(pane.processName) != nil {
                agentPaneKeys.insert(PaneKey(workspaceId: workspace.workspaceId, paneId: pane.paneId))
            }
        }

        var result: [CommandSidebarItem] = []
        for workspace in workspaces {
            guard let panes = attention?.workspaces
                .first(where: { $0.workspaceId == workspace.workspaceId })?
                .panes
            else { continue }
            for pane in panes {
                guard !agentPaneKeys.contains(
                    PaneKey(workspaceId: workspace.workspaceId, paneId: pane.paneId)
                ), let title = nonEmptyProcessName(pane.processName)
                else { continue }
                switch pane.status {
                case .working:
                    result.append(CommandSidebarItem(
                        workspaceId: workspace.workspaceId,
                        tabId: workspace.tabIdByPane[pane.paneId],
                        paneId: pane.paneId,
                        title: title,
                        detail: detail(workspace: workspace, paneId: pane.paneId),
                        indicator: .running
                    ))
                case .blocked, .done:
                    guard !pane.acknowledged else { continue }
                    result.append(CommandSidebarItem(
                        workspaceId: workspace.workspaceId,
                        tabId: workspace.tabIdByPane[pane.paneId],
                        paneId: pane.paneId,
                        title: title,
                        detail: detail(workspace: workspace, paneId: pane.paneId),
                        indicator: .done
                    ))
                case .unknown, .idle:
                    continue
                }
            }
        }
        return result
    }

    private static func nonEmptyProcessName(_ value: String?) -> String? {
        guard let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines),
              !trimmed.isEmpty
        else { return nil }
        return trimmed
    }

    private struct PaneKey: Hashable {
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

    private static func knownAgentName(_ process: String?) -> String? {
        guard let process else { return nil }
        let tokens = process.lowercased().split { character in
            !(character.isLetter || character.isNumber || character == "-" || character == "_")
        }
        for token in tokens {
            let token = String(token)
            if let agent = knownAgents.first(where: {
                token == $0 || token.hasPrefix($0 + "-") || token.hasPrefix($0 + "_")
            }) {
                return agent
            }
        }
        return nil
    }

    private static func detail(workspace: WorkspaceSidebarItem, paneId: UInt32) -> String {
        "\(workspace.name) · pane \(paneId)"
    }

    private static func agentDetail(
        status: String,
        agentName: String,
        tabNumber: Int?
    ) -> String {
        var values = [status, agentName]
        if let tabNumber, tabNumber > 0 {
            values.append("Tab \(tabNumber)")
        }
        return values.joined(separator: " · ")
    }

    private static func statusLabel(
        status: StructuredAgentStatus,
        attention: PaneAttention?
    ) -> String {
        if status == .unknown, let attention {
            return statusLabel(status: attention.status)
        }
        switch status {
        case .idle: return "Idle"
        case .working: return "Working"
        case .blocked: return "Blocked"
        case .done: return "Done"
        case .unknown: return "Unknown"
        }
    }

    private static func statusLabel(status: PaneAttentionStatus) -> String {
        switch status {
        case .idle: return "Idle"
        case .working: return "Working"
        case .blocked: return "Blocked"
        case .done: return "Done"
        case .unknown: return "Unknown"
        }
    }

    private static func indicator(
        status: StructuredAgentStatus,
        attention: PaneAttention?
    ) -> AgentSidebarIndicator {
        switch status {
        case .working:
            return .running
        case .blocked, .done:
            return attention?.acknowledged == true ? .read : .done
        case .idle:
            return .read
        case .unknown:
            return attention.map(indicator(attention:)) ?? .read
        }
    }

    private static func indicator(attention: PaneAttention) -> AgentSidebarIndicator {
        switch attention.status {
        case .working:
            return .running
        case .blocked, .done:
            return attention.acknowledged ? .read : .done
        case .unknown, .idle:
            return .read
        }
    }
}
