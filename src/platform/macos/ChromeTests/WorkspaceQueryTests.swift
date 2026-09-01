import XCTest
@testable import MuxtermChrome

final class WorkspaceQueryTests: XCTestCase {
    private func config(
        name: String = "MuxTerm",
        runtime: TargetRuntime = .tmux,
        transport: TargetTransport = .local,
        path: String = "/srv/muxterm"
    ) -> TargetConfig {
        TargetConfig(
            name: name,
            runtime: runtime,
            transport: transport,
            path: path
        )
    }

    func testFuzzyQueryCombinesCaseInsensitiveRuntimeAndSSHFilters() {
        let remote = config(transport: .ssh(name: "RyZen"))
        let local = config()

        XCTAssertTrue(WorkspaceQuery("MXE @TMUX @RYZEN").matches(remote))
        XCTAssertFalse(WorkspaceQuery("MXE @TMUX @RYZEN").matches(local))
        XCTAssertTrue(WorkspaceQuery("mxe @tmux @local").matches(local))
        XCTAssertFalse(WorkspaceQuery("mxe @tmux @local").matches(remote))
    }

    func testQueryAlsoSearchesAttachIdentityFields() {
        var target = config(
            name: "display-name",
            runtime: .herdr,
            path: ""
        )
        target.session = "agents"
        target.socket = "/tmp/agents.sock"
        target.workspaceID = "w7"

        XCTAssertTrue(WorkspaceQuery("agent").matches(target))
        XCTAssertTrue(WorkspaceQuery("w7").matches(target))
        XCTAssertTrue(WorkspaceQuery("sock").matches(target))
    }

    func testAtCompletionIncludesRuntimeLocalAndCaseInsensitiveAliasWithoutDuplicates() {
        XCTAssertEqual(
            WorkspaceQuery.completionCandidates(
                for: "@",
                sshAliases: ["ryzen", "RYZEN", "legion"]
            ),
            ["@shell", "@tmux", "@herdr", "@local", "@ryzen", "@legion"]
        )
        XCTAssertEqual(
            WorkspaceQuery.completionCandidates(for: "@ry", sshAliases: ["ryzen", "legion"]),
            ["@ryzen"]
        )
        XCTAssertEqual(
            WorkspaceQuery.completionCandidates(for: "project ", sshAliases: ["ryzen"]),
            []
        )
    }

    func testCompletionReplacesOnlyTheCurrentToken() {
        XCTAssertEqual(
            WorkspaceQuery.replaceCurrentToken(in: "project @ry", with: "@ryzen"),
            "project @ryzen"
        )
        XCTAssertEqual(
            WorkspaceQuery.replaceCurrentToken(in: "project @tmux", with: "@tmux"),
            "project @tmux"
        )
    }
}
