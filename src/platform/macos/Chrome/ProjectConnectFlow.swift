import Foundation

/// Project 连接流程：先 attach 已有 session，明确失败后再按 twork 语义创建，
/// 创建成功后 attach 同一 session。local 与 ssh 共用同一状态机。
public struct ProjectConnectFailure: Equatable {
    public enum Stage: Equatable {
        case attachExisting
        case create
        case attachCreated
    }

    public let stage: Stage
    public let detail: String

    public init(stage: Stage, detail: String) {
        self.stage = stage
        self.detail = detail
    }
}

public enum ProjectConnectState: Equatable {
    /// 尝试 attach 已有 session（twork 的 has-session + attach 语义）。
    case attachExisting(session: String)
    /// attach 明确失败：创建 detached session（twork 的 new-session -d）。
    case createDetached(session: String, directory: String)
    /// 创建成功：attach 刚创建的 session。
    case attachCreated(session: String)
    /// 全部成功。
    case done
    /// 某一步失败（区分 attach / create / attach-after-create）。
    case failed(ProjectConnectFailure)
}

/// Project tmux 目标的连接状态机（纯逻辑，可单测）。
public struct ProjectConnectFlow {
    /// 最终 session 名：显式 name 优先，空 name 用 path basename（与 twork 对齐）。
    public let session: String
    /// 创建 detached session 时使用的目录（twork 的 `-c <dir>`）。
    public let directory: String
    public private(set) var state: ProjectConnectState

    public init(config: TargetConfig) {
        let trimmedName = config.name.trimmingCharacters(in: .whitespacesAndNewlines)
        self.session = trimmedName.isEmpty
            ? QuickConnect.defaultName(for: config.path)
            : trimmedName
        let trimmedPath = config.path.trimmingCharacters(in: .whitespacesAndNewlines)
        self.directory = trimmedPath.isEmpty ? "~" : trimmedPath
        self.state = .attachExisting(session: session)
    }

    /// attach 已有 session 成功。
    public mutating func attachExistingSucceeded() {
        state = .done
    }

    /// attach 明确失败 → 创建 detached session（不杀、不破坏已有 session）。
    public mutating func attachExistingFailed(message: String) {
        guard case .attachExisting = state else { return }
        state = .createDetached(session: session, directory: directory)
    }

    /// detached session 创建成功 → attach 它。
    public mutating func createSucceeded() {
        guard case .createDetached = state else { return }
        state = .attachCreated(session: session)
    }

    /// attach 刚创建的 session 成功。
    public mutating func attachCreatedSucceeded() {
        guard case .attachCreated = state else { return }
        state = .done
    }

    /// 创建 detached session 失败（与 attach 失败区分）。
    public mutating func createFailed(message: String) {
        state = .failed(ProjectConnectFailure(stage: .create, detail: message))
    }

    /// attach 刚创建的 session 失败（与首次 attach 失败区分）。
    public mutating func attachCreatedFailed(message: String) {
        state = .failed(ProjectConnectFailure(stage: .attachCreated, detail: message))
    }
}
