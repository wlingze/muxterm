import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 统一面板不能只显示列表：Linux 已有的搜索范围与 Attention 动作要走真实 FFI。
final class UnifiedPanelActionsE2ETests: XCTestCase {
    func testSearchScopesFilterCurrentPaneAndWorkspace() throws {
        let fx = TwoPaneCat(label: "panel-scopes")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let activePane = UInt32(fx.panes[0].replacingOccurrences(of: "%", with: "")) ?? 0
        let backgroundPane = UInt32(fx.panes[1].replacingOccurrences(of: "%", with: "")) ?? 1
        app.testSwitchPane(activePane)
        app.testOpenSearchPanel()
        app.testSetSearchQuery("E2E_")
        AppE2E.pump(80)

        XCTAssertNotNil(app.testView(identifier: "muxterm.search.scope.pane"))
        XCTAssertNotNil(app.testView(identifier: "muxterm.search.scope.workspace"))
        XCTAssertNotNil(app.testView(identifier: "muxterm.search.scope.all"))
        XCTAssertTrue(Set(app.unifiedPanel.testSearchHitPaneIDs()).isSuperset(of: [activePane, backgroundPane]))

        app.unifiedPanel.testSetSearchScope(.pane)
        XCTAssertEqual(Set(app.unifiedPanel.testSearchHitPaneIDs()), [activePane])

        app.unifiedPanel.testSetSearchScope(.workspace)
        XCTAssertTrue(Set(app.unifiedPanel.testSearchHitPaneIDs()).isSuperset(of: [activePane, backgroundPane]))
    }

