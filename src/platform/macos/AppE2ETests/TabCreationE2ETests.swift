import XCTest
@testable import MuxtermAppLib

/// Tab 创建必须走 Core task → tmux runtime → Core snapshot；GUI 不得自己
/// 猜 tmux window index，也不能在 snapshot 到达前画半个 Surface。
final class TabCreationE2ETests: XCTestCase {
    func testNewTabUsesRuntimeSnapshotAndRevealsSurface() throws {
        let painted = PaintedWorkspace(label: "tab-creation")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2), "初始 runtime 必须暴露两个 tab")

        let before = app.testTabIDs()
        app.testNewTab()

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                let ids = app.testTabIDs()
                return ids.count == before.count + 1
                    && ids.contains(where: { !before.contains($0) })
            },
            "新建 tab 必须由 Core snapshot 收敛，before=\(before) after=\(app.testTabIDs())"
        )

        let after = app.testTabIDs()
        let newTab = try XCTUnwrap(after.first(where: { !before.contains($0) }))
        XCTAssertEqual(app.testActiveTabID(), newTab, "tmux runtime 新建 window 后应激活新 tab")
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testPaneSurfaceReady(app.testActivePaneID())
            },
            "新 tab 的唯一 pane Surface 必须在 seed/catch-up 完成后显示"
        )

        let runtimeWindows = Tmux.out(
            socket: painted.socket,
            args: ["list-windows", "-t", painted.session, "-F", "#{window_id}"]
        )
            .split(whereSeparator: \.isNewline)
            .map(String.init)
        XCTAssertEqual(
            Set(runtimeWindows),
            Set(after.map { "@\($0)" }),
            "tmux runtime 与 Core snapshot 必须是一一对应的 tab 集合"
        )
    }
}
