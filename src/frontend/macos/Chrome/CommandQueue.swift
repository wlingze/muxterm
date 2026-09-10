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

    private var coalescingKey: CoalescingKey? {
        switch self {
        case .task(let task) where task.coalescing == .switchTab:
            return .switchTab
        case .resize(let resize):
            return .resize(resize.coalescingKey)
        case .task, .input:
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
