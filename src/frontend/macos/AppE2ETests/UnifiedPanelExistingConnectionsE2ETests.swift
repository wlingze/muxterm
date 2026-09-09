import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Workspaces 根列表必须提供与 Linux 对齐的 Existing Connections 入口，
/// 且选择已有 session 只能 attach，不能进入 Project 的 create fallback。
final class UnifiedPanelExistingConnectionsE2ETests: XCTestCase {
    func testCatalogRuntimeListExposesHerdrForProjectChoices() throws {
        let runtimes = try CoreBridge.runtimeCatalog()
        XCTAssertEqual(runtimes.compactMap { TargetRuntime(rawValue: $0.id) }, [.tmux, .herdr, .shell])

        AppE2E.ensureApp()
        let window = TargetConfigWindow(
            owner: nil,
            store: QuickConnectStore(),
            sshHosts: [],
            availableRuntimes: runtimes.compactMap { TargetRuntime(rawValue: $0.id) }
        )
        defer { window.close() }
        XCTAssertEqual(window.testAvailableRuntimes(), [.tmux, .herdr, .shell])
    }

    func testCatalogHerdrCandidateMapsTypedIdentityWithoutParsingID() {
        let candidate = CoreWorkspaceCandidate(
            id: "opaque-display-id",
            name: "muxterm",
            runtime: "herdr",
            transport: "ssh",
            target: "buildbox",
            session: "agents",
            socket: "/remote/.config/herdr/sessions/agents/herdr.sock",
            workspaceID: "w7"
        )

        XCTAssertEqual(candidate.targetConfig, TargetConfig(
            name: "muxterm",
            runtime: .herdr,
            transport: .ssh(name: "buildbox"),
            path: "",
            session: "agents",
            socket: "/remote/.config/herdr/sessions/agents/herdr.sock",
            workspaceID: "w7"
        ))
    }

