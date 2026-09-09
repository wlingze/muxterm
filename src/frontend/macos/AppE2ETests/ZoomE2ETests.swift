import AppKit
import XCTest
@testable import MuxtermAppLib

/// 先前 macOS bug：Alt/Cmd+Enter 后 tmux 已 zoom，GUI 仍显示多 pane。
final class ZoomE2ETests: XCTestCase {
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

        let previous = try XCTUnwrap(
            app.testMakeKeyEvent(key: "[", keyCode: 33, option: true),
            "必须能构造 Alt-["
        )
        XCTAssertTrue(app.testDispatchKeyEvent(previous), "Alt-[ 必须被窗口快捷键消费")
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
            "Alt-[ 应切回上一个 pane，并继续保持全屏"
        )
    }
}
