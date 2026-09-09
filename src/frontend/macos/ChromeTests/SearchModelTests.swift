import XCTest
@testable import MuxtermChrome

final class SearchSnapshotDecodeTests: XCTestCase {
    func testDecodesHits() throws {
        let json = """
        {
          "ok": true,
          "hits": [
            {"workspace_id": "legion@local", "tab_id": 1, "pane_id": 1, "seq": 3, "line": "TOKEN_BODY example"},
            {"workspace_id": "other@local", "tab_id": 2, "pane_id": 2, "seq": 7, "line": "build ok"}
          ]
        }
        """
        let snapshot = try XCTUnwrap(SearchSnapshot.decode(Data(json.utf8)))
        XCTAssertEqual(snapshot.hits.count, 2)
        XCTAssertEqual(snapshot.hits[0].workspaceId, "legion@local")
        XCTAssertEqual(snapshot.hits[0].tabId, 1)
        XCTAssertEqual(snapshot.hits[0].paneId, 1)
        XCTAssertEqual(snapshot.hits[0].seq, 3)
        XCTAssertEqual(snapshot.hits[0].line, "TOKEN_BODY example")
    }

    func testDecodesTabIdZero() throws {
        let json = """
        {
          "ok": true,
          "hits": [
            {"workspace_id": "ws@local", "tab_id": 0, "pane_id": 1, "seq": 9, "line": "on-tab-zero"}
          ]
        }
        """
        let snapshot = try XCTUnwrap(SearchSnapshot.decode(Data(json.utf8)))
        XCTAssertEqual(snapshot.hits.count, 1)
        XCTAssertEqual(snapshot.hits[0].tabId, 0)
        XCTAssertEqual(snapshot.hits[0].seq, 9)
    }

    func testDecodeFailureReturnsNil() {
        XCTAssertNil(SearchSnapshot.decode(Data("bad".utf8)))
        XCTAssertNil(SearchSnapshot.decode(Data(#"{"ok": false}"#.utf8)))
    }
}

final class SearchListTests: XCTestCase {
    func testFiltersByLineAndWorkspace() {
        let snap = SearchSnapshot(hits: [
            SearchHit(workspaceId: "legion@local", tabId: 1, paneId: 1, seq: 3, line: "TOKEN_BODY example"),
            SearchHit(workspaceId: "other@local", tabId: 2, paneId: 2, seq: 7, line: "build ok"),
        ])
        let (rows, isEmpty) = SearchList.rows(from: snap, query: "TOKEN_BODY")
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].paneId, 1)
        XCTAssertFalse(isEmpty)

        let (empty, emptyFlag) = SearchList.rows(from: snap, query: "missing")
        XCTAssertTrue(empty.isEmpty)
        XCTAssertTrue(emptyFlag)
    }
}
