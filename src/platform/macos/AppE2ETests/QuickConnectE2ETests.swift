import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 对标 `linux_quickconnect_e2e`：status snapshot + zoom 任务。
final class QuickConnectE2ETests: XCTestCase {
    func testStartupLocalWorkspaceAppearsInRecent() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(
            bridge: bridge,
            debug: true,
            quickConnectStore: QuickConnectStore()
        )
        defer { app.testShutdown() }

        app.openQuickConnect()
        AppE2E.pump(80)

        let cell = app.unifiedPanel.testWorkspaceCell(at: 0)
        XCTAssertNotNil(cell, "启动创建的 local workspace 必须出现在 Quick Connect")
        XCTAssertEqual(cell?.testTitleText(), "workspace")
        XCTAssertEqual(cell?.testBadgeDotSizes().count, 1, "启动 workspace 必须带 Recent 标记")
        XCTAssertTrue(cell?.testIsCurrent() == true, "启动 workspace 必须标为 Current")
    }

    func testStatusSnapshotAndFullscreenZoomOnIsolatedTmux() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("qc-status")
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", "stat",
            "-x", "80", "-y", "24",
        ])
        let bridge = try CoreBridge(backendType: "tmux", socket: socket, session: "stat")
        defer { bridge.shutdown() }

        XCTAssertTrue(
            AppE2E.wait(timeout: 3) {
                _ = bridge.pollEvents()
                guard let json = bridge.statusBarSnapshotJSON(),
                      let data = json.data(using: .utf8),
                      let response = try? JSONDecoder().decode(StatusBarResponse.self, from: data),
                      let snap = response.status
                else {
                    return false
                }
                return snap.enabled && !snap.windows.isEmpty
            },
            "status snapshot 应含窗口列表"
        )
        let snapshot: StatusBarSnapshot = try {
            let json = try XCTUnwrap(bridge.statusBarSnapshotJSON())
            let response = try JSONDecoder().decode(StatusBarResponse.self, from: Data(json.utf8))
            return try XCTUnwrap(response.status)
        }()
        XCTAssertTrue(snapshot.windows.contains(where: \.current), "应有当前窗口标记")

        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                _ = bridge.pollEvents()
                let readyTabs = bridge.getTabs()
                guard let tab = readyTabs.first(where: \.isActive)
                    ?? readyTabs.first
                else {
                    return false
                }
                return !bridge.getPanes(tabId: tab.id).isEmpty
            },
            "执行 pane 操作前 core 拓扑必须就绪"
        )
        let tabs = bridge.getTabs()
        let active = try XCTUnwrap(tabs.first(where: \.isActive) ?? tabs.first).id
        let panes = bridge.getPanes(tabId: active)
        let paneId = try XCTUnwrap(panes.first(where: \.isActive) ?? panes.first).id
        XCTAssertEqual(bridge.execute(task: MuxTask.splitPane(targetPane: paneId, horizontal: true)), 0)
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                _ = bridge.pollEvents()
                return bridge.getPanes(tabId: active).count >= 2
            },
            "fullscreen 前 split-pane 必须完成并同步到 core"
        )
        XCTAssertEqual(bridge.execute(task: MuxTask.togglePaneFullscreen(paneId)), 0, "resize-pane -Z 应成功")
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                Tmux.out(socket: socket, args: ["display-message", "-p", "-t", "stat", "#{window_zoomed_flag}"]) == "1"
            },
            "tmux window_zoomed_flag 应为 1"
        )
        _ = bridge.execute(task: MuxTask.togglePaneFullscreen(paneId))
    }

    func testUnifiedPanelShowsFullHyphenatedNamesAsColorDots() {
        AppE2E.ensureApp()
        let store = QuickConnectStore()
        store.upsertProject(TargetConfig(
            name: "archmini-home",
            runtime: .tmux,
            transport: .ssh(name: "archmini"),
            path: "~"
        ))
        store.upsertProject(TargetConfig(
            name: "pc-home",
            runtime: .tmux,
            transport: .ssh(name: "pc"),
            path: "~"
        ))
        store.upsertProject(TargetConfig(
            name: "ubuntu-home",
            runtime: .tmux,
            transport: .ssh(name: "cd"),
            path: "~"
        ))
        let panel = UnifiedPanelController(
            store: store,
            ownerWindow: nil,
            snapshot: { nil },
            paneOutput: { _ in Data() },
            sendInput: { _, _ in },
            search: { _, _ in [] }
        )
        panel.present()
        AppE2E.pump(80)
        panel.window?.layoutIfNeeded()
        XCTAssertGreaterThan(
            panel.testTableColumnWidth(),
            600,
            "统一面板列必须跟 720pt 窗口，不能停在 100pt。width=\(panel.testTableColumnWidth())"
        )
        let expected = ["archmini-home", "pc-home", "ubuntu-home"]
        for (index, name) in expected.enumerated() {
            let cell = panel.testWorkspaceCell(at: index)
            XCTAssertNotNil(cell, "row \(index) 必须是 QuickTargetCellView（\(name)）")
            guard let cell else { continue }
            XCTAssertEqual(cell.testTitleText(), name)
            let needed = cell.testTitleTextWidth()
            let got = cell.testTitleBoundsWidth()
            XCTAssertGreaterThan(needed, 30, "\(name) 文字宽度不能退化成一个字形")
            XCTAssertGreaterThanOrEqual(
                got,
                needed - 1,
                "\(name) 必须完整显示。needed=\(needed) got=\(got)"
            )
            let dots = cell.testBadgeDotSizes()
            XCTAssertEqual(dots.count, 1, "\(name) 应有一个 Project 色块")
            XCTAssertEqual(dots.first?.width ?? 0, QuickTargetCellView.badgeDotSize, accuracy: 0.5)
        }
        panel.dismiss()
    }

    func testFillWidthScrollViewStretchesDefaultHundredPointColumn() {
        AppE2E.ensureApp()
        let scroll = MuxtermFillWidthScrollView(frame: NSRect(x: 0, y: 0, width: 720, height: 200))
        let table = NSTableView()
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("panel"))
        column.width = 100
        column.minWidth = 40
        table.addTableColumn(column)
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        scroll.documentView = table
        scroll.layoutSubtreeIfNeeded()
        scroll.tile()
        let width = column.width
        XCTAssertEqual(width, scroll.contentView.bounds.width, accuracy: 1.5)
        XCTAssertGreaterThan(
            width,
            600,
            "列必须跟 clip view（720），不能停在默认 100pt。width=\(width) clip=\(scroll.contentView.bounds.width)"
        )
    }

    func testHyphenatedProjectNameStaysFullyVisibleBesideBadgeDots() {
        AppE2E.ensureApp()
        let host = NSView(frame: NSRect(x: 0, y: 0, width: 240, height: QuickTargetCellView.preferredRowHeight))
        let cell = QuickTargetCellView(identifier: NSUserInterfaceItemIdentifier("qc"))
        cell.translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(cell)
        NSLayoutConstraint.activate([
            cell.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            cell.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            cell.topAnchor.constraint(equalTo: host.topAnchor),
            cell.bottomAnchor.constraint(equalTo: host.bottomAnchor),
        ])
        cell.config = TargetConfig(
            name: "archmini-home",
            runtime: .tmux,
            transport: .ssh(name: "archmini"),
            path: "~"
        )
        cell.badges = [.recent, .project]
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(cell.testTitleText(), "archmini-home")
        let needed = cell.testTitleTextWidth()
        let got = cell.testTitleBoundsWidth()
        XCTAssertGreaterThan(needed, 40, "archmini-home 的文字宽度不能退化成一个字形。needed=\(needed)")
        XCTAssertGreaterThanOrEqual(
            got,
            needed - 1,
            "名称必须完整显示，不能截成 a。needed=\(needed) got=\(got)"
        )
        let dots = cell.testBadgeDotSizes()
        XCTAssertEqual(dots.count, 2)
        for size in dots {
            XCTAssertEqual(size.width, QuickTargetCellView.badgeDotSize, accuracy: 0.5)
            XCTAssertEqual(size.height, QuickTargetCellView.badgeDotSize, accuracy: 0.5)
            XCTAssertLessThan(size.width, 20, "徽章必须是小色块，不能是 42pt PROJECT 胶囊。size=\(size)")
        }
    }

    func testExistingProjectShowsGreenDotBeforeName() {
        AppE2E.ensureApp()
        let host = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: QuickTargetCellView.preferredRowHeight))
        let cell = QuickTargetCellView(identifier: NSUserInterfaceItemIdentifier("qc-exists"))
        cell.translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(cell)
        NSLayoutConstraint.activate([
            cell.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            cell.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            cell.topAnchor.constraint(equalTo: host.topAnchor),
            cell.bottomAnchor.constraint(equalTo: host.bottomAnchor),
        ])
        cell.config = TargetConfig(
            name: "muxterm",
            runtime: .tmux,
            transport: .local,
            path: FileManager.default.temporaryDirectory.path
        )
        cell.badges = [.project]
        cell.existence = .exists
        host.layoutSubtreeIfNeeded()

        XCTAssertTrue(cell.testProjectExistenceDotVisible())
        XCTAssertLessThan(
            cell.testProjectExistenceDotFrame().maxX,
            cell.testTitleFrame().minX,
            "存在状态点必须位于 Project 名称左侧"
        )

        let titleWithDot = cell.testTitleFrame()
        cell.existence = .missing
        host.layoutSubtreeIfNeeded()
        XCTAssertFalse(cell.testProjectExistenceDotVisible())
        let titleWithoutDot = cell.testTitleFrame()
        XCTAssertGreaterThanOrEqual(
            titleWithDot.minX - titleWithoutDot.minX,
            QuickTargetCellView.badgeDotSize + 5,
            "隐藏存在点时名称不能保留空白占位"
        )
    }
}
