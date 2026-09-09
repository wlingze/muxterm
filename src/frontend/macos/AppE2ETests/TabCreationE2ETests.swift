import XCTest
@testable import MuxtermAppLib

/// Tab 创建必须走 Core task → tmux runtime → Core snapshot；GUI 不得自己
/// 猜 tmux window index，也不能在 snapshot 到达前画半个 Surface。
final class TabCreationE2ETests: XCTestCase {
    /// 真实 AppKit + tmux control-mode 回归：attach 后后台 tab 正在逐 pane
    /// 建索引时，NewTab/CloseTab 仍必须在交互预算内完成并显示可用 Surface。
    func testBusyBackgroundTabsDoNotBlockCreateOrClose() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("tab-mutation-latency")
        let session = "tab-mutation-latency"
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "120", "-y", "36", "--", "/bin/cat",
        ])

        // 6 个隐藏 window、每个 2 pane，确保 attach 后存在一批真实的后台
        // capture 工作；mutation 不能排在整批 capture 之后。
        for index in 1...6 {
            Tmux.ok(socket: socket, args: [
                "new-window", "-d", "-t", session, "-n", "background-\(index)", "/bin/cat",
            ])
            let window = Tmux.out(
                socket: socket,
                args: ["list-windows", "-t", session, "-F", "#{window_id}"]
            )
                .split(whereSeparator: \.isNewline)
                .map(String.init)
                .last ?? ""
            XCTAssertFalse(window.isEmpty, "应找到刚创建的后台 window")
            Tmux.ok(socket: socket, args: ["split-window", "-h", "-t", window, "/bin/cat"])
            Tmux.sendLiteral(
                socket: socket,
                target: window,
                text: "BACKGROUND_INDEX_\(index)_\(String(repeating: "x", count: 512))"
            )
        }
        Tmux.ok(socket: socket, args: ["select-window", "-t", "\(session):0"])

        let app = try AppE2E.attachWindow(socket: socket, session: session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 7), "attach 后必须先暴露全部已有 tab")
        let originalTabs = app.testTabIDs()
        XCTAssertEqual(originalTabs.count, 7)

        let createStarted = Date()
        app.testNewTab()
        let createdReady = AppE2E.wait(timeout: 2.0) {
            app.testPollOnce()
            app.testFlushFeeds()
            let ids = app.testTabIDs()
            guard let created = ids.first(where: { !originalTabs.contains($0) }) else {
                return false
            }
            return ids.count == originalTabs.count + 1
                && app.testActiveTabID() == created
                && app.testPaneSurfaceReady(app.testActivePaneID())
        }
        let createElapsed = Date().timeIntervalSince(createStarted)
        XCTAssertTrue(createdReady, "繁忙后台索引期间新 tab 必须在 2s 内出现且 Surface 可用")
        XCTAssertLessThan(createElapsed, 2.0, "NewTab 实际耗时 \(createElapsed)s")
        let created = try XCTUnwrap(
            app.testTabIDs().first(where: { !originalTabs.contains($0) })
        )

        let closeStarted = Date()
        app.testCloseTab(created)
        let closedReady = AppE2E.wait(timeout: 2.0) {
            app.testPollOnce()
            app.testFlushFeeds()
            return Set(app.testTabIDs()) == Set(originalTabs)
                && app.testPaneSurfaceReady(app.testActivePaneID())
        }
        let closeElapsed = Date().timeIntervalSince(closeStarted)
        XCTAssertTrue(closedReady, "繁忙后台索引期间关闭 tab 必须在 2s 内收敛且保留可用 Surface")
        XCTAssertLessThan(closeElapsed, 2.0, "CloseTab 实际耗时 \(closeElapsed)s")
    }

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

    func testNewTabThenFirstVisitOtherTabThenCloseKeepsSurfaces() throws {
        let painted = PaintedWorkspace(label: "tab-lifecycle")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2), "初始 runtime 必须暴露两个 tab")

        let original = app.testTabIDs()
        let firstTab = app.testActiveTabID()
        let secondTab = try XCTUnwrap(
            original.first { $0 != firstTab },
            "必须能找到第二个 tab"
        )
        let firstPane = app.testActivePaneID()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testPaneSurfaceReady(firstPane)
            },
            "attach 活动 tab 必须先有 Surface"
        )

        app.testNewTab()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                let ids = app.testTabIDs()
                return ids.count == original.count + 1
                    && ids.contains(where: { !original.contains($0) })
            },
            "新建 tab 必须由 Core snapshot 收敛"
        )
        let created = try XCTUnwrap(app.testTabIDs().first { !original.contains($0) })
        XCTAssertEqual(app.testActiveTabID(), created)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testPaneSurfaceReady(app.testActivePaneID())
            },
            "新建 tab 的 Surface 必须显示，不能卡在 seeding"
        )
        XCTAssertTrue(
            app.testHasCachedTab(firstTab),
            "新建 tab 不得把已打开的 tab 树丢掉"
        )

        app.testSwitchTab(secondTab)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testActiveTabID() == secondTab
                    && app.testPaneSurfaceReady(app.testActivePaneID())
            },
            "新建 tab 之后第一次点另一个已存在的 tab 仍必须显示 Surface"
        )
        XCTAssertTrue(
            app.waitTerminalContains(painted.tab2Token),
            "第一次点过去必须能看到 attach 前已经写进 pane 的 token \(painted.tab2Token)"
        )

        app.testSwitchTab(firstTab)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return app.testActiveTabID() == firstTab
                    && app.testPaneSurfaceReady(firstPane)
            },
            "再切回已打开的 tab 必须立刻有 Surface，不得重新 seeding"
        )

        app.testCloseTab(created)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return !app.testTabIDs().contains(created)
                    && app.testTabIDs().count == original.count
            },
            "关掉新建 tab 后 Core snapshot 必须回到原来的 tab 集合"
        )
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testPaneSurfaceReady(app.testActivePaneID())
            },
            "关掉 tab 后当前 Surface 仍必须可见"
        )

        app.testSwitchTab(secondTab)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testActiveTabID() == secondTab
                    && app.testPaneSurfaceReady(app.testActivePaneID())
            },
            "关 tab 之后再切入另一个已打开的 tab 不得卡死"
        )
        XCTAssertTrue(app.waitTerminalContains(painted.tab2Token))

        let runtimeWindows = Tmux.out(
            socket: painted.socket,
            args: ["list-windows", "-t", painted.session, "-F", "#{window_id}"]
        )
            .split(whereSeparator: \.isNewline)
            .map(String.init)
            .filter { !$0.isEmpty }
        XCTAssertEqual(
            Set(runtimeWindows),
            Set(app.testTabIDs().map { "@\($0)" }),
            "关 tab 后 tmux window 与 Core snapshot 仍必须一一对应"
        )
    }

    func testNewPaneAndTabFocusTerminalInput() throws {
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("focus-input")
        let session = "focus-input"
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "80", "-y", "24", "--", "/bin/cat",
        ])
        let app = try AppE2E.attachWindow(socket: socket, session: session)
        defer { app.testShutdown() }
        app.window?.makeKeyAndOrderFront(nil)
        XCTAssertTrue(app.waitReady(minTabs: 1, minLeaves: 1), "attach 后应有 1 个 pane")

        func cursorInActiveTerminal() -> Bool {
            app.testPollOnce()
            app.testFlushFeeds()
            app.testRestoreTerminalFocus()
            let active = app.testActivePaneID()
            guard app.testPaneSurfaceReady(active) else { return false }
            guard app.testFocusTargetPaneID() == active else { return false }
            app.testMakeActiveTerminalFirstResponder()
            return app.testFocusedTerminalPaneID() == active
                && !app.testFirstResponderIsPaneHost()
        }

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout, cursorInActiveTerminal),
            "attach 后键盘目标必须是 SwiftTerm。target=\(String(describing: app.testFocusTargetPaneID())) active=\(app.testActivePaneID()) responder=\(String(describing: app.testFocusedTerminalPaneID()))"
        )

        let beforeLeaves = app.testLayoutLeafIDs()
        app.testSplitHorizontal()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                cursorInActiveTerminal()
                    && app.testLayoutLeafIDs().count == beforeLeaves.count + 1
            },
            "新建 pane 后光标必须在新 pane 的 SwiftTerm 输入里，不能停在 host。active=\(app.testActivePaneID()) target=\(String(describing: app.testFocusTargetPaneID())) host=\(app.testFirstResponderIsPaneHost()) leaves=\(app.testLayoutLeafIDs())"
        )

        let beforeTabs = app.testTabIDs()
        app.testNewTab()
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                let ids = app.testTabIDs()
                return cursorInActiveTerminal()
                    && ids.count == beforeTabs.count + 1
                    && ids.contains(where: { !beforeTabs.contains($0) })
            },
            "新建 tab 后光标必须在新 tab 的 SwiftTerm 输入里。active=\(app.testActivePaneID()) target=\(String(describing: app.testFocusTargetPaneID()))"
        )
    }
}