    func testExistingRowUsesAttachCallbackInsteadOfProjectConnect() {
        AppE2E.ensureApp()
        let choice = ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(name: "attach-only", windowCount: 2, attached: false),
            socket: "muxterm-test-attach-only"
        )
        let panel = makePanel()
        var projectConnectCount = 0
        var attached: ExistingConnectionChoice?
        panel.onConnect = { _ in projectConnectCount += 1 }
        panel.onLoadExistingConnections = { completion in completion(.success([choice])) }
        panel.onAttachExistingConnection = { attached = $0 }
        panel.present()

        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.existingConnections))
        panel.testActivateWorkspaceItem(matching: choice.session.name)

        XCTAssertEqual(attached, choice)
        XCTAssertEqual(projectConnectCount, 0, "Existing 行不得进入 Project create-if-missing 流程")
        panel.dismiss()
    }

    func testHerdrExistingRowKeepsIdentityAndUsesAttachOnlyCallback() {
        AppE2E.ensureApp()
        let config = TargetConfig(
            name: "muxterm",
            runtime: .herdr,
            transport: .local,
            path: "",
            session: "agents",
            socket: "/tmp/muxterm-test-agents/herdr.sock",
            workspaceID: "w7"
        )
        let choice = ExistingConnectionChoice(config: config)
        let panel = makePanel()
        var attached: ExistingConnectionChoice?
        var projectConnectCount = 0
        panel.onConnect = { _ in projectConnectCount += 1 }
        panel.onLoadExistingConnections = { completion in completion(.success([choice])) }
        panel.onAttachExistingConnection = { attached = $0 }
        panel.present()

        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.existingConnections))
        panel.testSetQuery("w7")
        XCTAssertTrue(panel.testWorkspaceTitles().contains("muxterm"))
        panel.testSetQuery("")
        panel.testActivateWorkspaceItem(matching: "muxterm")

        XCTAssertEqual(attached?.config, config)
        XCTAssertEqual(projectConnectCount, 0)
        panel.dismiss()
    }

    func testLateDiscoveryCannotReplaceWorkspaceRootAfterBack() {
        AppE2E.ensureApp()
        let panel = makePanel()
        var completion: ((Result<[ExistingConnectionChoice], Error>) -> Void)?
        panel.onLoadExistingConnections = { completion = $0 }
        panel.present()
        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.existingConnections))
        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.existingBack))

        completion?(.success([ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(name: "late-session", windowCount: 1, attached: false),
            socket: nil
        )]))
        AppE2E.pump(40)

        XCTAssertFalse(panel.testWorkspaceShowsExistingConnections())
        XCTAssertEqual(panel.testWorkspaceTitles().first, MuxtermI18n.shared.tr(.existingConnections))
        XCTAssertFalse(panel.testWorkspaceTitles().contains("late-session"))
        panel.dismiss()
    }

    func testConnectedWorkspacesAreSearchableBeforeExistingDiscoveryReturns() {
        AppE2E.ensureApp()
        let connected = TargetConfig(
            name: "saved-agents",
            runtime: .herdr,
            transport: .ssh(name: "buildbox"),
            path: "/srv/project",
            session: "agents",
            socket: "/remote/herdr.sock",
            workspaceID: "w7"
        )
        let panel = UnifiedPanelController(
            store: QuickConnectStore(),
            ownerWindow: nil,
            snapshot: { nil },
            paneOutput: { _ in Data() },
            sendInput: { _, _ in },
            search: { _, _ in [] },
            connectedWorkspaces: { [connected] }
        )
        var discoveryCompletion: ((Result<[ExistingConnectionChoice], Error>) -> Void)?
        panel.onLoadExistingConnections = { discoveryCompletion = $0 }
        var attached: TargetConfig?
        panel.onConnect = { attached = $0 }

        panel.present(initial: .workspaces)
        panel.testSetQuery("w7")

        XCTAssertTrue(
            panel.testWorkspaceTitles().contains("saved-agents"),
            "已连接 Workspace 必须在异步 Existing discovery 返回前可搜索"
        )
        panel.activateForTest()
        XCTAssertEqual(attached, connected, "Enter 必须连接当前过滤后的同一条目")

        discoveryCompletion?(.success([ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(name: "other", windowCount: 1, attached: false),
            socket: nil
        )]))
        AppE2E.pump(40)
        XCTAssertTrue(
            panel.testWorkspaceTitles().contains("saved-agents"),
            "Existing discovery 追加结果时不得覆盖已连接 Workspace"
        )
        panel.dismiss()
    }

    func testConnectedWorkspaceSearchCoversPathRuntimeAliasSessionAndIdentity() {
        AppE2E.ensureApp()
        let connected = TargetConfig(
            name: "saved-agents",
            runtime: .herdr,
            transport: .ssh(name: "buildbox"),
            path: "/srv/project",
            session: "agents",
            socket: "/remote/herdr.sock",
            workspaceID: "w7"
        )
        let panel = UnifiedPanelController(
            store: QuickConnectStore(),
            ownerWindow: nil,
            snapshot: { nil },
            paneOutput: { _ in Data() },
            sendInput: { _, _ in },
            search: { _, _ in [] },
            connectedWorkspaces: { [connected] }
        )
        panel.present(initial: .workspaces)

        for token in ["/srv/project", "herdr", "buildbox", "agents", "w7"] {
            panel.testSetQuery(token)
            XCTAssertTrue(
                panel.testWorkspaceTitles().contains("saved-agents"),
                "Workspace 搜索必须支持字段 (token)"
            )
        }
        panel.dismiss()
    }

    func testExistingConnectionsDiscoversAndAttachesIsolatedLocalSession() throws {
        let fixture = OnePaneCat(label: "panel-existing")
        let extraSession = "existing-extra-\(ProcessInfo.processInfo.processIdentifier)"
        let extraToken = "EXISTING_ATTACH_TOKEN_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.ok(socket: fixture.socket, args: [
            "new-session", "-d", "-s", extraSession,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        let extraPane = Tmux.out(
            socket: fixture.socket,
            args: ["list-panes", "-t", extraSession, "-F", "#{pane_id}"]
        )
        Tmux.sendLiteral(socket: fixture.socket, target: extraPane, text: extraToken)
        Tmux.waitCapture(socket: fixture.socket, target: extraPane, needle: extraToken)

        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        app.openQuickConnect()
        AppE2E.pump(80)
        XCTAssertEqual(
            app.unifiedPanel.testWorkspaceTitles().first,
            MuxtermI18n.shared.tr(.existingConnections),
            "Workspaces 根列表第一行必须是 Existing Connections"
        )

        app.unifiedPanel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.existingConnections))
        XCTAssertTrue(app.unifiedPanel.testWorkspaceShowsExistingConnections())
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                AppE2E.pump(40)
                return app.unifiedPanel.testWorkspaceTitles().contains(extraSession)
            },
            "已有连接列表必须发现当前隔离 socket 上的 session；titles=\(app.unifiedPanel.testWorkspaceTitles())"
        )
        XCTAssertTrue(app.unifiedPanel.testIsPresented(), "异步发现完成后面板不能关闭")

        app.unifiedPanel.testActivateWorkspaceItem(matching: extraSession)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.attachTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testActiveWorkspaceSession() == extraSession
                    && app.testAllVisibleTerminalText().contains(extraToken)
            },
            "选择 Existing session 必须直接 attach 并显示其 Surface"
        )
        XCTAssertFalse(app.unifiedPanel.testIsPresented())

        let sessions = Tmux.out(
            socket: fixture.socket,
            args: ["list-sessions", "-F", "#{session_name}"]
        ).split(whereSeparator: \.isNewline)
        XCTAssertEqual(sessions.count, 2, "Existing attach 不得额外创建 Project session")
    }

    private func makePanel() -> UnifiedPanelController {
        UnifiedPanelController(
            store: QuickConnectStore(),
            ownerWindow: nil,
            snapshot: { nil },
            paneOutput: { _ in Data() },
            sendInput: { _, _ in },
            search: { _, _ in [] }
        )
    }
}
