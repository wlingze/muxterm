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

    public init(
        paneId: UInt32,
        displayName: String?,
        title: String?,
        name: String?,
        kind: String?,
        status: StructuredAgentStatus
    ) {
        self.paneId = paneId
        self.displayName = displayName
        self.title = title
        self.name = name
        self.kind = kind
        self.status = status
    }

    enum CodingKeys: String, CodingKey {
        case paneId = "pane_id"
        case displayName = "display_name"
        case title, name, kind, status
    }

    fileprivate func replacingStatus(_ status: StructuredAgentStatus) -> StructuredPaneAgent {
        StructuredPaneAgent(
            paneId: paneId,
            displayName: displayName,
            title: title,
            name: name,
            kind: kind,
            status: status
        )
    }
}

/// Pane-scoped agent identity cache. Runtime snapshots may stop reporting an
/// agent after it exits, but the Agents sidebar keeps the last identity until
/// the pane itself closes. A missing runtime agent therefore becomes read.
public struct StructuredAgentRegistry: Sendable {
    private var agents: [UInt32: StructuredPaneAgent] = [:]

    public init() {}

    public mutating func observe(paneId: UInt32, agent: StructuredPaneAgent?) {
        if let agent {
            agents[paneId] = agent
        } else if let previous = agents[paneId] {
            agents[paneId] = previous.replacingStatus(.idle)
        }
    }

    public mutating func removePane(_ paneId: UInt32) {
        agents.removeValue(forKey: paneId)
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

    public init(
        workspaceId: String,
        name: String,
        runtime: String,
        transport: String,
        isActive: Bool,
        shortcut: Int? = nil,
        structuredAgents: [StructuredPaneAgent] = []
    ) {
        self.workspaceId = workspaceId
        self.name = name
        self.runtime = runtime
        self.transport = transport
        self.isActive = isActive
        self.shortcut = shortcut
        self.structuredAgents = structuredAgents
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
    public let paneId: UInt32
    public let title: String
    public let detail: String
    public let indicator: AgentSidebarIndicator

    public init(
        workspaceId: String,
        paneId: UInt32,
        title: String,
        detail: String,
        indicator: AgentSidebarIndicator
    ) {
        self.workspaceId = workspaceId
        self.paneId = paneId
        self.title = title
        self.detail = detail
        self.indicator = indicator
    }
}

/// A running or unread ordinary command. Agents remain in the Agents section.
public struct CommandSidebarItem: Sendable, Equatable {
    public let workspaceId: String
    public let paneId: UInt32
    public let title: String
    public let detail: String
    public let indicator: AgentSidebarIndicator

    public init(
        workspaceId: String,
        paneId: UInt32,
        title: String,
        detail: String,
        indicator: AgentSidebarIndicator
    ) {
        self.workspaceId = workspaceId
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
        "pi", "hermes",
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
                result.append(AgentSidebarItem(
                    workspaceId: workspace.workspaceId,
                    paneId: agent.paneId,
                    title: firstNonempty([
                        agent.displayName,
                        agent.title,
                        agent.name,
                        agent.kind,
                    ]) ?? "Agent",
                    detail: detail(workspace: workspace, paneId: agent.paneId),
                    indicator: indicator(status: agent.status, attention: attention)
                ))
            }

            let generic = attention?.workspaces
                .first(where: { $0.workspaceId == workspace.workspaceId })?
                .panes ?? []
            for pane in generic where !structuredPaneIDs.contains(pane.paneId) {
                guard let name = knownAgentName(pane.processName) else { continue }
                result.append(AgentSidebarItem(
                    workspaceId: workspace.workspaceId,
                    paneId: pane.paneId,
                    title: name,
                    detail: detail(workspace: workspace, paneId: pane.paneId),
                    indicator: indicator(attention: pane)
                ))
            }
        }
        return result
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
            })?.panes ?? [] where knownAgentName(pane.processName) != nil {
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
                        paneId: pane.paneId,
                        title: title,
                        detail: detail(workspace: workspace, paneId: pane.paneId),
                        indicator: .running
                    ))
                case .blocked, .done:
                    guard !pane.acknowledged else { continue }
                    result.append(CommandSidebarItem(
                        workspaceId: workspace.workspaceId,
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

    private static func indicator(
        status: StructuredAgentStatus,
        attention: PaneAttention?
    ) -> AgentSidebarIndicator {
        switch status {
        case .working:
            return .running
        case .blocked, .done:
            return attention?.acknowledged == true ? .read : .done
        case .idle, .unknown:
            return .read
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
