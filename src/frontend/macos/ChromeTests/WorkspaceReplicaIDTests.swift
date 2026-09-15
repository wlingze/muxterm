import XCTest
@testable import MuxtermChrome

final class WorkspaceReplicaIDTests: XCTestCase {
    func testSshSessionKeepsProjectPathLikeCoreReplicaId() {
        XCTAssertEqual(
            WorkspaceReplicaID.from(
                session: "muxterm",
                path: "~/Developer/self/muxterm",
                transport: "ryzen"
            ),
            "muxterm:~/Developer/self/muxterm@ryzen"
        )
    }

    func testSharedSessionNamesStayDistinctByPath() {
        XCTAssertNotEqual(
            WorkspaceReplicaID.from(session: "shared", path: "one", transport: "local"),
            WorkspaceReplicaID.from(session: "shared", path: "two", transport: "local")
        )
    }

    func testEmptySessionFallsBackToPathBasename() {
        XCTAssertEqual(
            WorkspaceReplicaID.from(session: nil, path: "/tmp/foo", transport: "local"),
            "foo@local"
        )
    }

    func testHerdrReplicaUsesWorkspaceIDNotProjectPath() {
        let config = TargetConfig(
            name: "yakit",
            runtime: .herdr,
            transport: .local,
            path: "~/Developer/Yak/yakit",
            session: "default",
            workspaceID: "w9"
        )
        XCTAssertEqual(WorkspaceReplicaID.from(config), "default:w9@local")
        XCTAssertFalse(WorkspaceReplicaID.from(config).contains("Yak"))
    }

    func testTargetConfigUsesSessionPathAndSshAlias() {
        let config = TargetConfig(
            name: "muxterm",
            runtime: .tmux,
            transport: .ssh(name: "ryzen"),
            path: "~/Developer/self/muxterm",
            session: "muxterm"
        )
        XCTAssertEqual(
            WorkspaceReplicaID.from(config),
            "muxterm:~/Developer/self/muxterm@ryzen"
        )
        XCTAssertFalse(WorkspaceReplicaID.from(config).contains("ssh/ryzen/"))
    }
}
