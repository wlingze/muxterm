import XCTest
@testable import MuxtermAppLib

/// 切 tab 与切 pane 使用同一套“上次看到这里”语义：离开旧 Surface 后
/// 继续接收 live 输出，回来时给用户一个可验证的稳定行跳转入口。
final class LastSeenE2ETests: XCTestCase {
    func testTabSwitchRecordsLastSeenAndReturnsToOldLine() throws {
        let painted = PaintedWorkspace(label: "last-seen-tab")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2), "attach 后应有至少两个 tab")
        let firstTab = app.testActiveTabID()
        let secondTab = try XCTUnwrap(
            app.testTabIDs().first { $0 != firstTab },
            "必须能找到第二个 tab"
        )
        let firstPaneID = app.testActivePaneID()
        let firstPaneTarget = try XCTUnwrap(
            painted.tab1Panes.first {
                UInt32($0.trimmingCharacters(in: CharacterSet(charactersIn: "%@"))) == firstPaneID
            },
            "tmux pane 列表必须包含当前 active pane \(firstPaneID)"
        )

        app.testSwitchTab(secondTab)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testActiveTabID() == secondTab
            },
            "切到第二个 tab 后必须收到 active-tab 事件"
        )

        let leftHere = "LEFT_HERE_\(ProcessInfo.processInfo.processIdentifier)"
        var lines = "\(leftHere)\n"
        // 填充必须超过当前 pane 可见行，否则 last-seen 的 offset 仍是 0。
        // 1280×800 左栏全高大约 40+ 行；再留余量给 CI runner 的大格子。
        let visibleRows = max(32, Int(app.testPaneAllocation(firstPaneID).height / 12))
        let padCount = visibleRows + 40
        for index in 0..<padCount {
            lines += "away-pad-\(index)\n"
        }
        Tmux.sendLiteral(socket: painted.socket, target: firstPaneTarget, text: lines)
        Tmux.waitCapture(socket: painted.socket, target: firstPaneTarget, needle: leftHere, history: true)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testSearchAll(leftHere).contains { hit in
                    hit.paneId == firstPaneID && hit.line.contains(leftHere)
                }
            },
            "离开旧 tab 时 Core Index 仍必须消费旧 pane 的 live 输出"
        )

        app.testSwitchTab(firstTab)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testActiveTabID() == firstTab
                    && app.testLastSeenVisible()
                    && app.testPaneSurfaceReady(firstPaneID)
            },
            "从 tab 切回后必须显示 last-seen 按钮；active=\(app.testActiveTabID()) lastSeen=\(app.testLastSeenDiagnostics(paneId: firstPaneID)) hits=\(app.testSearchAll(leftHere)) viewport=\(app.testPaneViewport())"
        )
        // tab 结构事件之后 PaneLayout 还有一次异步几何同步；先让它
        // 稳定，再点击，避免把“布局尚未挂载”误判成 last-seen 跳转失败。
        AppE2E.pump(200)
        app.testPollOnce()
        app.testFlushFeeds()
        let jumpOffset = app.testLastSeenJumpOffset()
        XCTAssertGreaterThan(
            jumpOffset ?? 0,
            0,
            "last-seen 目标必须离开 live 尾部；\(app.testLastSeenDiagnostics(paneId: firstPaneID)) jump=\(String(describing: jumpOffset))"
        )
        app.testClickLastSeen()
        AppE2E.pump(80)
        XCTAssertGreaterThan(
            app.testPaneViewport(),
            0,
            "点击 last-seen 必须把 core viewport 带离 live 尾部；viewport=\(app.testPaneViewport()) jump=\(String(describing: jumpOffset))"
        )
        XCTAssertLessThan(
            app.testNativeScrollPosition(),
            0.999,
            "SwiftTerm 必须跟着离开底部；pos=\(app.testNativeScrollPosition()) visible=\(app.testPaneTerminalText(firstPaneID))"
        )
        XCTAssertFalse(app.testLastSeenVisible(), "点击后 last-seen 按钮应隐藏")
    }
}
