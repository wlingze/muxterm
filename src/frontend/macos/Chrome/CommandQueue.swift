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

/// One UI-to-Core task waiting for the next main-thread event-pump flush.
public struct QueuedMuxCommand: Equatable, Sendable {
    public let workspaceID: String?
    public let task: QueuedMuxTask
    public let failureMessage: String

    public init(
        workspaceID: String?,
        task: QueuedMuxTask,
        failureMessage: String
    ) {
        self.workspaceID = workspaceID
        self.task = task
        self.failureMessage = failureMessage
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
           previous.task.coalescing == .switchTab,
           command.task.coalescing == .switchTab
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
