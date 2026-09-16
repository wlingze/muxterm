import XCTest
@testable import MuxtermChrome

final class WorkspaceSidebarModelTests: XCTestCase {
    func testWorkspaceShortcutIndexesUseOpenedOrderAndStopAtNine() {
        let ids = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"]

        XCTAssertEqual(
            WorkspaceShortcutIndex.byWorkspaceID(ids),
            [
                "one": 1,
                "two": 2,
                "three": 3,
                "four": 4,
                "five": 5,
                "six": 6,
                "seven": 7,
                "eight": 8,
                "nine": 9,
            ]
        )
        XCTAssertNil(WorkspaceShortcutIndex.byWorkspaceID(ids)["ten"])
    }

    func testCmdCtrlZeroSwitchesLastWorkspaceAndCmdZeroResetsFont() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, control: true, key: "0")),
            .switchWorkspace(0)
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, control: true, key: "9")),
            .switchWorkspace(9)
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "0")),
            .resetFontSize
        )
    }

    func testStructuredAgentRegistryKeepsReadIdentityUntilPaneCloses() {
        let workspaceId = "local@@agents@herdr@w2"
        var registry = StructuredAgentRegistry()
        registry.observe(
            workspaceId: workspaceId,
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

        registry.observe(workspaceId: workspaceId, paneId: 4, agent: nil)
        XCTAssertEqual(registry.snapshot(workspaceId: workspaceId).count, 1)
        XCTAssertEqual(registry.snapshot(workspaceId: workspaceId)[0].displayName, "Codex")
        XCTAssertEqual(registry.snapshot(workspaceId: workspaceId)[0].status, .unknown)
        let workspace = WorkspaceSidebarItem(
            workspaceId: workspaceId,
            name: "muxterm",
            runtime: "herdr",
            transport: "local",
            isActive: true,
            structuredAgents: registry.snapshot(workspaceId: workspaceId)
        )
        let projected = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: nil
        )
        XCTAssertEqual(projected.count, 1)
        XCTAssertEqual(projected[0].title, "muxterm")
        XCTAssertEqual(projected[0].detail, "idle · Codex")
        XCTAssertEqual(projected[0].indicator, .idle)

        registry.removePane(workspaceId: workspaceId, paneId: 4)
        XCTAssertTrue(registry.snapshot(workspaceId: workspaceId).isEmpty)
    }

    func testStructuredAgentRegistryRejectsOlderVersion() {
        let workspaceId = "local@@agents@herdr@w2"
        var registry = StructuredAgentRegistry()
        registry.observe(
            workspaceId: workspaceId,
            paneId: 4,
            agent: StructuredPaneAgent(
                paneId: 4,
                displayName: "Codex",
                title: nil,
                name: "codex",
                kind: "codex",
                status: .working,
                stateChangeSeq: 11,
                revision: 11
            )
        )
        registry.observe(
            workspaceId: workspaceId,
            paneId: 4,
            agent: StructuredPaneAgent(
                paneId: 4,
                displayName: "Codex",
                title: nil,
                name: "codex",
                kind: "codex",
                status: .idle,
                stateChangeSeq: 10,
                revision: 99
            )
        )
        XCTAssertEqual(registry.snapshot(workspaceId: workspaceId)[0].status, .working)

        registry.observe(
            workspaceId: workspaceId,
            paneId: 4,
            agent: StructuredPaneAgent(
                paneId: 4,
                displayName: "Codex",
                title: nil,
                name: "codex",
                kind: "codex",
                status: .idle,
                stateChangeSeq: 12,
                revision: 1
            )
        )
        XCTAssertEqual(registry.snapshot(workspaceId: workspaceId)[0].status, .idle)
    }

    /// paneId 跨 Workspace 会重复；只按 paneId 缓存会把 legion 的 Codex 串到 muxterm。
    func testStructuredAgentRegistryIsolatesSamePaneIdAcrossWorkspaces() {
        var registry = StructuredAgentRegistry()
        registry.observe(
            workspaceId: "legion@local",
            paneId: 1,
            agent: StructuredPaneAgent(
                paneId: 1,
                displayName: "Codex",
                title: nil,
                name: "codex",
                kind: "codex",
                status: .working
            )
        )
        registry.observe(
            workspaceId: "muxterm@local",
            paneId: 1,
            agent: nil
        )

        XCTAssertEqual(
            registry.snapshot(workspaceId: "legion@local").map(\.name),
            ["codex"]
        )
        XCTAssertTrue(
            registry.snapshot(workspaceId: "muxterm@local").isEmpty,
            "muxterm 未上报 agent 时不得继承 legion 同 paneId 的 Codex"
        )

        let muxterm = WorkspaceSidebarItem(
            workspaceId: "muxterm@local",
            name: "muxterm",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            structuredAgents: registry.snapshot(workspaceId: "muxterm@local"),
            tabNumberByPane: [1: 1, 2: 2, 3: 3]
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: muxterm.workspaceId,
                    blocked: 0,
                    done: 0,
                    working: 0,
                    panes: [
                        PaneAttention(
                            paneId: 1,
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 1,
                            processName: "zsh"
                        ),
                        PaneAttention(
                            paneId: 3,
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 2,
                            processName: "zsh"
                        ),
                    ]
                ),
            ]
        )
        XCTAssertTrue(
            WorkspaceSidebarProjection.agents(workspaces: [muxterm], attention: attention).isEmpty,
            "空闲 Tab1/Tab3 不得凭跨 Workspace paneId 碰撞冒出 Codex"
        )
    }

    func testUnknownStructuredStatusUsesAttentionIndicator() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@agents@herdr@w2",
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
                    status: .unknown
                ),
            ]
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
                            paneId: 4,
                            status: .working,
                            acknowledged: true,
                            lastLine: "running",
                            seq: 1,
                            processName: "codex"
                        ),
                    ]
                ),
            ]
        )

        let item = try! XCTUnwrap(
            WorkspaceSidebarProjection.agents(
                workspaces: [workspace],
                attention: attention
            ).first
        )
        XCTAssertEqual(item.detail, "working · Codex")
        XCTAssertEqual(item.indicator, .working)
    }

    func testAgentStatusFollowsAttentionWhenStructuredStaysWorking() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "muxterm",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            structuredAgents: [
                StructuredPaneAgent(
                    paneId: 4,
                    displayName: "Codex",
                    title: nil,
                    name: "codex",
                    kind: "codex",
                    status: .working
                ),
            ]
        )
        let attention = AttentionSnapshot(
            blockedCount: 0,
            workspaces: [
                WorkspaceAttention(
                    workspaceId: workspace.workspaceId,
                    blocked: 0,
                    done: 1,
                    working: 0,
                    panes: [
                        PaneAttention(
                            paneId: 4,
                            status: .done,
                            acknowledged: false,
                            lastLine: "complete",
                            seq: 2,
                            processName: "codex"
                        ),
                    ]
                ),
            ]
        )
        let item = try! XCTUnwrap(
            WorkspaceSidebarProjection.agents(
                workspaces: [workspace],
                attention: attention
            ).first
        )
        XCTAssertEqual(item.detail, "done · Codex")
        XCTAssertEqual(item.indicator, .done)
    }

    func testSidebarTargetsCarryStableTabIDs() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            structuredAgents: [
                StructuredPaneAgent(
                    paneId: 4,
                    displayName: "Codex",
                    title: nil,
                    name: "codex",
                    kind: "codex",
                    status: .working
                ),
            ],
            tabNumberByPane: [4: 2, 8: 3],
            tabIdByPane: [4: 42, 8: 84]
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
                            paneId: 4,
                            status: .working,
                            lastLine: "agent",
                            seq: 1,
                            processName: "codex"
                        ),
                        PaneAttention(
                            paneId: 8,
                            status: .working,
                            lastLine: "command",
                            seq: 2,
                            processName: "cargo"
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

        XCTAssertEqual(agents.map(\.tabId), [42])
        XCTAssertEqual(commands.map(\.tabId), [84])
    }

    func testProjectsStructuredHerdrAndTmuxAgentsAcrossWorkspaces() {
        let herdr = WorkspaceSidebarItem(
            workspaceId: "local@@agents@herdr@w2",
            name: "muxterm",
            runtime: "herdr",
            transport: "local",
            isActive: true,
            shortcut: 1,
            structuredAgents: [
                StructuredPaneAgent(
                    paneId: 4,
                    displayName: "Codex",
                    title: "Review muxterm",
                    name: "codex",
                    kind: "codex",
                    status: .working
                ),
            ],
            tabNumberByPane: [4: 2]
        )
        let tmux = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: false,
            shortcut: 2,
            tabNumberByPane: [9: 1]
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
                            processName: "pi",
                            processIsAgent: true,
                            agentName: "pi"
                        ),
                    ]
                ),
            ]
        )

        let items = WorkspaceSidebarProjection.agents(
            workspaces: [herdr, tmux],
            attention: attention
        )

        // 同状态块内按 workspace shortcut 固定：muxterm(1) 在前，dev(2) 在后。
        XCTAssertEqual(items.map(\.title), ["muxterm", "dev"])
        XCTAssertEqual(items.map(\.detail), [
            "working · Codex · Tab 2",
            "working · pi · Tab 1",
        ])
        XCTAssertEqual(items.map(\.indicator), [.working, .working])
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
            ],
            tabNumberByPane: [4: 2]
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
        XCTAssertEqual(agents[0].title, "muxterm")
        XCTAssertEqual(agents[0].detail, "done · Codex · Tab 2")
        XCTAssertEqual(agents[0].tabNumber, 2)
        XCTAssertFalse(agents[0].detail.localizedCaseInsensitiveContains("pane"))
        XCTAssertEqual(agents[0].indicator, .idle)
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
                            processName: "codex",
                            processIsAgent: true,
                            agentName: "codex"
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
        XCTAssertEqual(commands.map(\.indicator), [.working, .done])
    }

    func testCoreClassifiedWrapperAgentGoesToAgentsNotCommands() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            tabNumberByPane: [7: 1]
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

        XCTAssertEqual(agents.map(\.title), ["dev"])
        XCTAssertEqual(agents.map(\.detail), ["working · cursor · Tab 1"])
        XCTAssertTrue(commands.isEmpty)
    }

    func testAgentsSortByStatusBlocksThenStableTabPane() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "local@@dev@tmux@dev",
            name: "dev",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            shortcut: 1,
            tabNumberByPane: [1: 1, 2: 1, 3: 2, 4: 3]
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
                        PaneAttention(
                            paneId: 4,
                            status: .working,
                            acknowledged: true,
                            lastLine: "",
                            seq: 4,
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

        // 状态成块；块内按 tab/pane 固定。
        XCTAssertEqual(agents.map(\.indicator), [.done, .working, .working, .idle])
        XCTAssertEqual(agents.map(\.agentName), ["droid", "codex", "cursor", "amp"])
        XCTAssertEqual(agents.map(\.paneId), [2, 1, 4, 3])

        // 仅状态变化：条目在块间移动，同块内相对顺序不变。
        let flipped = AttentionSnapshot(
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
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 5,
                            processName: "codex",
                            processIsAgent: true
                        ),
                        PaneAttention(
                            paneId: 2,
                            status: .done,
                            acknowledged: false,
                            lastLine: "",
                            seq: 6,
                            processName: "droid",
                            processIsAgent: true
                        ),
                        PaneAttention(
                            paneId: 3,
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 7,
                            processName: "amp",
                            processIsAgent: true
                        ),
                        PaneAttention(
                            paneId: 4,
                            status: .working,
                            acknowledged: true,
                            lastLine: "",
                            seq: 8,
                            processName: "cursor",
                            processIsAgent: true
                        ),
                    ]
                ),
            ]
        )
        let after = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: flipped
        )
        XCTAssertEqual(after.map(\.indicator), [.done, .working, .idle, .idle])
        XCTAssertEqual(after.map(\.agentName), ["droid", "cursor", "codex", "amp"])
        XCTAssertEqual(after.map(\.paneId), [2, 4, 1, 3])
    }

    func testCursorProcessIsNotLabeledCodexAndIdleShellStaysOutOfAgents() {
        let workspace = WorkspaceSidebarItem(
            workspaceId: "muxterm@local",
            name: "muxterm",
            runtime: "tmux",
            transport: "local",
            isActive: true,
            shortcut: 1,
            tabNumberByPane: [10: 1, 20: 2, 30: 3]
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
                            paneId: 10,
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 1,
                            processName: "zsh"
                        ),
                        PaneAttention(
                            paneId: 20,
                            status: .working,
                            acknowledged: true,
                            lastLine: "ctrl+c to stop",
                            seq: 2,
                            processName:
                                "node /Users/x/.codex/vendor/cursor-agent/versions/1/index.js",
                            processIsAgent: true,
                            agentName: "cursor"
                        ),
                        PaneAttention(
                            paneId: 30,
                            status: .idle,
                            acknowledged: true,
                            lastLine: "",
                            seq: 3,
                            processName: "zsh"
                        ),
                    ]
                ),
            ]
        )

        let agents = WorkspaceSidebarProjection.agents(
            workspaces: [workspace],
            attention: attention
        )
        XCTAssertEqual(agents.count, 1)
        XCTAssertEqual(agents[0].paneId, 20)
        XCTAssertEqual(agents[0].agentName, "cursor")
        XCTAssertEqual(agents[0].tabNumber, 2)
        XCTAssertFalse(agents[0].detail.localizedCaseInsensitiveContains("codex"))
    }

    func testPanelJumpRoutingRequiresMatchingWorkspaceForSharedPaneId() {
        let yaklang = "local@@yaklang@tmux@yaklang"
        let muxterm = "local@@muxterm@tmux@muxterm"
        let scenes = [
            PanelJumpRouting.ScenePaneIndex(
                workspaceIds: [yaklang],
                paneIds: [151, 152]
            ),
            PanelJumpRouting.ScenePaneIndex(
                workspaceIds: [muxterm],
                paneIds: [151, 153]
            ),
        ]

        XCTAssertNotNil(
            PanelJumpRouting.targetSceneContainsPane(
                workspaceId: yaklang,
                paneId: 151,
                scenes: scenes
            )
        )
        XCTAssertNil(
            PanelJumpRouting.targetSceneContainsPane(
                workspaceId: yaklang,
                paneId: 153,
                scenes: scenes
            ),
            "同 paneId 在别的 workspace 时不得误命中"
        )
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
