import Foundation

/// Shared 1-based Workspace shortcut projection.
///
/// Sidebar and Quick Connect must use the same opened order. Only the first
/// nine entries have keyboard shortcuts; later workspaces remain reachable by
/// the panel without claiming another shortcut. Digit 0 always means last.
public enum WorkspaceShortcutIndex {
    public static let maximum = 9

    public static func byWorkspaceID(_ orderedIDs: [String]) -> [String: Int] {
        orderedIDs.prefix(maximum).enumerated().reduce(into: [:]) { result, item in
            let (offset, id) = item
            guard result[id] == nil else { return }
            result[id] = offset + 1
        }
    }
}

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

/// Pane-scoped agent identity cache, keyed by **workspace + pane**.
///
/// PaneId 在不同 Workspace 之间会重复（tmux `%0` / Herdr leaf 都从 0 起）。
/// 只按 paneId 缓存会把 legion 的 Codex 串到 muxterm 的 Tab1。
public struct StructuredAgentKey: Hashable, Sendable {
    public let workspaceId: String
    public let paneId: UInt32

    public init(workspaceId: String, paneId: UInt32) {
        self.workspaceId = workspaceId
        self.paneId = paneId
    }
}

/// Pane-scoped agent identity cache. Runtime snapshots may stop reporting an
/// agent after it exits, but the Agents sidebar keeps the last identity until
/// the pane itself closes. A missing runtime agent is marked unknown until a
/// newer authoritative status or attention snapshot arrives.
public struct StructuredAgentRegistry: Sendable {
    private var agents: [StructuredAgentKey: StructuredPaneAgent] = [:]
    private var versions: [StructuredAgentKey: StructuredAgentVersion] = [:]

    public init() {}

    public mutating func observe(workspaceId: String, paneId: UInt32, agent: StructuredPaneAgent?) {
        let key = StructuredAgentKey(workspaceId: workspaceId, paneId: paneId)
        if let agent {
            let incoming = StructuredAgentVersion(
                stateChangeSeq: agent.stateChangeSeq,
                revision: agent.revision
            )
            let current = versions[key] ?? agents[key].map {
                StructuredAgentVersion(
                    stateChangeSeq: $0.stateChangeSeq,
                    revision: $0.revision
                )
            }
            if let current, !incoming.accepts(current) {
                return
            }
            agents[key] = agent
            if incoming.isKnown {
                versions[key] = incoming
            }
        } else if let previous = agents[key] {
            // Runtime 的 agent authority 暂时缺失时不能猜成 idle；保留 identity，
            // 交给 attention snapshot 在可用时补充显示状态。
            agents[key] = previous.replacingStatus(.unknown)
        }
    }

    public mutating func removePane(workspaceId: String, paneId: UInt32) {
        let key = StructuredAgentKey(workspaceId: workspaceId, paneId: paneId)
        agents.removeValue(forKey: key)
        versions.removeValue(forKey: key)
    }

    public func snapshot(workspaceId: String) -> [StructuredPaneAgent] {
        agents
            .filter { $0.key.workspaceId == workspaceId }
            .map(\.value)
            .sorted { $0.paneId < $1.paneId }
    }

    /// 全量快照仅用于诊断；侧栏必须按 workspace 取。
    public var snapshot: [StructuredPaneAgent] {
        agents.values.sorted { lhs, rhs in
            if lhs.paneId != rhs.paneId {
                return lhs.paneId < rhs.paneId
            }
            return lhs.name ?? "" < (rhs.name ?? "")
        }
    }
}

/// Agents / Commands / Attention 共用机器标记，不另行推测 runtime 身份。
public enum WorkspaceDisplayTitle {
    public static func make(name: String, transport: String) -> String {
        let machine = transport.trimmingCharacters(in: .whitespacesAndNewlines)
        return machine.isEmpty ? name : "\(name) · \(machine)"
    }
}

/// One open Workspace shown by the main-window sidebar.
public struct WorkspaceSidebarItem: Sendable, Equatable {
    public let workspaceId: String
    public let name: String
    public let runtime: String
    public let transport: String
    public let isActive: Bool
    public let shortcut: Int?
    /// 固定聚合槽不能关闭或参与真实 Workspace 的拖动改序。
    public let isClosable: Bool
    public let isReorderable: Bool
    public let structuredAgents: [StructuredPaneAgent]
    /// 以当前 Tab 排序生成的 1-based 编号；pane id 仍只用于内部跳转。
    public let tabNumberByPane: [UInt32: Int]
    /// Pane 所属的稳定 TabId；侧栏点击时直接复用，避免再次遍历 Core 拓扑。
    public let tabIdByPane: [UInt32: UInt32]

