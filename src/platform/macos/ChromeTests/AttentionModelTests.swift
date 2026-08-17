import XCTest
@testable import MuxtermChrome

final class AttentionSnapshotDecodeTests: XCTestCase {
    func testDecodesBlockedCountAndWorkspaces() throws {
        let json = """
        {
          "ok": true,
          "blocked_count": 2,
          "workspaces": [
            {
              "workspace_id": "legion@local",
              "blocked": 1,
              "done": 0,
              "working": 0,
              "panes": [
                {"pane_id": 1, "status": "blocked", "last_line": "ask?", "seq": 3, "process_name": "cat"}
              ]
            },
            {
              "workspace_id": "other@local",
              "blocked": 0,
              "done": 1,
              "working": 0,
              "panes": [
                {"pane_id": 2, "status": "done", "last_line": "complete", "seq": 7, "process_name": null}
              ]
            }
          ]
        }
        """
        let snapshot = try XCTUnwrap(
            AttentionSnapshot.decode(Data(json.utf8))
        )
        XCTAssertEqual(snapshot.blockedCount, 2)
        XCTAssertEqual(snapshot.workspaces.count, 2)
        XCTAssertEqual(snapshot.workspaces[0].workspaceId, "legion@local")
        XCTAssertEqual(snapshot.workspaces[0].panes[0].status, .blocked)
        XCTAssertEqual(snapshot.workspaces[0].panes[0].lastLine, "ask?")
        XCTAssertEqual(snapshot.workspaces[1].panes[0].status, .done)
    }

    func testDecodeFailureReturnsNil() {
        XCTAssertNil(AttentionSnapshot.decode(Data("not json".utf8)))
        XCTAssertNil(AttentionSnapshot.decode(Data(#"{"ok": false}"#.utf8)))
    }
}

final class AttentionListTests: XCTestCase {
    private func snapshot(panes: [(id: UInt32, status: PaneAttentionStatus, seq: UInt64, line: String)]) -> AttentionSnapshot {
        AttentionSnapshot(
            blockedCount: panes.filter { $0.status == .blocked }.count,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: "ws@local",
                    blocked: panes.filter { $0.status == .blocked }.count,
                    done: panes.filter { $0.status == .done }.count,
                    working: 0,
                    panes: panes.map {
                        PaneAttention(paneId: $0.id, status: $0.status, lastLine: $0.line, seq: $0.seq, processName: "cat")
                    }
                )
            ]
        )
    }

    func testOnlyBlockedAndDoneAreListed() {
        let snap = snapshot(panes: [
            (1, .blocked, 1, "ask?"),
            (2, .done, 2, "complete"),
            (3, .working, 3, "running"),
            (4, .idle, 4, "idle"),
        ])
        let rows = AttentionList.rows(from: snap, query: "")
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0].pane.paneId, 1)
        XCTAssertEqual(rows[1].pane.paneId, 2)
    }

    func testBlockedFirstThenNewerSeq() {
        let snap = snapshot(panes: [
            (1, .done, 1, "old done"),
            (2, .blocked, 2, "ask"),
            (3, .blocked, 3, "ask2"),
        ])
        let rows = AttentionList.rows(from: snap, query: "")
        XCTAssertEqual(rows.map(\.pane.paneId), [3, 2, 1])
    }

    func testQueryFiltersByWorkspaceProcessAndLine() {
        let snap = AttentionSnapshot(
            blockedCount: 2,
            workspaces: [
                WorkspaceAttention(workspaceId: "legion@local", blocked: 1, done: 0, working: 0, panes: [
                    PaneAttention(paneId: 1, status: .blocked, lastLine: "ask?", seq: 1, processName: "codex")
                ]),
                WorkspaceAttention(workspaceId: "other@local", blocked: 1, done: 0, working: 0, panes: [
                    PaneAttention(paneId: 2, status: .blocked, lastLine: "confirm?", seq: 2, processName: "bash")
                ]),
            ]
        )
        XCTAssertEqual(AttentionList.rows(from: snap, query: "legion").count, 1)
        XCTAssertEqual(AttentionList.rows(from: snap, query: "codex").count, 1)
        XCTAssertEqual(AttentionList.rows(from: snap, query: "confirm").count, 1)
        XCTAssertEqual(AttentionList.rows(from: snap, query: "missing").count, 0)
    }
}

final class AttentionNotificationsDecodeTests: XCTestCase {
    func testDecodesBlockedAndDone() throws {
        let json = #"{"ok": true, "blocked": ["a@local"], "done": ["b@local"]}"#
        let notifications = try XCTUnwrap(AttentionNotifications.decode(Data(json.utf8)))
        XCTAssertEqual(notifications.blocked, ["a@local"])
        XCTAssertEqual(notifications.done, ["b@local"])
    }

    func testDecodeFailureReturnsNil() {
        XCTAssertNil(AttentionNotifications.decode(Data(#"{"ok": false}"#.utf8)))
    }
}
