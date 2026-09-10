import Foundation

/// Core task DTO queued by macOS UI actions.
///
/// The Chrome module deliberately does not import the C header.  `MainWindow`
/// maps this owned value to `MuxTask` at the single event-pump boundary.
public struct QueuedMuxTask: Equatable, Sendable {
    public enum Coalescing: Equatable, Sendable {
        case switchTab
        case none
    }

    public let type: UInt32
    public let targetPane: UInt32
    public let targetTab: UInt32
    public let dir: UInt32
    public let name: String?
    public let coalescing: Coalescing

    public init(
        type: UInt32,
        targetPane: UInt32 = 0,
        targetTab: UInt32 = 0,
        dir: UInt32 = 0,
        name: String? = nil,
        coalescing: Coalescing = .none
    ) {
        self.type = type
        self.targetPane = targetPane
        self.targetTab = targetTab
        self.dir = dir
        self.name = name
        self.coalescing = coalescing
    }
}

/// Core input or resize operation waiting for the event-pump boundary.
public enum QueuedMuxOperation: Equatable, Sendable {
    case task(QueuedMuxTask)
    case input(paneID: UInt32, data: Data, quiet: Bool)
    case resize(QueuedMuxResize)
    case attention(QueuedMuxAttention)
    case viewport(paneID: UInt32, offset: UInt32)
    case closeWorkspace
    case colours(QueuedMuxColours)

    private var coalescingKey: CoalescingKey? {
        switch self {
        case .task(let task) where task.coalescing == .switchTab:
            return .switchTab
        case .resize(let resize):
            return .resize(resize.coalescingKey)
        case .viewport(let paneID, _):
            return .viewport(paneID)
        case .colours(let colours):
            switch colours {
            case .pane(let paneID, _, _):
                return .coloursPane(paneID)
            case .all:
                return .coloursAll
            }
        case .attention(let attention):
            switch attention {
            case .becameVisible(let paneID):
                return .attentionVisible(paneID)
            case .setProcessName(let paneID, _):
                return .attentionProcessName(paneID)
            case .acknowledge, .mute:
                return nil
            }
        case .task, .input, .closeWorkspace:
            return nil
        }
    }

    fileprivate func canCoalesce(with other: QueuedMuxOperation) -> Bool {
        guard let key = coalescingKey else { return false }
        return key == other.coalescingKey
    }

    private enum CoalescingKey: Equatable {
        case switchTab
        case resize(QueuedMuxResize.CoalescingKey)
        case viewport(UInt32)
        case coloursPane(UInt32)
        case coloursAll
        case attentionVisible(UInt32)
        case attentionProcessName(UInt32)
    }
}

/// Resize operations use separate keys because a full pane resize, a divider
/// resize, and a client resize are different Core calls even when they share a
/// pane or workspace.
public enum QueuedMuxResize: Equatable, Sendable {
    case pane(paneID: UInt32, cols: UInt16, rows: UInt16)
    case paneAxis(paneID: UInt32, horizontal: Bool, size: UInt16)
    case client(cols: UInt16, rows: UInt16)

    fileprivate var coalescingKey: CoalescingKey {
        switch self {
        case .pane(let paneID, _, _):
            return .pane(paneID)
        case .paneAxis(let paneID, let horizontal, _):
            return .paneAxis(paneID, horizontal: horizontal)
        case .client:
            return .client
        }
    }

    fileprivate enum CoalescingKey: Equatable {
        case pane(UInt32)
        case paneAxis(UInt32, horizontal: Bool)
        case client
    }
}

/// Attention mutations are routed through the same serialized boundary as
/// terminal tasks so a scene switch cannot retarget an acknowledgement.
public enum QueuedMuxAttention: Equatable, Sendable {
    case becameVisible(paneID: UInt32)
    case setProcessName(paneID: UInt32, name: String?)
    case acknowledge(paneID: UInt32)
    case mute(paneID: UInt32, seconds: UInt64)
}

/// Terminal colour reports are scoped to a workspace so a delayed event-pump
/// flush cannot update a pane with the same numeric ID in another scene.
public enum QueuedMuxColours: Equatable, Sendable {
    case pane(paneID: UInt32, fgHex: String, bgHex: String)
    case all(fgHex: String, bgHex: String)
}

/// One UI-to-Core operation waiting for the next main-thread event-pump flush.
public struct QueuedMuxCommand: Equatable, Sendable {
    public let workspaceID: String?
    public let operation: QueuedMuxOperation
    public let failureMessage: String

    public init(
        workspaceID: String?,
        task: QueuedMuxTask,
        failureMessage: String
    ) {
        self.workspaceID = workspaceID
        self.operation = .task(task)
        self.failureMessage = failureMessage
    }

    public init(
        workspaceID: String?,
        operation: QueuedMuxOperation,
        failureMessage: String
    ) {
        self.workspaceID = workspaceID
        self.operation = operation
        self.failureMessage = failureMessage
    }

    public static func input(
        workspaceID: String?,
        paneID: UInt32,
        data: Data,
        quiet: Bool = false,
        failureMessage: String
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            operation: .input(paneID: paneID, data: data, quiet: quiet),
            failureMessage: failureMessage
        )
    }

    public static func resize(
        workspaceID: String?,
        _ resize: QueuedMuxResize,
        failureMessage: String
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            operation: .resize(resize),
            failureMessage: failureMessage
        )
    }

    public static func attention(
        workspaceID: String?,
        _ attention: QueuedMuxAttention,
        failureMessage: String
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            operation: .attention(attention),
            failureMessage: failureMessage
        )
    }

    public static func viewport(
        workspaceID: String?,
        paneID: UInt32,
        offset: UInt32,
        failureMessage: String = ""
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            operation: .viewport(paneID: paneID, offset: offset),
            failureMessage: failureMessage
        )
    }

    public static func closeWorkspace(
        workspaceID: String,
        failureMessage: String
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            operation: .closeWorkspace,
            failureMessage: failureMessage
        )
    }

    public static func colours(
        workspaceID: String?,
        _ colours: QueuedMuxColours,
        failureMessage: String = ""
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            operation: .colours(colours),
            failureMessage: failureMessage
        )
    }
}

/// Main-thread command queue for the macOS frontend.
///
/// UI actions only append owned commands and return.  The event pump is the
/// sole consumer and is responsible for dispatching them to Core.  Consecutive
/// tab switches for one Workspace collapse to the last target; commands for
/// different Workspaces remain ordered.
public struct MacCommandQueue: Sendable {
    private var pending: [QueuedMuxCommand] = []

    public init() {}

    public var isEmpty: Bool { pending.isEmpty }

    public var count: Int { pending.count }

    public mutating func enqueue(_ command: QueuedMuxCommand) {
        if let previous = pending.last,
           previous.workspaceID == command.workspaceID,
           previous.operation.canCoalesce(with: command.operation)
        {
            pending[pending.count - 1] = command
            return
        }
        pending.append(command)
    }

    public mutating func drain() -> [QueuedMuxCommand] {
        defer { pending.removeAll(keepingCapacity: true) }
        return pending
    }
}
