import XCTest
@testable import MuxtermChrome

final class MacCommandQueueTests: XCTestCase {
    private func switchCommand(
        workspaceID: String?,
        tabID: UInt32,
        failureMessage: String = "switch failed"
    ) -> QueuedMuxCommand {
        QueuedMuxCommand(
            workspaceID: workspaceID,
            task: QueuedMuxTask(
                type: 1,
                targetTab: tabID,
                coalescing: .switchTab
            ),
            failureMessage: failureMessage
        )
    }

    func testConsecutiveSwitchesForOneWorkspaceKeepLastTarget() {
        var queue = MacCommandQueue()
        queue.enqueue(switchCommand(workspaceID: "one", tabID: 61))
        queue.enqueue(switchCommand(workspaceID: "one", tabID: 0))

        XCTAssertEqual(queue.count, 1)
        XCTAssertEqual(queue.drain().map { $0.task.targetTab }, [0])
    }

    func testSwitchesAcrossWorkspacesRemainOrdered() {
        var queue = MacCommandQueue()
        queue.enqueue(switchCommand(workspaceID: "one", tabID: 61))
        queue.enqueue(switchCommand(workspaceID: "two", tabID: 7))
        queue.enqueue(switchCommand(workspaceID: "one", tabID: 0))

        XCTAssertEqual(queue.count, 3)
        let commands = queue.drain()
        XCTAssertEqual(commands.map { $0.workspaceID ?? "" }, ["one", "two", "one"])
        XCTAssertEqual(commands.map { $0.task.targetTab }, [61, 7, 0])
    }

    func testNonNavigationTasksKeepOrder() {
        var queue = MacCommandQueue()
        queue.enqueue(QueuedMuxCommand(
            workspaceID: "one",
            task: QueuedMuxTask(type: 2),
            failureMessage: "first"
        ))
        queue.enqueue(QueuedMuxCommand(
            workspaceID: "one",
            task: QueuedMuxTask(type: 2),
            failureMessage: "second"
        ))

        XCTAssertEqual(queue.count, 2)
        XCTAssertEqual(queue.drain().map(\.failureMessage), ["first", "second"])
    }
}
