import XCTest
@testable import MuxtermChrome

final class ProjectConnectFlowTests: XCTestCase {
    private func config(
        name: String,
        path: String,
        transport: TargetTransport = .local
    ) -> TargetConfig {
        TargetConfig(name: name, runtime: .tmux, transport: transport, path: path)
    }

    func testSessionNameUsesExplicitNameWhenPresent() {
        let flow = ProjectConnectFlow(config: config(name: "yaklang", path: "/x/yaklang-workspace"))
        XCTAssertEqual(flow.session, "yaklang")
        XCTAssertEqual(flow.state, .attachExisting(session: "yaklang"))
    }

    func testSessionNameFallsBackToPathBasenameLikeTwork() {
        let flow = ProjectConnectFlow(
            config: config(name: "", path: "/Users/wlz/Developer/self/muxterm")
        )
        XCTAssertEqual(flow.session, "muxterm")
        XCTAssertEqual(flow.state, .attachExisting(session: "muxterm"))
    }

    func testFallbackDoesNotUseRandomMuxtermSuffix() {
        let flow = ProjectConnectFlow(
            config: config(name: "", path: "/home/wlz/Developer/self/muxterm")
        )
        XCTAssertEqual(flow.session, "muxterm")
        XCTAssertFalse(flow.session.hasPrefix("muxterm-muxterm-"))
        XCTAssertFalse(flow.session.contains("-"))
    }

    func testAttachFailureFallsBackToDetachedCreateWithSameSessionAndDirectory() {
        var flow = ProjectConnectFlow(config: config(name: "proj", path: "/srv/proj"))
        flow.attachExistingFailed(message: "no such session")
        XCTAssertEqual(
            flow.state,
            .createDetached(session: "proj", directory: "/srv/proj")
        )
    }

    func testCreateSuccessThenAttachCreatedSession() {
        var flow = ProjectConnectFlow(config: config(name: "proj", path: "/srv/proj"))
        flow.attachExistingFailed(message: "no such session")
        flow.createSucceeded()
        XCTAssertEqual(flow.state, .attachCreated(session: "proj"))
        flow.attachCreatedSucceeded()
        XCTAssertEqual(flow.state, .done)
    }

    func testCreateFailureIsDistinguished() {
        var flow = ProjectConnectFlow(config: config(name: "proj", path: "/srv/proj"))
        flow.attachExistingFailed(message: "no such session")
        flow.createFailed(message: "permission denied")
        XCTAssertEqual(
            flow.state,
            .failed(ProjectConnectFailure(stage: .create, detail: "permission denied"))
        )
    }

    func testAttachAfterCreateFailureIsDistinguishedFromAttachFailure() {
        var flow = ProjectConnectFlow(config: config(name: "proj", path: "/srv/proj"))
        flow.attachExistingFailed(message: "no such session")
        flow.createSucceeded()
        flow.attachCreatedFailed(message: "detach race")
        XCTAssertEqual(
            flow.state,
            .failed(ProjectConnectFailure(stage: .attachCreated, detail: "detach race"))
        )
    }

    func testSSHAndLocalFollowSameProjectSemantics() {
        let local = ProjectConnectFlow(config: config(name: "", path: "/x/muxterm"))
        let ssh = ProjectConnectFlow(
            config: config(name: "", path: "~/Developer/self/muxterm", transport: .ssh(name: "ryzen"))
        )
        XCTAssertEqual(local.session, ssh.session)
        XCTAssertEqual(local.directory, local.directory)
        XCTAssertEqual(local.state, ssh.state)
    }
}