    func testAttentionActionsExposePreviewAndMuteProductionPath() throws {
        let fx = TwoPaneCat(label: "panel-attention-actions")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let pane0 = UInt32(fx.panes[0].replacingOccurrences(of: "%", with: "")) ?? 0
        app.testSwitchPane(pane0)
        fx.sendBelOnBackground()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return app.testBlockedCount() >= 1
        })

        app.testOpenAttentionPanel()
        AppE2E.pump(80)
        app.unifiedPanel.testSelectFirstRow()
        XCTAssertNotNil(app.testView(identifier: "muxterm.attention.jump"))
        XCTAssertNotNil(app.testView(identifier: "muxterm.attention.open"))
        XCTAssertNotNil(app.testView(identifier: "muxterm.attention.mute"))

        app.unifiedPanel.testOpenSelectedAttention()
        AppE2E.pump(80)
        XCTAssertTrue(app.testReplyOverlayVisible(), "可见的 Open 动作必须走现有 replica overlay")
        app.toggleReplyOverlay()

        app.unifiedPanel.testMuteSelected(seconds: 300)
        XCTAssertTrue(AppE2E.wait(timeout: 2) {
            app.testPollOnce()
            return app.testBlockedCount() == 0
        }, "静音必须立即从红点计数排除")
    }

    func testAllScopeFindsAndJumpsToWarmBackgroundWorkspace() throws {
        let first = OnePaneCat(label: "panel-all-first")
        let second = OnePaneCat(label: "panel-all-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        let firstBridge = app.bridge

        let secondBridge = try CoreBridge(
            backendType: "tmux",
            socket: second.socket,
            session: second.session
        )
        app.testActivateWorkspaceBridge(secondBridge, session: second.session)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testActiveWorkspaceSession() == second.session
                && app.testLayoutLeafIDs().count == 1
        })

        let token = "PANEL_BACKGROUND_SEARCH_\(UUID().uuidString)"
        Tmux.sendLiteral(socket: first.socket, target: first.pane, text: token)
        Tmux.ok(socket: first.socket, args: ["send-keys", "-t", first.pane, "Enter"])
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            guard let json = firstBridge.searchAllJSON(query: token),
                  let snapshot = SearchSnapshot.decode(Data(json.utf8))
            else {
                return false
            }
            return !snapshot.hits.isEmpty
        }, "后台 Workspace 的 PaneBuf 必须继续索引")

        app.testOpenSearchPanel()
        app.testSetSearchQuery(token)
        AppE2E.pump(80)
        let firstWorkspace = "\(first.session)@local"
        XCTAssertFalse(
            app.unifiedPanel.testSearchHitWorkspaceIDs().contains(firstWorkspace),
            "Cmd-F 默认只搜索当前 Workspace；后台 Workspace 只能由 Cmd-Shift-F 全局搜索命中"
        )

        app.unifiedPanel.testSetSearchScope(.workspace)
        XCTAssertEqual(app.testSearchHitCount(), 0, "Workspace 范围不能混入后台连接")
        app.unifiedPanel.testSetSearchScope(.all)
        XCTAssertTrue(app.unifiedPanel.testSearchHitWorkspaceIDs().contains(firstWorkspace))

        app.testActivateFirstSearchHit()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            return app.testActiveWorkspaceSession() == first.session
                && app.testAllVisibleTerminalText().contains(token)
        }, "All 搜索命中必须激活后台 Workspace 并显示对应 Surface")
        XCTAssertFalse(app.testSearchPanelOpen())
    }

    func testAttentionMuteRoutesToWarmBackgroundWorkspaceWithoutActivatingIt() throws {
        let first = OnePaneCat(label: "panel-mute-first")
        let second = OnePaneCat(label: "panel-mute-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        let firstBridge = app.bridge

        let secondBridge = try CoreBridge(
            backendType: "tmux",
            socket: second.socket,
            session: second.session
        )
        app.testActivateWorkspaceBridge(secondBridge, session: second.session)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testActiveWorkspaceSession() == second.session
        })

        Tmux.sendHex(socket: first.socket, target: first.pane, bytes: [0x07])
        Tmux.ok(socket: first.socket, args: ["send-keys", "-t", first.pane, "Enter"])
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return Self.blockedCount(firstBridge) > 0
        }, "后台 Workspace 的 BEL 必须进入 Attention")

        app.testOpenAttentionPanel()
        AppE2E.pump(80)
        app.unifiedPanel.testSelectFirstRow()
        XCTAssertEqual(
            app.unifiedPanel.testSelectedAttentionRow()?.workspaceId,
            "\(first.session)@local"
        )
        app.unifiedPanel.testMuteSelected(seconds: 300)
        XCTAssertEqual(app.testActiveWorkspaceSession(), second.session, "Mute 不应切换 Workspace")
        XCTAssertTrue(AppE2E.wait(timeout: 2) {
            Self.blockedCount(firstBridge) == 0
        }, "Mute 必须发送到条目所属的后台 bridge")
    }

    func testAttentionOpenActivatesWarmBackgroundWorkspace() throws {
        let first = OnePaneCat(label: "panel-open-first")
        let second = OnePaneCat(label: "panel-open-second")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))

        let secondBridge = try CoreBridge(
            backendType: "tmux",
            socket: second.socket,
            session: second.session
        )
        app.testActivateWorkspaceBridge(secondBridge, session: second.session)
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            return app.testActiveWorkspaceSession() == second.session
        })

        Tmux.sendHex(socket: first.socket, target: first.pane, bytes: [0x07])
        Tmux.ok(socket: first.socket, args: ["send-keys", "-t", first.pane, "Enter"])
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testOpenAttentionPanel()
            return app.testAttentionRowCount() > 0
        })
        app.unifiedPanel.testSelectFirstRow()
        app.unifiedPanel.testOpenSelectedAttention()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return app.testActiveWorkspaceSession() == first.session
                && app.testReplyOverlayVisible()
        }, "Open 必须先激活条目所属 Workspace，再打开对应 pane overlay")
    }

    private static func blockedCount(_ bridge: CoreBridge) -> Int {
        guard let json = bridge.attentionSnapshotJSON(),
              let snapshot = AttentionSnapshot.decode(Data(json.utf8))
        else {
            return 0
        }
        return snapshot.blockedCount
    }
}
