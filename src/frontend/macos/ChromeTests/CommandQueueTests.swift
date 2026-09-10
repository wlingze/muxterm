import Foundation
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
        XCTAssertEqual(queue.drain().compactMap { operation in
            guard case .task(let task) = operation.operation else { return nil }
            return task.targetTab
        }, [0])
    }

    func testSwitchesAcrossWorkspacesRemainOrdered() {
        var queue = MacCommandQueue()
        queue.enqueue(switchCommand(workspaceID: "one", tabID: 61))
        queue.enqueue(switchCommand(workspaceID: "two", tabID: 7))
        queue.enqueue(switchCommand(workspaceID: "one", tabID: 0))

        XCTAssertEqual(queue.count, 3)
        let commands = queue.drain()
        XCTAssertEqual(commands.map { $0.workspaceID ?? "" }, ["one", "two", "one"])
        XCTAssertEqual(commands.compactMap { operation in
            guard case .task(let task) = operation.operation else { return nil }
            return task.targetTab
        }, [61, 7, 0])
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

    func testInputKeepsByteOrderAndIsNotCoalesced() {
        var queue = MacCommandQueue()
        queue.enqueue(.input(
            workspaceID: "one",
            paneID: 3,
            data: Data([0x61]),
            failureMessage: "input failed"
        ))
        queue.enqueue(.input(
            workspaceID: "one",
            paneID: 3,
            data: Data([0x62, 0x0d]),
            failureMessage: "input failed"
        ))

        XCTAssertEqual(queue.count, 2)
        let commands = queue.drain()
        guard case .input(let firstPaneID, let firstData, let firstQuiet) = commands[0].operation,
              case .input(let secondPaneID, let secondData, let secondQuiet) = commands[1].operation
        else {
            return XCTFail("expected two input operations")
        }
        XCTAssertEqual(firstPaneID, 3)
        XCTAssertEqual(Array(firstData), [0x61])
        XCTAssertFalse(firstQuiet)
        XCTAssertEqual(secondPaneID, 3)
        XCTAssertEqual(Array(secondData), [0x62, 0x0d])
        XCTAssertFalse(secondQuiet)
    }

    func testConsecutivePaneResizeForOneWorkspaceAndPaneKeepsLastSize() {
        var queue = MacCommandQueue()
        queue.enqueue(.resize(
            workspaceID: "one",
            .pane(paneID: 3, cols: 80, rows: 24),
            failureMessage: "resize failed"
        ))
        queue.enqueue(.resize(
            workspaceID: "one",
            .pane(paneID: 3, cols: 120, rows: 40),
            failureMessage: "resize failed"
        ))

        XCTAssertEqual(queue.count, 1)
        let commands = queue.drain()
        guard case .resize(.pane(let paneID, let cols, let rows)) = commands[0].operation else {
            return XCTFail("expected a pane resize")
        }
        XCTAssertEqual(paneID, 3)
        XCTAssertEqual(cols, 120)
        XCTAssertEqual(rows, 40)
    }

    func testResizeAcrossWorkspacesOrPanesRemainsOrdered() {
        var queue = MacCommandQueue()
        queue.enqueue(.resize(
            workspaceID: "one",
            .pane(paneID: 3, cols: 80, rows: 24),
            failureMessage: "first"
        ))
        queue.enqueue(.resize(
            workspaceID: "one",
            .pane(paneID: 4, cols: 90, rows: 25),
            failureMessage: "second"
        ))
        queue.enqueue(.resize(
            workspaceID: "two",
            .pane(paneID: 3, cols: 100, rows: 26),
            failureMessage: "third"
        ))

        XCTAssertEqual(queue.count, 3)
        XCTAssertEqual(queue.drain().map(\.failureMessage), ["first", "second", "third"])
    }

    func testResizeSeparatedByInputDoesNotReorderOrCoalesceAcrossInput() {
        var queue = MacCommandQueue()
        queue.enqueue(.resize(
            workspaceID: "one",
            .pane(paneID: 3, cols: 80, rows: 24),
            failureMessage: "first"
        ))
        queue.enqueue(.input(
            workspaceID: "one",
            paneID: 3,
            data: Data([0x0d]),
            failureMessage: "input"
        ))
        queue.enqueue(.resize(
            workspaceID: "one",
            .pane(paneID: 3, cols: 120, rows: 40),
            failureMessage: "second"
        ))

        XCTAssertEqual(queue.count, 3)
        XCTAssertEqual(queue.drain().map(\.failureMessage), ["first", "input", "second"])
    }

    func testAttentionMutationsRemainWorkspaceAwareAndOrdered() {
        var queue = MacCommandQueue()
        queue.enqueue(.attention(
            workspaceID: "one",
            .acknowledge(paneID: 3),
            failureMessage: "acknowledge failed"
        ))
        queue.enqueue(.attention(
            workspaceID: "two",
            .mute(paneID: 4, seconds: 30),
            failureMessage: "mute failed"
        ))

        XCTAssertEqual(queue.count, 2)
        let commands = queue.drain()
        XCTAssertEqual(commands.map { $0.workspaceID ?? "" }, ["one", "two"])
        guard case .attention(.acknowledge(let firstPaneID)) = commands[0].operation,
              case .attention(.mute(let secondPaneID, let seconds)) = commands[1].operation
        else {
            return XCTFail("expected ordered attention operations")
        }
        XCTAssertEqual(firstPaneID, 3)
        XCTAssertEqual(secondPaneID, 4)
        XCTAssertEqual(seconds, 30)
    }

    func testConsecutiveViewportChangesForOneWorkspaceAndPaneKeepLastOffset() {
        var queue = MacCommandQueue()
        queue.enqueue(.viewport(
            workspaceID: "one",
            paneID: 3,
            offset: 12
        ))
        queue.enqueue(.viewport(
            workspaceID: "one",
            paneID: 3,
            offset: 4
        ))

        XCTAssertEqual(queue.count, 1)
        guard case .viewport(let paneID, let offset) = queue.drain()[0].operation else {
            return XCTFail("expected a viewport operation")
        }
        XCTAssertEqual(paneID, 3)
        XCTAssertEqual(offset, 4)
    }

    func testViewportChangesAcrossPanesRemainOrdered() {
        var queue = MacCommandQueue()
        queue.enqueue(.viewport(workspaceID: "one", paneID: 3, offset: 12))
        queue.enqueue(.viewport(workspaceID: "one", paneID: 4, offset: 8))
        queue.enqueue(.viewport(workspaceID: "two", paneID: 3, offset: 2))

        XCTAssertEqual(queue.count, 3)
        XCTAssertEqual(queue.drain().map { $0.workspaceID ?? "" }, ["one", "one", "two"])
    }

    func testWorkspaceCloseKeepsExplicitWorkspaceIdentity() {
        var queue = MacCommandQueue()
        queue.enqueue(.closeWorkspace(
            workspaceID: "one",
            failureMessage: "close failed"
        ))

        let commands = queue.drain()
        XCTAssertEqual(commands.count, 1)
        XCTAssertEqual(commands[0].workspaceID, "one")
        guard case .closeWorkspace = commands[0].operation else {
            return XCTFail("expected a workspace close operation")
        }
    }
}
