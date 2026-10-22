import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// Open Folder 的主机选择留在 Quick Panel：输入筛选、方向键选择、Tab 补全、Enter 确认。
/// 分组标题比工作区矮，并且不能被选中或变成孤立搜索结果。
final class UnifiedPanelLauncherE2ETests: XCTestCase {
    func testConnectSSHUsesPanelCompletionInsteadOfLeavingThePanel() {
        AppE2E.ensureApp()
        let panel = makePanel()
        var opened: TargetTransport?
        panel.onOpenFolder = { opened = $0 }
        panel.onLoadSSHAliases = { completion in
            completion(.success(["ryzen", "buildbox"]))
        }
        panel.present(initial: .workspaces)

        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.panelConnectSSH))
        XCTAssertTrue(panel.testIsPresented())
        XCTAssertTrue(panel.testShowsSSHHostPicker())
        XCTAssertEqual(panel.testWorkspaceTitles().first, MuxtermI18n.shared.tr(.existingBack))
        XCTAssertTrue(panel.testWorkspaceTitles().contains("ryzen"))
        XCTAssertTrue(panel.testWorkspaceTitles().contains("buildbox"))

        panel.testSetQuery("bu")
        panel.testCompleteSSHHost()
        XCTAssertEqual(panel.testQuery(), "buildbox")
        panel.activateForTest()

        guard case .ssh(let host) = opened else {
            return XCTFail("Enter 必须打开选中的 SSH 主机")
        }
        XCTAssertEqual(host, "buildbox")
        XCTAssertFalse(panel.testIsPresented())
    }

    func testTypedSSHHostCanBeConfirmedWhenItIsNotAnAlias() {
        AppE2E.ensureApp()
        let panel = makePanel()
        var opened: TargetTransport?
        panel.onOpenFolder = { opened = $0 }
        panel.onLoadSSHAliases = { completion in
            completion(.success(["ryzen"]))
        }
        panel.present(initial: .workspaces)
        panel.testActivateWorkspaceItem(matching: MuxtermI18n.shared.tr(.panelConnectSSH))
        panel.testSetQuery("custom.example")

        let typed = MuxtermI18n.shared.tr(.panelUseSSHHost, arguments: ["host": "custom.example"])
        XCTAssertTrue(panel.testWorkspaceTitles().contains(typed))
        panel.testActivateWorkspaceItem(matching: typed)

        guard case .ssh(let host) = opened else {
            return XCTFail("手工输入的主机必须可以确认")
        }
        XCTAssertEqual(host, "custom.example")
    }

    func testHostHeadersStayCompactAndOutOfSearch() {
        AppE2E.ensureApp()
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-launcher-\(UUID().uuidString)", isDirectory: true)
        let store = QuickConnectStore(
            recentsURL: directory.appendingPathComponent("workspace-recents.json")
        ) { _ in }
        defer { try? FileManager.default.removeItem(at: directory) }
        let recent = TargetConfig(
            name: "muxterm",
            runtime: .herdr,
            transport: .ssh(name: "ryzen"),
            path: "/home/wlz/Developer/self/muxterm"
        )
        store.recordRecent(recent)
        let panel = UnifiedPanelController(
            store: store,
            ownerWindow: nil,
            snapshot: { nil },
            sendInput: { _, _ in },
            search: { _ in }
        )
        panel.present(initial: .workspaces)
        defer { panel.dismiss() }

        let titles = panel.testWorkspaceTitles()
        guard let host = titles.firstIndex(of: "local"),
              let action = titles.firstIndex(of: MuxtermI18n.shared.tr(.panelOpenFolder)),
              let target = titles.firstIndex(of: recent.path)
        else {
            return XCTFail("打开列表应同时包含分组、动作和最近目录: \(titles)")
        }
        XCTAssertEqual(panel.testRowHeight(at: host), QuickActionCellView.sectionRowHeight)
        XCTAssertEqual(panel.testRowHeight(at: action), QuickActionCellView.preferredRowHeight)
        XCTAssertEqual(panel.testRowHeight(at: target), QuickTargetCellView.preferredRowHeight)
        XCTAssertFalse(panel.testWorkspaceRowIsSelectable(matching: "local"))
        XCTAssertLessThan(panel.testRowHeight(at: host), panel.testRowHeight(at: target))

        panel.testSetQuery("local")
        XCTAssertFalse(panel.testWorkspaceTitles().contains("local"))
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