    /// Core 提供的机器名与工作区名一起展示，不能仅藏在 tooltip。
    public var displayTitle: String {
        WorkspaceDisplayTitle.make(name: name, transport: transport)
    }

    /// Workspaces 列表与 Quick Panel 共用的切换标识。固定聚合槽使用
    /// 字母地标，真实 Workspace 才使用 Cmd-Ctrl-1...9 的编号。
    public var shortcutText: String? {
        switch workspaceId {
        case AggregateWorkspaceIdentity.shells:
            return "S"
        case AggregateWorkspaceIdentity.agents:
            return "A"
        default:
            return shortcut.map(String.init)
        }
    }

    public var isAggregate: Bool {
        workspaceId == AggregateWorkspaceIdentity.shells
            || workspaceId == AggregateWorkspaceIdentity.agents
    }

    public var keyboardShortcutText: String? {
        switch workspaceId {
        case AggregateWorkspaceIdentity.shells:
            return "Cmd-Ctrl-S"
        case AggregateWorkspaceIdentity.agents:
            return "Cmd-Ctrl-A"
        default:
            return shortcut.map { "Cmd-Ctrl-\($0)" }
        }
    }

    public init(
        workspaceId: String,
        name: String,
        runtime: String,
        transport: String,
        isActive: Bool,
        shortcut: Int? = nil,
        isClosable: Bool = true,
        isReorderable: Bool = true,
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
        self.isClosable = isClosable
        self.isReorderable = isReorderable
        self.structuredAgents = structuredAgents
        self.tabNumberByPane = tabNumberByPane
        self.tabIdByPane = tabIdByPane
    }
}

/// Herdr 同款四态：working / idle / blocked / done。
public enum AgentSidebarIndicator: Sendable, Equatable {
    case working
    case idle
    case blocked
    case done

    public var marker: String {
        self == .idle ? "○" : "●"
    }
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
    /// Runtime 提供的 agent/session title；与 agentName 分开，供聚合 Tab 展示。
    public let sessionTitle: String?
    public let tabNumber: Int?

