import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Open 先选机器，再列出这台机器上的 tmux/herdr；新路径在面板里补全。
final class UnifiedPanelLauncherE2ETests: XCTestCase {
    func testOpenFiltersMachinesThenListsSessionsOnThatMachine() {
        AppE2E.ensureApp()
        let panel = makePanel()
        var requestedHost: String?
        let choice = ExistingConnectionChoice(
            target: .ssh(SSHHostInfo(alias: "buildbox", hostname: "", user: nil, port: nil)),
            session: TmuxSessionInfo(name: "agents", windowCount: 1, attached: false),
            socket: nil
        )
        var attached: ExistingConnectionChoice?
        panel.onLoadSSHAliases = { completion in
            completion(.success(["ryzen", "buildbox"]))
        }
        panel.onLoadHostSessions = { host, completion in
            requestedHost = host
            completion(.success([choice]))
        }
        panel.onAttachExistingConnection = { attached = $0 }
        panel.present(initial: .workspaces)

        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.panelOpen))
        XCTAssertTrue(panel.testShowsSSHHostPicker())
        XCTAssertTrue(panel.testWorkspaceTitles().contains("local"))
        XCTAssertTrue(panel.testWorkspaceTitles().contains("ryzen"))
        panel.testSetQuery("bu")
        panel.testCompleteSSHHost()
        XCTAssertEqual(panel.testQuery(), "buildbox")
        panel.activateForTest()

        XCTAssertEqual(requestedHost, "buildbox")
        XCTAssertTrue(panel.testShowsHostSessions())
        XCTAssertEqual(
            panel.testWorkspaceTitles().dropFirst().first,
            MuxtermI18n.shared.tr(.panelOpenNewPath)
        )
        panel.testActivateWorkspaceItem(matching: "agents")
        XCTAssertEqual(attached, choice)
        panel.dismiss()
    }

    func testOpenNewPathCompletesDirectoriesAndConfirms() {
        AppE2E.ensureApp()
        let panel = makePanel()
        var opened: TargetConfig?
        panel.onLoadSSHAliases = { completion in
            completion(.success(["ryzen", "buildbox"]))
        }
        panel.onListDirectories = { host, path, completion in
            XCTAssertEqual(host, "ryzen")
            if path == "~" || path == "~/" {
                completion(.success(["Developer", "notes"]))
            } else {
                completion(.success(["muxterm"]))
            }
        }
        panel.onConnect = { opened = $0 }
        panel.present(initial: .workspaces)
        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.panelOpen))
        panel.testActivateWorkspaceItem(matching: "ryzen")
        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.panelOpenNewPath))

        XCTAssertTrue(panel.testShowsPathPicker())
        XCTAssertEqual(
            panel.testWorkspaceTitles().dropFirst().first,
            MuxtermI18n.shared.tr(.panelOpenThisDirectory)
        )
        panel.testSetQuery("~/D")
        XCTAssertTrue(panel.testWorkspaceTitles().contains("Developer"))
        XCTAssertFalse(panel.testWorkspaceTitles().contains("notes"))
        panel.testSelectRow(offset: 1)
        panel.testCompleteSSHHost()
        XCTAssertTrue(panel.testShowsPathPicker(), "Tab 补全目录后仍停留在路径选择")
        XCTAssertEqual(panel.testQuery(), "~/Developer/")

        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.panelOpenThisDirectory))
        XCTAssertEqual(opened?.runtime, .herdr)
        XCTAssertEqual(opened?.name, "Developer")
        if case .ssh(let host) = opened?.transport {
            XCTAssertEqual(host, "ryzen")
        } else {
            XCTFail("SSH 新路径必须记在所选机器上")
        }
        XCTAssertNil(opened?.persistedID)
    }

    func testRootListsOnlyOpenProjectsAndWorkspacesByFrequency() {
        AppE2E.ensureApp()
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-picker-\(UUID().uuidString)", isDirectory: true)
        let store = QuickConnectStore(
            recentsURL: directory.appendingPathComponent("workspace-recents.json")
        ) { _ in }
        defer { try? FileManager.default.removeItem(at: directory) }
        let rare = TargetConfig(name: "notes", runtime: .tmux, transport: .local, path: "~/notes")
        let frequent = TargetConfig(
            name: "muxterm",
            runtime: .herdr,
            transport: .ssh(name: "ryzen"),
            path: "/home/wlz/muxterm"
        )
        store.recordRecent(rare)
        store.recordRecent(frequent)
        store.recordAttach(frequent)
        store.recordAttach(frequent)
        store.upsertProject(frequent)
        let panel = UnifiedPanelController(
            store: store,
            ownerWindow: nil,
            snapshot: { nil },
            sendInput: { _, _ in },
            search: { _ in }
        )
        panel.present(initial: .workspaces)
        defer { panel.dismiss() }

        XCTAssertEqual(
            panel.testWorkspaceTitles(),
            [
                MuxtermI18n.shared.tr(.panelOpen),
                "muxterm",
                "notes",
            ]
        )
        XCTAssertFalse(panel.testWorkspaceTitles().contains(MuxtermI18n.shared.tr(.panelNewProject)))
        XCTAssertFalse(panel.testWorkspaceTitles().contains(MuxtermI18n.shared.tr(.panelOpenFolder)))
        XCTAssertFalse(panel.testWorkspaceTitles().contains(MuxtermI18n.shared.tr(.panelConnectSSH)))
        let open = panel.testOpenRowSymbol()
        let workspace = panel.testWorkspaceCell(at: 1)
        workspace?.layoutSubtreeIfNeeded()
        XCTAssertEqual(open?.symbol, "+")
        XCTAssertEqual(open?.emphasized, true)
        XCTAssertEqual(
            open?.symbolMinX ?? -1,
            workspace?.testIndexFrame.minX ?? -2,
            accuracy: 0.5
        )
        XCTAssertEqual(
            open?.titleMinX ?? -1,
            workspace?.testTitleFrame.minX ?? -2,
            accuracy: 0.5
        )
    }

    func testSearchingOpenSelectsTheOpenAction() {
        AppE2E.ensureApp()
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-picker-\(UUID().uuidString)", isDirectory: true)
        let store = QuickConnectStore(
            recentsURL: directory.appendingPathComponent("workspace-recents.json")
        ) { _ in }
        defer { try? FileManager.default.removeItem(at: directory) }
        let target = TargetConfig(
            name: "openapi-server",
            runtime: .tmux,
            transport: .local,
            path: "~/openapi"
        )
        store.recordRecent(target)
        store.upsertProject(target)
        let panel = UnifiedPanelController(
            store: store,
            ownerWindow: nil,
            snapshot: { nil },
            sendInput: { _, _ in },
            search: { _ in }
        )
        panel.present(initial: .workspaces)
        defer { panel.dismiss() }

        panel.testSetQuery("open")
        XCTAssertEqual(
            panel.testWorkspaceTitles().first,
            MuxtermI18n.shared.tr(.panelOpen),
            "输入 open 时动作行必须排在最前"
        )
        XCTAssertEqual(
            panel.testSelectedWorkspaceTitle(),
            MuxtermI18n.shared.tr(.panelOpen),
            "搜索 open 就应该选中 Open"
        )
        XCTAssertTrue(
            panel.testWorkspaceTitles().contains("openapi-server"),
            "路径模糊命中的 Project 仍要保留在结果里"
        )

        panel.testSetQuery("openapi")
        XCTAssertEqual(panel.testWorkspaceTitles().first, "openapi-server")
    }

    private func makePanel() -> UnifiedPanelController {
        UnifiedPanelController(
            store: QuickConnectStore(),
            ownerWindow: nil,
            snapshot: { nil },
            sendInput: { _, _ in },
            search: { _ in }
        )
    }
}
