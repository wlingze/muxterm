import XCTest
@testable import MuxtermChrome

final class AggregateWorkspaceModelTests: XCTestCase {
    func testShellTabsAlwaysPutEveryLocalTabBeforeRemoteMachines() {
        let tabs = AggregateWorkspaceProjection.shellTabs(sources: [
            ShellAggregateSource(
                workspaceId: "ssh@@ryzen",
                transport: "ryzen",
                openedOrder: 1,
                tabs: [AggregateSourceTab(id: 8, title: "remote")]
            ),
            ShellAggregateSource(
                workspaceId: "local@@shell",
                transport: "local",
                openedOrder: 9,
                tabs: [
                    AggregateSourceTab(id: 2, title: "notes"),
                    AggregateSourceTab(id: 4, title: "scratch"),
                ]
            ),
            ShellAggregateSource(
                workspaceId: "ssh@@build",
                transport: "build",
                openedOrder: 2,
                tabs: [AggregateSourceTab(id: 3, title: "build")]
            ),
        ])

        XCTAssertEqual(tabs.map(\.workspaceId), [
            "local@@shell", "local@@shell", "ssh@@ryzen", "ssh@@build",
        ])
        XCTAssertEqual(tabs.map(\.sourceTabId), [2, 4, 8, 3])
        XCTAssertEqual(tabs.map(\.displayId), [1, 2, 3, 4])
        XCTAssertEqual(tabs.first?.title, "local · notes")
    }

    func testAgentTabsUseStableWorkspaceAndTopologyOrderInsteadOfStatusOrder() {
        let agents = [
            AgentSidebarItem(
                workspaceId: "second",
                tabId: 22,
                paneId: 9,
                title: "server · ryzen",
                detail: "done · Codex · Tab 2",
                indicator: .done,
                agentName: "Codex",
                sessionTitle: "Fix build",
                tabNumber: 2
            ),
            AgentSidebarItem(
                workspaceId: "first",
                tabId: 11,
                paneId: 5,
                title: "muxterm · local",
                detail: "working · Grok · Tab 1",
                indicator: .working,
                agentName: "Grok",
                sessionTitle: "Review UI",
                tabNumber: 1
            ),
            AgentSidebarItem(
                workspaceId: "first",
                tabId: 11,
                paneId: 6,
                title: "muxterm · local",
                detail: "working · Claude · Tab 1",
                indicator: .working,
                agentName: "Claude",
                sessionTitle: "Pair review",
                tabNumber: 1
            ),
        ]

        let tabs = AggregateWorkspaceProjection.agentTabs(
            agents: agents,
            workspaceOrder: ["first", "second"]
        )

        XCTAssertEqual(tabs.map(\.workspaceId), ["first", "second"])
        XCTAssertEqual(tabs.map(\.sourceTabId), [11, 22])
        XCTAssertEqual(tabs.map(\.agentPaneIds), [[5, 6], [9]])
        XCTAssertEqual(tabs.map(\.title), [
            "muxterm · local · Grok, Claude · Review UI / Pair review",
            "server · ryzen · Codex · Fix build",
        ])
        XCTAssertEqual(tabs.map(\.displayId), [1, 2])
    }

    func testAgentTabIdentityIncludesWorkspaceBecauseTabIdsRepeat() {
        let first = AgentAggregateKey(workspaceId: "one", sourceTabId: 1)
        let second = AgentAggregateKey(workspaceId: "two", sourceTabId: 1)
        let anotherTab = AgentAggregateKey(workspaceId: "one", sourceTabId: 2)

        XCTAssertNotEqual(first, second)
        XCTAssertNotEqual(first, anotherTab)
    }

    func testDefaultAggregateShortcutsAndConfigActions() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, control: true, key: "s")),
            .openShells
        )
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, control: true, key: "a")),
            .openAgents
        )
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, key: "k")))
        XCTAssertEqual(KeyBindingsConfig.action(from: "open_shells"), .openShells)
        XCTAssertEqual(KeyBindingsConfig.action(from: "open_agents"), .openAgents)
    }
}
