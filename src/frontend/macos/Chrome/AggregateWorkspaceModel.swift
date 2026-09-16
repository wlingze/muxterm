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
    public let paneId: UInt32

    public init(workspaceId: String, paneId: UInt32) {
        self.workspaceId = workspaceId
        self.paneId = paneId
    }
}

/// Agents 槽的一页；只指向源 Workspace/pane，不拥有或复制 Surface。
public struct AgentAggregateTab: Sendable, Equatable {
    public let displayId: UInt32
    public let workspaceId: String
    public let sourceTabId: UInt32?
    public let paneId: UInt32
    public let title: String

    public var key: AgentAggregateKey {
        AgentAggregateKey(workspaceId: workspaceId, paneId: paneId)
    }

    public init(
        displayId: UInt32,
        workspaceId: String,
        sourceTabId: UInt32?,
        paneId: UInt32,
        title: String
    ) {
        self.displayId = displayId
        self.workspaceId = workspaceId
        self.sourceTabId = sourceTabId
        self.paneId = paneId
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

    /// 聚合 Tab 的次序只由 Workspace/Tab/Pane 身份决定，状态变化不会让页面跳位。
    public static func agentTabs(
        agents: [AgentSidebarItem],
        workspaceOrder: [String]
    ) -> [AgentAggregateTab] {
        let rank = Dictionary(uniqueKeysWithValues: workspaceOrder.enumerated().map {
            ($0.element, $0.offset)
        })
        let sorted = agents.sorted { lhs, rhs in
            let lhsRank = rank[lhs.workspaceId] ?? Int.max
            let rhsRank = rank[rhs.workspaceId] ?? Int.max
            if lhsRank != rhsRank { return lhsRank < rhsRank }
            if lhs.tabNumber != rhs.tabNumber {
                return (lhs.tabNumber ?? Int.max) < (rhs.tabNumber ?? Int.max)
            }
            if lhs.paneId != rhs.paneId { return lhs.paneId < rhs.paneId }
            return lhs.agentName.localizedCaseInsensitiveCompare(rhs.agentName) == .orderedAscending
        }
        return sorted.enumerated().map { offset, agent in
            var components = [agent.title, agent.agentName]
            if let title = nonempty(agent.sessionTitle),
               !components.contains(where: { $0.caseInsensitiveCompare(title) == .orderedSame })
            {
                components.append(title)
            }
            return AgentAggregateTab(
                displayId: UInt32(offset + 1),
                workspaceId: agent.workspaceId,
                sourceTabId: agent.tabId,
                paneId: agent.paneId,
                title: components.joined(separator: " · ")
            )
        }
    }

    private static func nonempty(_ value: String?) -> String? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines),
              !value.isEmpty
        else { return nil }
        return value
    }
}
