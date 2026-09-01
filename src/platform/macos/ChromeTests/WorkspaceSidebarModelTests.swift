import XCTest
@testable import MuxtermChrome

final class WorkspaceSidebarModelTests: XCTestCase {
    func testStructuredAgentRegistryKeepsReadIdentityUntilPaneCloses() {
        var registry = StructuredAgentRegistry()
        registry.observe(
            paneId: 4,
            agent: StructuredPaneAgent(
                paneId: 4,
                displayName: "Codex",
                title: "Review muxterm",
                name: "codex",
                kind: "codex",
                status: .working
            )
        )

        registry.observe(paneId: 4, agent: nil)
        XCTAssertEqual(registry.snapshot.count, 1)
        XCTAssertEqual(registry.snapshot[0].displayName, "Codex")
        XCTAssertEqual(registry.snapshot[0].status, .idle)
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@agents@herdr@w2",
            name: "muxterm",
            runtime: "herdr",
            transport: "local",
            isActive: true,
            structuredAgents: registry.snapshot
        )
        let projected = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: nil
        )
        XCTAssertEqual(projected.count, 1)
        XCTAssertEqual(projected[0].indicator, .read)

        registry.removePane(4)
        XCTAssertTrue(registry.snapshot.isEmpty)
    }

    func testProjectsStructuredHerdrAndTmuxAgentsAcrossWorkspaces() {
        let herdr = WorkspaceSidebarItem(
            workspaceId: "local@@agents@herdr@w2",
            name: "muxterm",
            runtime: "herdr",
            transport: "local",
            isActive: true,
            structuredAgents: [
                StructuredPaneAgent(
                    paneId: 4,
                    displayName: "Codex",
                    title: "Review muxterm",
                    name: "codex",
                    kind: "codex",
                    status: .working
                ),
            ]
        )
        let tmux = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: false
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: herdr.workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 1,
                    panes: [
                        PaneAttention(
                            paneId: 4,
                            status: .working,
                            acknowledged: true,
                            lastLine: "running",
                            seq: 1,
                            processName: "codex"
                        ),
                    ]
                ),
                WorkspaceAttention(
                    workspaceId: tmux.workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 1,
                    panes: [
                        PaneAttention(
                            paneId: 9,
                            status: .working,
                            acknowledged: true,
                            lastLine: "running",
                            seq: 2,
                            processName: "pi"
                        ),
                    ]
                ),
            ]
        )

        let items = WorkspaceSidebarProjection.agents(
            workspaces: [herdr, tmux],
            attention: attention
        )

        XCTAssertEqual(items.map(\.title), ["Codex", "pi"])
        XCTAssertEqual(items.map(\.indicator), [.running, .running])
        XCTAssertEqual(items.map(\.workspaceId), [herdr.workspaceId, tmux.workspaceId])
    }

    func testReadStructuredAgentStaysInAgentSidebarButLeavesAttention() {
        let workspaceId = "local@@agents@herdr@w2"
        let workspace = WorkspaceSidebarItem(
            workspaceId: workspaceId,
            name: "muxterm",
            runtime: "herdr",
            transport: "local",
            isActive: false,
            structuredAgents: [
                StructuredPaneAgent(
                    paneId: 4,
                    displayName: "Codex",
                    title: nil,
                    name: "codex",
                    kind: "codex",
                    status: .done
                ),
            ]
        )
        let readPane = PaneAttention(
            paneId: 4,
            status: .done,
            acknowledged: true,
            lastLine: "complete",
            seq: 3,
            processName: "codex"
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 0,
                    panes: [readPane]
                ),
            ]
        )

        let agents = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: attention
        )

        XCTAssertEqual(agents.count, 1)
        XCTAssertEqual(agents[0].indicator, .read)
        XCTAssertTrue(AttentionList.rows(from: attention, query: "").isEmpty)
    }

    func testCommandsSeparateRunningAndUnreadCommandsFromAgents() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            shortcut: 1
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: workspace.workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 2,
                    panes: [
                        PaneAttention(
                            paneId: 1,
                            status: .working,
                            acknowledged: true,
                            lastLine: "building",
                            seq: 1,
                            processName: "cargo test"
                        ),
                        PaneAttention(
                            paneId: 2,
                            status: .done,
                            acknowledged: false,
                            lastLine: "finished",
                            seq: 2,
                            processName: "sleep"
                        ),
                        PaneAttention(
                            paneId: 3,
                            status: .done,
                            acknowledged: true,
                            lastLine: "read",
                            seq: 3,
                            processName: "make"
                        ),
                        PaneAttention(
                            paneId: 4,
                            status: .working,
                            acknowledged: true,
                            lastLine: "codex",
                            seq: 4,
                            processName: "codex"
                        ),
                    ]
                ),
            ]
        )

        let commands = WorkspaceSidebarProjection.commands(
            workspaces: [workspace],
            attention: attention
        )

        XCTAssertEqual(commands.map(\.title), ["cargo test", "sleep"])
        XCTAssertEqual(commands.map(\.paneId), [1, 2])
        XCTAssertEqual(commands.map(\.indicator), [.running, .done])
    }

    func testCoreClassifiedWrapperAgentGoesToAgentsNotCommands() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: workspace.workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 1,
                    panes: [
                        PaneAttention(
                            paneId: 7,
                            status: .working,
                            acknowledged: true,
                            lastLine: "agent",
                            seq: 9,
                            processName: "cursor",
                            processIsAgent: true
                        ),
                    ]
                ),
            ]
        )

        let agents = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: attention
        )
        let commands = WorkspaceSidebarProjection.commands(
            workspaces: [workspace],
            attention: attention
        )

        XCTAssertEqual(agents.map(\.title), ["cursor"])
        XCTAssertTrue(commands.isEmpty)
    }

    func testAgentsSortUnreadBeforeRunningBeforeRead() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: workspace.workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 0,
                    panes: [
                        PaneAttention(
                            paneId: 1,
                            status: .working,
                            acknowledged: true,
                            lastLine: "",
                            seq: 1,
                            processName: "codex",
                            processIsAgent: true
                        ),
                        PaneAttention(
                            paneId: 2,
                            status: .done,
                            acknowledged: false,
                            lastLine: "",
                            seq: 2,
                            processName: "droid",
                            processIsAgent: true
                        ),
                        PaneAttention(
                            paneId: 3,
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 3,
                            processName: "amp",
                            processIsAgent: true
                        ),
                    ]
                ),
            ]
        )

        let agents = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: attention
        )

        XCTAssertEqual(agents.map(\.title), ["droid", "codex", "amp"])
        XCTAssertEqual(agents.map(\.indicator), [.done, .running, .read])
    }

    func testOrdinaryCommandDoesNotBecomePermanentAgent() {
        let workspaceId = "local@@dev@tmux@dev"
        let workspace = WorkspaceSidebarItem(
            workspaceId: workspaceId,
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 1,
                    panes: [
                        PaneAttention(
                            paneId: 1,
                            status: .working,
                            acknowledged: true,
                            lastLine: "building",
                            seq: 4,
                            processName: "cargo"
                        ),
                    ]
                ),
            ]
        )

        XCTAssertTrue(
            WorkspaceSidebarProjection.agents(workspaces: [workspace], attention: attention).isEmpty
        )
        XCTAssertEqual(AttentionList.rows(from: attention, query: "").count, 1)
    }
}