    public init(
        workspaceId: String,
        tabId: UInt32? = nil,
        paneId: UInt32,
        title: String,
        detail: String,
        indicator: AgentSidebarIndicator,
        agentName: String = "Agent",
        sessionTitle: String? = nil,
        tabNumber: Int? = nil
    ) {
        self.workspaceId = workspaceId
        self.tabId = tabId
        self.paneId = paneId
        self.title = title
        self.detail = detail
        self.indicator = indicator
        self.agentName = agentName
        self.sessionTitle = sessionTitle
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

/// 侧栏跳转：paneId 跨 Workspace 会重复，fallback 必须限定在目标 workspace。
public enum PanelJumpRouting {
    public struct ScenePaneIndex: Sendable, Equatable {
        public let workspaceIds: Set<String>
        public let paneIds: Set<UInt32>
        public let isOpen: Bool

        public init(
            workspaceIds: Set<String>,
            paneIds: Set<UInt32>,
            isOpen: Bool = true
        ) {
            self.workspaceIds = workspaceIds
            self.paneIds = paneIds
            self.isOpen = isOpen
        }
    }

    /// replica id 暂时对不上时，仅当 pane 确实属于目标 workspace 才允许直接跳。
    public static func targetSceneContainsPane(
        workspaceId: String,
        paneId: UInt32,
        scenes: [ScenePaneIndex]
    ) -> ScenePaneIndex? {
        scenes.first { scene in
            scene.isOpen
                && scene.workspaceIds.contains(workspaceId)
                && scene.paneIds.contains(paneId)
        }
    }
}

/// Pure projection shared by AppKit rendering and tests.
public enum WorkspaceSidebarProjection {
    /// Agents 只投影 Core/runtime 已标记的数据：Herdr structured agent + attention
    /// 里 `processIsAgent == true` 的 pane。Frontend 不再根据 processName 猜 agent。
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
                    title: workspace.displayTitle,
                    detail: agentDetail(
                        status: statusLabel(status: agent.status, attention: attention),
                        agentName: agentName,
                        tabNumber: tabNumber
                    ),
                    indicator: indicator(status: agent.status, attention: attention),
                    agentName: agentName,
                    sessionTitle: agent.title,
                    tabNumber: tabNumber
                ))
            }

            let generic = attention?.workspaces
                .first(where: { $0.workspaceId == workspace.workspaceId })?
                .panes ?? []
            for pane in generic where !structuredPaneIDs.contains(pane.paneId) {
                guard pane.processIsAgent else { continue }
                let name = firstNonempty([
                    pane.agentName,
                    pane.processName,
                ]) ?? "Agent"
                let tabNumber = workspace.tabNumberByPane[pane.paneId]
                let tabId = workspace.tabIdByPane[pane.paneId]
                result.append(AgentSidebarItem(
                    workspaceId: workspace.workspaceId,
                    tabId: tabId,
                    paneId: pane.paneId,
                    title: workspace.displayTitle,
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
            // 状态成块；块内按 workspace 快捷键 / tab / pane 固定，只更新状态
            // 时条目在块间移动，块内相对顺序不变。
            let rank: (AgentSidebarIndicator) -> Int = { indicator in
                switch indicator {
                case .done: 0
                case .blocked: 1
                case .working: 2
                case .idle: 3
                }
            }
            let lhsRank = rank(lhs.indicator)
            let rhsRank = rank(rhs.indicator)
            if lhsRank != rhsRank { return lhsRank < rhsRank }
            let lhsShortcut = workspaces.first(where: { $0.workspaceId == lhs.workspaceId })?.shortcut
            let rhsShortcut = workspaces.first(where: { $0.workspaceId == rhs.workspaceId })?.shortcut
            if lhsShortcut != rhsShortcut {
                return (lhsShortcut ?? Int.max) < (rhsShortcut ?? Int.max)
            }
            let workspaceOrder = lhs.title.localizedCaseInsensitiveCompare(rhs.title)
            if workspaceOrder != .orderedSame {
                return workspaceOrder == .orderedAscending
            }
            if lhs.tabNumber != rhs.tabNumber {
                return (lhs.tabNumber ?? Int.max) < (rhs.tabNumber ?? Int.max)
            }
            if lhs.paneId != rhs.paneId {
                return lhs.paneId < rhs.paneId
            }
            let nameOrder = lhs.agentName.localizedCaseInsensitiveCompare(rhs.agentName)
            return nameOrder == .orderedAscending
        }
    }

    /// Project ordinary commands across open workspaces.
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
            })?.panes ?? [] where pane.processIsAgent {
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
                        title: workspace.displayTitle,
                        detail: agentDetail(status: statusLabel(status: pane.status),
                            agentName: title, tabNumber: workspace.tabNumberByPane[pane.paneId]),
                        indicator: .working
                    ))
                case .blocked, .done:
                    guard !pane.acknowledged else { continue }
                    result.append(CommandSidebarItem(
                        workspaceId: workspace.workspaceId,
                        tabId: workspace.tabIdByPane[pane.paneId],
                        paneId: pane.paneId,
                        title: workspace.displayTitle,
                        detail: agentDetail(status: statusLabel(status: pane.status),
                            agentName: title, tabNumber: workspace.tabNumberByPane[pane.paneId]),
                        indicator: pane.status == .blocked ? .blocked : .done
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
        if let attention {
            return statusLabel(status: attention.status)
        }
        switch status {
        case .idle: return "idle"
        case .working: return "working"
        case .blocked: return "blocked"
        case .done: return "done"
        case .unknown: return "idle"
        }
    }

    private static func statusLabel(status: PaneAttentionStatus) -> String {
        switch status {
        case .idle: return "idle"
        case .working: return "working"
        case .blocked: return "blocked"
        case .done: return "done"
        case .unknown: return "idle"
        }
    }

    private static func indicator(
        status: StructuredAgentStatus,
        attention: PaneAttention?
    ) -> AgentSidebarIndicator {
        if let attention {
            return indicator(attention: attention)
        }
        switch status {
        case .working:
            return .working
        case .blocked:
            return .blocked
        case .done:
            return .done
        case .idle, .unknown:
            return .idle
        }
    }

    private static func indicator(attention: PaneAttention) -> AgentSidebarIndicator {
        switch attention.status {
        case .working:
            return .working
        case .blocked:
            return .blocked
        case .done:
            return attention.acknowledged ? .idle : .done
        case .unknown, .idle:
            return .idle
        }
    }
}
