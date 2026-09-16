import Foundation

/// 前端固定聚合槽的稳定身份。它们只用于 GUI 选择，不会传给 Core。
public enum AggregateWorkspaceIdentity {
    public static let shells = "muxterm.frontend.aggregate.shells"
    public static let agents = "muxterm.frontend.aggregate.agents"
}

public struct AggregateSourceTab: Sendable, Equatable {
    public let id: UInt32
    public let title: String

    public init(id: UInt32, title: String) {
        self.id = id
        self.title = title
    }
}

/// 一个真实 shell Workspace 的只读投影输入。
public struct ShellAggregateSource: Sendable, Equatable {
    public let workspaceId: String
    public let transport: String
    public let openedOrder: UInt64
    public let tabs: [AggregateSourceTab]

    public init(
        workspaceId: String,
        transport: String,
        openedOrder: UInt64,
        tabs: [AggregateSourceTab]
    ) {
        self.workspaceId = workspaceId
        self.transport = transport
        self.openedOrder = openedOrder
        self.tabs = tabs
    }
}

/// Shells 槽的一页；sourceTabId 仍属于对应的真实 shell Workspace。
public struct ShellAggregateTab: Sendable, Equatable {
    public let displayId: UInt32
    public let workspaceId: String
    public let sourceTabId: UInt32
    public let title: String
    public let isLocal: Bool

    public init(
        displayId: UInt32,
        workspaceId: String,
        sourceTabId: UInt32,
        title: String,
        isLocal: Bool
    ) {
        self.displayId = displayId
        self.workspaceId = workspaceId
        self.sourceTabId = sourceTabId
        self.title = title
        self.isLocal = isLocal
    }
}

public struct AgentAggregateKey: Hashable, Sendable {
    public let workspaceId: String
    public let sourceTabId: UInt32

    public init(workspaceId: String, sourceTabId: UInt32) {
        self.workspaceId = workspaceId
        self.sourceTabId = sourceTabId
    }
}

/// Agents 槽的一页；只指向包含 agent 的真实源 Tab，不拥有或复制 Surface。
public struct AgentAggregateTab: Sendable, Equatable {
    public let displayId: UInt32
    public let workspaceId: String
    public let sourceTabId: UInt32
    public let agentPaneIds: [UInt32]
    public let title: String

    public var key: AgentAggregateKey {
        AgentAggregateKey(workspaceId: workspaceId, sourceTabId: sourceTabId)
    }

    public init(
        displayId: UInt32,
        workspaceId: String,
        sourceTabId: UInt32,
        agentPaneIds: [UInt32],
        title: String
    ) {
        self.displayId = displayId
        self.workspaceId = workspaceId
        self.sourceTabId = sourceTabId
        self.agentPaneIds = agentPaneIds
        self.title = title
    }
}

public enum AggregateWorkspaceProjection {
    /// 本机页永远排在最前；机器之间沿用真实 Workspace 的固定打开顺序。
    public static func shellTabs(sources: [ShellAggregateSource]) -> [ShellAggregateTab] {
        let sorted = sources.sorted { lhs, rhs in
            let lhsLocal = lhs.transport == "local"
            let rhsLocal = rhs.transport == "local"
            if lhsLocal != rhsLocal { return lhsLocal }
            if lhs.openedOrder != rhs.openedOrder { return lhs.openedOrder < rhs.openedOrder }
            return lhs.workspaceId < rhs.workspaceId
        }
        var displayId: UInt32 = 1
        var result: [ShellAggregateTab] = []
        for source in sorted {
            let isLocal = source.transport == "local"
            for tab in source.tabs {
                result.append(ShellAggregateTab(
                    displayId: displayId,
                    workspaceId: source.workspaceId,
                    sourceTabId: tab.id,
                    title: "\(source.transport) · \(tab.title)",
                    isLocal: isLocal
                ))
                displayId &+= 1
            }
        }
        return result
    }

    /// 一个真实源 Tab 只投影一页，即使它同时包含 shell 与多个 agent pane。
    /// 次序只由 Workspace/Tab 身份决定，状态变化不会让页面跳位。
    public static func agentTabs(
        agents: [AgentSidebarItem],
        workspaceOrder: [String]
    ) -> [AgentAggregateTab] {
        let rank = Dictionary(uniqueKeysWithValues: workspaceOrder.enumerated().map {
            ($0.element, $0.offset)
        })
        let sorted = agents.compactMap { agent -> AgentSidebarItem? in
            agent.tabId == nil ? nil : agent
        }.sorted { lhs, rhs in
            let lhsRank = rank[lhs.workspaceId] ?? Int.max
            let rhsRank = rank[rhs.workspaceId] ?? Int.max
            if lhsRank != rhsRank { return lhsRank < rhsRank }
            if lhs.tabNumber != rhs.tabNumber {
                return (lhs.tabNumber ?? Int.max) < (rhs.tabNumber ?? Int.max)
            }
            if lhs.tabId != rhs.tabId {
                return (lhs.tabId ?? UInt32.max) < (rhs.tabId ?? UInt32.max)
            }
            if lhs.paneId != rhs.paneId { return lhs.paneId < rhs.paneId }
            return lhs.agentName.localizedCaseInsensitiveCompare(rhs.agentName) == .orderedAscending
        }

        var orderedKeys: [AgentAggregateKey] = []
        var agentsByTab: [AgentAggregateKey: [AgentSidebarItem]] = [:]
        for agent in sorted {
            guard let sourceTabId = agent.tabId else { continue }
            let key = AgentAggregateKey(
                workspaceId: agent.workspaceId,
                sourceTabId: sourceTabId
            )
            if agentsByTab[key] == nil {
                orderedKeys.append(key)
            }
            agentsByTab[key, default: []].append(agent)
        }

        return orderedKeys.enumerated().compactMap { offset, key in
            guard let tabAgents = agentsByTab[key], let first = tabAgents.first else {
                return nil
            }
            var components = [first.title]
            let names = uniqueNonempty(tabAgents.map(\.agentName))
            if !names.isEmpty {
                components.append(names.joined(separator: ", "))
            }
            let sessionTitles = uniqueNonempty(tabAgents.compactMap(\.sessionTitle)).filter { title in
                !components.contains(where: {
                    $0.caseInsensitiveCompare(title) == .orderedSame
                })
            }
            if !sessionTitles.isEmpty {
                components.append(sessionTitles.joined(separator: " / "))
            }
            return AgentAggregateTab(
                displayId: UInt32(offset + 1),
                workspaceId: key.workspaceId,
                sourceTabId: key.sourceTabId,
                agentPaneIds: tabAgents.map(\.paneId),
                title: components.joined(separator: " · ")
            )
        }
    }

    private static func uniqueNonempty(_ values: [String]) -> [String] {
        var seen = Set<String>()
        return values.compactMap { value in
            guard let value = nonempty(value) else { return nil }
            let key = value.lowercased()
            guard seen.insert(key).inserted else { return nil }
            return value
        }
    }

    private static func nonempty(_ value: String?) -> String? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines),
              !value.isEmpty
        else { return nil }
        return value
    }
}
