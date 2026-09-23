import AppKit
import XCTest
@testable import MuxtermAppLib

/// 先前 macOS bug：Alt/Cmd+Enter 后 tmux 已 zoom，GUI 仍显示多 pane。
final class ZoomE2ETests: XCTestCase {

    func testCachedTabRevealKeepsAllocatedGridAndContents() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let manager = TerminalManager(bridge: bridge)
        manager.setBridgeQueriesEnabled(false)
        let layout = PaneLayoutView(terminalManager: manager)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 600),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = layout
        window.orderFront(nil)
        defer { window.orderOut(nil) }
        let panes = [Pane(id: 1, cols: 178, rows: 23, isActive: true),
                     Pane(id: 2, cols: 178, rows: 23, isActive: false)]
        let split = LayoutNode.split(horizontal: false, ratio: 500,
            first: .leaf(paneId: 1), second: .leaf(paneId: 2))
        layout.apply(layout: split, panes: panes, tabId: 1)
        let other = [Pane(id: 3, cols: 178, rows: 50, isActive: true)]
        for fullscreen in [false, true] {
            if fullscreen { layout.toggleFullscreen(paneId: 1) }
            AppE2E.pump(100)
            let view = manager.view(for: 1)
            let before = view.renderedGridSize
            view.feedOutput(Data("\u{1b}[H\u{1b}[2JTOP\u{1b}[\(before.rows);1HBOTTOM".utf8))
            for _ in 0..<3 {
                layout.apply(layout: .leaf(paneId: 3), panes: other, tabId: 2)
                AppE2E.pump(30)
                XCTAssertNotNil(layout.revealCachedTab(1))
                XCTAssertEqual(view.renderedGridSize.cols, before.cols)
                XCTAssertEqual(view.renderedGridSize.rows, before.rows)
                AppE2E.pump(60)
                XCTAssertEqual(view.renderedGridSize.cols, before.cols)
                XCTAssertEqual(view.renderedGridSize.rows, before.rows)
                XCTAssertTrue(view.visibleScreenText().contains("BOTTOM"))
            }
        }
    }
    func testRepeatedFullscreenSnapshotsKeepTheSameHost() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let manager = TerminalManager(bridge: bridge)
        let layout = PaneLayoutView(terminalManager: manager)
        layout.frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
        let panes = [Pane(id: 1, cols: 80, rows: 24, isActive: true),
                     Pane(id: 2, cols: 80, rows: 24, isActive: false)]
        let split = LayoutNode.split(horizontal: false, ratio: 500,
            first: .leaf(paneId: 1), second: .leaf(paneId: 2))
        XCTAssertTrue(layout.apply(layout: split, panes: panes, tabId: 1))
        layout.toggleFullscreen(paneId: 1)
        let host = try XCTUnwrap(layout.testHost(for: 1))
        for _ in 0..<3 {
            XCTAssertTrue(layout.apply(layout: split, panes: panes, tabId: 1))
            XCTAssertTrue(layout.testHost(for: 1) === host,
                "全屏投影只有一个 pane，不能拿完整 tab 的两个 pane 判定缓存失效")
        }
        let other = [Pane(id: 3, cols: 80, rows: 24, isActive: true)]
        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 3), panes: other, tabId: 2))
        XCTAssertNotNil(layout.revealCachedTab(1))
        XCTAssertTrue(layout.apply(layout: split, panes: panes, tabId: 1))
        XCTAssertTrue(layout.testHost(for: 1) === host)
    }

    func testRuntimeZoomProjectionKeepsTheSameHost() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let layout = PaneLayoutView(terminalManager: TerminalManager(bridge: bridge))
        let panes = [Pane(id: 1, cols: 178, rows: 50, isActive: true),
                     Pane(id: 2, cols: 88, rows: 24, isActive: false)]
        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 1), panes: panes, tabId: 1))
        let host = try XCTUnwrap(layout.testHost(for: 1))
        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 1), panes: panes, tabId: 1))
        XCTAssertTrue(layout.testHost(for: 1) === host)
        XCTAssertFalse(layout.testPaneTitleVisible(1))
    }

    func testAltEnterCollapsesLayoutToSingleLeaf() throws {
        let painted = PaintedWorkspace(label: "zoom")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3), "zoom 前应有 3 leaf")
        let token = painted.tab1Tokens[0]
        XCTAssertTrue(app.waitTerminalContains(token))

        app.testTogglePaneFullscreen()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(30)
                return Tmux.out(
                    socket: painted.socket,
                    args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                ) == "1"
            },
            "tmux window_zoomed_flag 应为 1"
        )
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testLayoutLeafIDs().count == 1
            },
            "GUI 必须变成单 leaf（不能只 zoom tmux）。leaves=\(app.testLayoutLeafIDs())"
        )
        let leaf = try XCTUnwrap(app.testLayoutLeafIDs().first)
        let size = app.testPaneAllocation(leaf)
        XCTAssertGreaterThanOrEqual(size.width, AppE2E.minPanePx)
        XCTAssertGreaterThanOrEqual(size.height, AppE2E.minPanePx)
        XCTAssertTrue(
            app.testPaneTerminalText(leaf).contains(token)
                || app.testAllVisibleTerminalText().contains(token),
            "zoom 后最后一帧必须还在"
        )

        app.testTogglePaneFullscreen()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.testLayoutLeafIDs().count == 3
            },
            "再按一次应恢复 3 leaf。leaves=\(app.testLayoutLeafIDs())"
        )
    }

    func testBracketNavigationKeepsTheNextPaneZoomed() throws {
        let painted = PaintedWorkspace(label: "zoom-navigation")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3), "zoom 前应有 3 leaf")
        let paneIDs = painted.tab1Panes.compactMap { UInt32($0.dropFirst()) }
        XCTAssertEqual(paneIDs.count, 3, "夹具 pane id 应可解析")
        let firstPane = try XCTUnwrap(paneIDs.first)
        let secondPane = try XCTUnwrap(paneIDs.dropFirst().first)
        XCTAssertEqual(app.testActivePaneID(), firstPane)
        let secondGridBeforeZoom = app.testPaneGrid(secondPane)

        app.testTogglePaneFullscreen()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return Tmux.out(
                    socket: painted.socket,
                    args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                ) == "1" && app.testLayoutLeafIDs() == [firstPane]
            },
            "第一个 pane 全屏后，tmux 与 GUI 都应只显示该 pane"
        )

        let next = try XCTUnwrap(
            app.testMakeKeyEvent(key: "]", keyCode: 30, command: true),
            "必须能构造 Cmd-]"
        )
        XCTAssertTrue(app.testDispatchKeyEvent(next), "Cmd-] 必须被窗口快捷键消费")
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.testActivePaneID() == secondPane
                    && app.testLayoutLeafIDs() == [secondPane]
                    && Tmux.out(
                        socket: painted.socket,
                        args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                    ) == "1"
            },
            "Cmd-] 应切到下一个 pane，并继续保持全屏"
        )
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(30)
                let after = app.testPaneGrid(secondPane)
                return after.cols > secondGridBeforeZoom.cols
                    || after.rows > secondGridBeforeZoom.rows
            },
            "目标 Surface 必须从 split grid 放大到全屏 allocation。before=\(secondGridBeforeZoom) after=\(app.testPaneGrid(secondPane))"
        )

        // 窗口快捷键用 Cmd-[。Option-only 组合（Alt-[）现在必须进 pane：
        // KeyPassthroughPolicy 只让 Cmd/Ctrl 钩子消费事件，否则 vim/emacs/Herdr
        // 的 Meta 组合会被前端吃掉。本条断言的是 bracket 导航本身，
        // 所以用产品实际消费的 chord。
        let previous = try XCTUnwrap(
            app.testMakeKeyEvent(key: "[", keyCode: 33, command: true),
            "必须能构造 Cmd-["
        )
        XCTAssertTrue(app.testDispatchKeyEvent(previous), "Cmd-[ 必须被窗口快捷键消费")
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.testActivePaneID() == firstPane
                    && app.testLayoutLeafIDs() == [firstPane]
                    && Tmux.out(
                        socket: painted.socket,
                        args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                    ) == "1"
            },
            "Cmd-[ 应切回上一个 pane，并继续保持全屏"
        )
    }

    func testRepeatedMouseSelectionDoesNotQueuePaneFocus() throws {
        let painted = PaintedWorkspace(label: "mouse-repeat")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))
        let pane = app.testActivePaneID()
        let count = app.testQueuedCommandCount()
        for _ in 0..<30 { app.testActivatePaneFromSurface(pane) }
        XCTAssertEqual(app.testQueuedCommandCount(), count,
                       "已经激活的 pane 上选区点击不能再发远端 select/focus")
        XCTAssertEqual(app.testFocusTargetPaneID(), pane)
    }

    func testMouseActivatedPaneBecomesFullscreenTarget() throws {
        let painted = PaintedWorkspace(label: "mouse-zoom")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3))
        let paneIDs = painted.tab1Panes.compactMap { UInt32($0.dropFirst()) }
        let secondPane = try XCTUnwrap(paneIDs.dropFirst().first)
        app.testActivatePaneFromSurface(secondPane)

        XCTAssertEqual(app.testActivePaneID(), secondPane, "鼠标点击必须立即更新产品 active pane")
        XCTAssertEqual(app.testFocusTargetPaneID(), secondPane, "键盘焦点与缩放目标必须是同一 pane")

        app.testTogglePaneFullscreen()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                return app.testLayoutLeafIDs() == [secondPane]
                    && Tmux.out(
                        socket: painted.socket,
                        args: ["display-message", "-p", "-t", painted.session, "#{pane_id}"]
                    ) == "%\(secondPane)"
            },
            "Cmd-Enter 应放大鼠标刚选择的 pane"
        )
    }
}
