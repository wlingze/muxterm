import AppKit
import XCTest
@testable import MuxtermAppLib

/// Surface seed 的 AppKit 可见性契约：host 不能在 SwiftTerm 分块恢复期间
/// 暴露空白/半截帧，seed 与同期间 live catch-up 完成后才一次性显示。
final class SurfaceVisibilityE2ETests: XCTestCase {
    func testPaneHostClipsTerminalGlyphsAtSplitBoundary() {
        AppE2E.ensureApp()
        let view = MuxTerminalView(
            paneId: 30,
            frame: NSRect(x: 0, y: 0, width: 400, height: 200)
        )
        let host = PaneHostView(paneId: 30, terminal: view)

        XCTAssertTrue(host.wantsLayer)
        XCTAssertEqual(
            host.layer?.masksToBounds,
            true,
            "最右侧 CJK glyph 不得越过 pane/split 边界"
        )
    }

    private func makeManager() throws -> (CoreBridge, TerminalManager) {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        return (bridge, TerminalManager(bridge: bridge))
    }

    func testHostStaysHiddenUntilSeedAndLiveCatchupFinish() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let host = PaneHostView(paneId: 1, terminal: view)
        manager.onSurfaceReadinessChanged = { _, ready in
            host.setSurfaceReady(ready)
        }
        host.setSurfaceReady(false)

        var seed = Data(repeating: 0x20, count: 96 * 1024)
        seed.append(contentsOf: Array("SEED_COMPLETE\r\n".utf8))
        manager.testQueueSurfaceSeed(paneId: 1, view: view, data: seed)
        manager.testQueueSurfaceLiveOutput(
            paneId: 1,
            data: Data("LIVE_CATCHUP\r\n".utf8)
        )

        XCTAssertTrue(host.isHidden, "seed 进行中不得显示 PaneHostView")
        XCTAssertFalse(manager.isSurfaceReady(for: 1))

        for _ in 0..<20 where !manager.isSurfaceReady(for: 1) {
            manager.testFlushSurfaceSeeds()
        }
        AppE2E.pump(20)

        XCTAssertTrue(manager.isSurfaceReady(for: 1))
        XCTAssertFalse(host.isHidden, "seed + live catch-up 完成后必须一次性显示")
        XCTAssertTrue(view.visibleScreenText().contains("LIVE_CATCHUP"))
    }

    func testEmptySurfaceSeedRevealsAsBlankFrame() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let view = MuxTerminalView(
            paneId: 2,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let host = PaneHostView(paneId: 2, terminal: view)
        manager.onSurfaceReadinessChanged = { _, ready in
            host.setSurfaceReady(ready)
        }
        host.setSurfaceReady(false)
        manager.testQueueSurfaceSeed(paneId: 2, view: view, data: Data())

        XCTAssertTrue(host.isHidden)
        manager.testFlushSurfaceSeeds()

        XCTAssertTrue(manager.isSurfaceReady(for: 2))
        XCTAssertFalse(host.isHidden, "空白 snapshot 也必须有可见的完整首帧")
    }

    /// 回归 2026-08-28 Legion pi：一次 poll 同时取到 attach snapshot 与
    /// continue/SIGWINCH 后的唯一最新 redraw。snapshot 是基线，排在它后面的
    /// PaneOutput 是新增量；若按 poll 批次整批抑制，界面会永久停在旧消息。
    func testSnapshotAndFollowingOutputInSameBatchPaintLatestFrame() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        manager.updatePaneSizes([
            Pane(id: 9, cols: 94, rows: 24, isActive: true),
        ])
        manager.beginEventBatch()
        manager.handleSnapshot(
            paneId: 9,
            data: Data("\u{1b}[H\u{1b}[2JSTALE_PI_FRAME".utf8)
        )
        manager.handleOutput(
            paneId: 9,
            data: Data("\u{1b}[H\u{1b}[2JLATEST_PI_REDRAW".utf8)
        )
        manager.endEventBatch()

        manager.flushSeedsNow(paneIds: [9])
        manager.testFlushFeeds()
        let text = manager.view(for: 9).visibleScreenText()
        XCTAssertTrue(text.contains("LATEST_PI_REDRAW"), "snapshot 后最新 redraw 必须可见。got=\(text)")
        XCTAssertFalse(text.contains("STALE_PI_FRAME"), "最新 redraw 应覆盖旧 frame。got=\(text)")
    }

    /// 94→125 是 14:37 日志的真实 attach resize。模型先扩列，再在同批应用
    /// resize 后 redraw；批次边界不能吞掉 SIGWINCH 产生的唯一新帧。
    func testResizeThenSnapshotAndLiveRedrawKeepsNewestWideFrame() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        manager.updatePaneSizes([
            Pane(id: 10, cols: 94, rows: 24, isActive: true),
        ])
        _ = manager.view(for: 10)
        manager.updatePaneSizes([
            Pane(id: 10, cols: 125, rows: 24, isActive: true),
        ])
        manager.beginEventBatch()
        manager.handleSnapshot(
            paneId: 10,
            data: Data("\u{1b}[H\u{1b}[2JPI_BEFORE_SIGWINCH".utf8)
        )
        manager.handleOutput(
            paneId: 10,
            data: Data("\u{1b}[H\u{1b}[2JPI_AFTER_SIGWINCH_125".utf8)
        )
        manager.endEventBatch()

        manager.flushSeedsNow(paneIds: [10])
        manager.testFlushFeeds()
        let view = manager.view(for: 10)
        XCTAssertEqual(view.getTerminal().cols, 125)
        let text = view.visibleScreenText()
        XCTAssertTrue(text.contains("PI_AFTER_SIGWINCH_125"), "resize 后最新 frame 必须可见。got=\(text)")
        XCTAssertFalse(text.contains("PI_BEFORE_SIGWINCH"), "resize 前 frame 不得残留。got=\(text)")
    }

    func testMultipleSeedsRemainHiddenIndividuallyUntilComplete() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let first = MuxTerminalView(
            paneId: 3,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let second = MuxTerminalView(
            paneId: 4,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let firstHost = PaneHostView(paneId: 3, terminal: first)
        let secondHost = PaneHostView(paneId: 4, terminal: second)
        manager.onSurfaceReadinessChanged = { paneId, ready in
            switch paneId {
            case 3: firstHost.setSurfaceReady(ready)
            case 4: secondHost.setSurfaceReady(ready)
            default: break
            }
        }
        firstHost.setSurfaceReady(false)
        secondHost.setSurfaceReady(false)
        manager.testQueueSurfaceSeed(
            paneId: 3,
            view: first,
            data: Data(repeating: 0x41, count: 128 * 1024)
        )
        manager.testQueueSurfaceSeed(
            paneId: 4,
            view: second,
            data: Data("SECOND\r\n".utf8)
        )

        XCTAssertTrue(firstHost.isHidden)
        XCTAssertTrue(secondHost.isHidden)
        for _ in 0..<40 where !manager.isSurfaceReady(for: 3)
            || !manager.isSurfaceReady(for: 4)
        {
            manager.testFlushSurfaceSeeds()
        }

        XCTAssertTrue(manager.isSurfaceReady(for: 3))
        XCTAssertTrue(manager.isSurfaceReady(for: 4))
        XCTAssertFalse(firstHost.isHidden)
        XCTAssertFalse(secondHost.isHidden)
    }

    func testReadySurfaceReseedDoesNotHideHost() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let host = PaneHostView(paneId: 1, terminal: view)
        manager.onSurfaceReadinessChanged = { _, ready in
            host.setSurfaceReady(ready)
        }
        host.setSurfaceReady(false)
        manager.testQueueSurfaceSeed(
            paneId: 1,
            view: view,
            data: Data("FIRST_FRAME\r\n".utf8)
        )
        for _ in 0..<10 where !manager.isSurfaceReady(for: 1) {
            manager.testFlushSurfaceSeeds()
        }
        AppE2E.pump(10)

        XCTAssertTrue(manager.isSurfaceReady(for: 1))
        XCTAssertFalse(host.isHidden)

        manager.handleSnapshot(paneId: 1, data: Data("SECOND_FRAME\r\n".utf8))
        XCTAssertTrue(
            manager.isSurfaceReady(for: 1),
            "已经在画的 Surface 再 seed 不得藏白"
        )
        XCTAssertFalse(host.isHidden, "切 tab / output-dropped 不得把已显示的 host 藏起来")

        for _ in 0..<10 {
            manager.testFlushSurfaceSeeds()
        }
        AppE2E.pump(10)

        XCTAssertTrue(manager.isSurfaceReady(for: 1))
        XCTAssertFalse(host.isHidden)
        XCTAssertTrue(view.visibleScreenText().contains("SECOND_FRAME"))
    }

    func testTmuxGridShrinkMovesPromptIntoView() throws {
        AppE2E.ensureApp()
        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 800, height: 900)
        )
        view.getTerminal().resize(cols: 128, rows: 63)
        view.feedOutput(Data(repeating: 0x0a, count: 62) + Data("PROMPT>\n".utf8), isSnapshot: true)
        view.applyGridSize(cols: 93, rows: 51, followTail: true)

        let dims = view.getTerminal().getDims()
        XCTAssertEqual(dims.cols, 93)
        XCTAssertEqual(dims.rows, 51)
        XCTAssertTrue(view.isAtLatest(), "缩小后必须回到底部，prompt 不能留在窗口下面")
        XCTAssertTrue(view.visibleScreenText().contains("PROMPT>"))
    }

    func testCachedTabHostIsReusedOnSecondVisit() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let layout = PaneLayoutView(terminalManager: manager)
        layout.frame = NSRect(x: 0, y: 0, width: 800, height: 400)
        let panes1 = [Pane(id: 1, cols: 80, rows: 24, isActive: true)]
        let panes2 = [Pane(id: 2, cols: 80, rows: 24, isActive: true)]

        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 1), panes: panes1, tabId: 10))
        let host1 = layout.testHost(for: 1)
        XCTAssertNotNil(host1)

        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 2), panes: panes2, tabId: 20))
        XCTAssertNotNil(layout.revealCachedTab(10))
        XCTAssertTrue(
            layout.testHost(for: 1) === host1,
            "第二次进入已加载的 tab 必须复用 host，不得重建 SwiftTerm"
        )
    }

    func testPrewarmMakesFirstVisitACacheHit() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let layout = PaneLayoutView(terminalManager: manager)
        layout.frame = NSRect(x: 0, y: 0, width: 800, height: 400)
        let panes1 = [Pane(id: 1, cols: 80, rows: 24, isActive: true)]
        let panes2 = [Pane(id: 2, cols: 80, rows: 24, isActive: true)]
        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 1), panes: panes1, tabId: 10))
        XCTAssertTrue(layout.prewarm(tabId: 20, layout: .leaf(paneId: 2), panes: panes2))
        XCTAssertTrue(layout.hasCachedTab(20))
        let host2 = layout.testHost(for: 2)
        XCTAssertNotNil(host2)
        XCTAssertNotNil(layout.revealCachedTab(20))
        XCTAssertTrue(
            layout.testHost(for: 2) === host2,
            "预热过的 tab 第一次点击必须复用 host"
        )
    }

    func testApplyingANewTabDoesNotDropCachedTrees() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let layout = PaneLayoutView(terminalManager: manager)
        layout.frame = NSRect(x: 0, y: 0, width: 800, height: 400)
        let panes1 = [Pane(id: 1, cols: 80, rows: 24, isActive: true)]
        let panes2 = [Pane(id: 2, cols: 80, rows: 24, isActive: true)]
        let panes3 = [Pane(id: 3, cols: 80, rows: 24, isActive: true)]
        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 1), panes: panes1, tabId: 10))
        let host1 = layout.testHost(for: 1)
        XCTAssertTrue(layout.prewarm(tabId: 20, layout: .leaf(paneId: 2), panes: panes2))
        let host2 = layout.testHost(for: 2)
        XCTAssertTrue(layout.apply(layout: .leaf(paneId: 3), panes: panes3, tabId: 30))
        XCTAssertTrue(layout.hasCachedTab(10), "新建 tab 不得把已打开的树丢掉")
        XCTAssertTrue(layout.hasCachedTab(20), "新建 tab 不得把预热树丢掉")
        XCTAssertNotNil(layout.revealCachedTab(10))
        XCTAssertTrue(layout.testHost(for: 1) === host1)
        XCTAssertNotNil(layout.revealCachedTab(20))
        XCTAssertTrue(layout.testHost(for: 2) === host2)
        layout.pruneTabs(keeping: [10, 20])
        XCTAssertFalse(layout.hasCachedTab(30), "关掉的 tab 才从缓存里拿掉")
        XCTAssertTrue(layout.hasCachedTab(10))
        XCTAssertTrue(layout.hasCachedTab(20))
    }

    func testReplaceTerminalManagerRestoresParkedHosts() throws {
        let (bridgeA, managerA) = try makeManager()
        let (bridgeB, managerB) = try makeManager()
        defer {
            bridgeA.shutdown()
            bridgeB.shutdown()
        }
        let layout = PaneLayoutView(terminalManager: managerA)
        layout.frame = NSRect(x: 0, y: 0, width: 800, height: 400)
        XCTAssertTrue(
            layout.apply(
                layout: .leaf(paneId: 1),
                panes: [Pane(id: 1, cols: 80, rows: 24, isActive: true)],
                tabId: 10
            )
        )
        let host = layout.testHost(for: 1)
        XCTAssertNotNil(host)
        XCTAssertFalse(
            layout.replaceTerminalManager(managerB),
            "第一次切到 B 没有停驻树，不得假装已经加载完"
        )
        _ = layout.apply(
            layout: .leaf(paneId: 2),
            panes: [Pane(id: 2, cols: 80, rows: 24, isActive: true)],
            tabId: 20
        )
        XCTAssertTrue(
            layout.replaceTerminalManager(managerA),
            "切回 A 必须挂上停驻树，不能等待重新加载"
        )
        XCTAssertTrue(
            layout.testHost(for: 1) === host,
            "切回 Workspace 必须复用原来的 SwiftTerm host"
        )
    }

    func testBackgroundSlotKeepsFeedingExistingSurface() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }
        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 320, height: 180)
        )
        manager.testQueueSurfaceSeed(paneId: 1, view: view, data: Data("FG_SEED\r\n".utf8))
        manager.testFlushSurfaceSeeds()
        manager.setViewCreationEnabled(false)
        manager.handleOutput(paneId: 1, data: Data("BG_LIVE\r\n".utf8))
        manager.testFlushFeeds()
        XCTAssertTrue(
            view.visibleScreenText().contains("BG_LIVE"),
            "后台 Workspace 必须继续喂已有 SwiftTerm，切回来才不会卡住加载"
        )
        manager.handleOutput(paneId: 2, data: Data("NEW_PANE\r\n".utf8))
        XCTAssertFalse(manager.hasView(for: 2), "后台不得为从未打开的 pane 建 SwiftTerm")
    }

    func testStashedSnapshotSeedsOnFirstView() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }
        manager.setViewCreationEnabled(false)
        manager.handleSnapshot(paneId: 7, data: Data("STASHED_PROMPT\r\n".utf8))
        XCTAssertFalse(manager.hasView(for: 7), "后台不得为从未打开的 pane 建 SwiftTerm")
        manager.setViewCreationEnabled(true)
        let view = manager.view(for: 7)
        manager.flushSeedsNow(paneIds: [7])
        XCTAssertTrue(manager.isSurfaceReady(for: 7))
        XCTAssertTrue(
            view.visibleScreenText().contains("STASHED_PROMPT"),
            "第一次建 Surface 必须立刻种上后台留住的快照。got=\(view.visibleScreenText())"
        )
    }

    func testBackgroundSnapshotReplacesStalePendingFrame() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        manager.setViewCreationEnabled(false)
        manager.handleFrame(paneId: 7, data: Data("STALE_FRAME\r\n".utf8))
        manager.handleSnapshot(paneId: 7, data: Data("AUTHORITATIVE_SNAPSHOT\r\n".utf8))

        manager.setViewCreationEnabled(true)
        let view = manager.view(for: 7)
        manager.flushSeedsNow(paneIds: [7])

        let text = view.visibleScreenText()
        XCTAssertTrue(text.contains("AUTHORITATIVE_SNAPSHOT"), "应显示最新 snapshot。got=\(text)")
        XCTAssertFalse(text.contains("STALE_FRAME"), "旧 full frame 不得在 snapshot 后重放。got=\(text)")
    }

    func testBackgroundSnapshotDropsOlderStashedOutput() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        manager.setViewCreationEnabled(false)
        manager.handleOutput(paneId: 7, data: Data("OLD_INCREMENT\r\n".utf8))
        manager.handleSnapshot(paneId: 7, data: Data("NEW_BASELINE\r\n".utf8))

        manager.setViewCreationEnabled(true)
        let view = manager.view(for: 7)
        manager.flushSeedsNow(paneIds: [7])

        let text = view.visibleScreenText()
        XCTAssertTrue(text.contains("NEW_BASELINE"), "应显示新 snapshot。got=\(text)")
        XCTAssertFalse(
            text.contains("OLD_INCREMENT"),
            "snapshot 之前缓存的 output 已被覆盖，不能在 snapshot 后重放。got=\(text)"
        )
    }

    func testBackgroundOutputOverflowWaitsForAuthoritativeSnapshot() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }
        let paneId: UInt32 = 7
        var requested: [UInt32] = []
        manager.onAuthoritativeSnapshotRequired = { pane in
            requested.append(pane)
            return true
        }

        manager.setViewCreationEnabled(false)
        manager.handleSnapshot(paneId: paneId, data: Data("STALE_BASELINE\r\n".utf8))
        var overflow = Data("\u{1b}]1337;unterminated=".utf8)
        overflow.append(Data(repeating: 0x76, count: 300 * 1024))
        overflow.append(Data("\u{1b}[38;2;118;".utf8))
        manager.handleOutput(paneId: paneId, data: overflow)

        manager.setViewCreationEnabled(true)
        let view = manager.view(for: paneId)
        manager.flushSeedsNow(paneIds: [paneId])

        XCTAssertEqual(requested, [paneId], "溢出后必须只请求一次权威 pane snapshot")
        XCTAssertFalse(
            manager.isSurfaceReady(for: paneId),
            "新 snapshot 到达前不得把旧 baseline 或任意 suffix 标成可显示"
        )
        XCTAssertFalse(view.visibleScreenText().contains("vvvvvvvv"))

        manager.handleOutput(paneId: paneId, data: Data("DROPPED_WHILE_FENCED\r\n".utf8))
        manager.setViewCreationEnabled(true)
        XCTAssertEqual(requested, [paneId], "等待权威 snapshot 时不得重复请求或恢复 live")

        manager.beginEventBatch()
        manager.handleSnapshot(
            paneId: paneId,
            data: Data("\u{1b}[38;2;36;41;46mCURRENT_FRAME\u{1b}[0m\r\n".utf8)
        )
        manager.handleOutput(paneId: paneId, data: Data("LIVE_AFTER_SNAPSHOT\r\n".utf8))
        manager.endEventBatch()
        manager.flushSeedsNow(paneIds: [paneId])

        let text = view.visibleScreenText()
        XCTAssertTrue(manager.isSurfaceReady(for: paneId))
        XCTAssertTrue(text.contains("CURRENT_FRAME"), "应显示恢复后的权威 frame。got=\(text)")
        XCTAssertTrue(text.contains("LIVE_AFTER_SNAPSHOT"), "snapshot 后的 live 必须保留。got=\(text)")
        XCTAssertFalse(text.contains("STALE_BASELINE"), "旧 baseline 不得复活。got=\(text)")
        XCTAssertFalse(text.contains("DROPPED_WHILE_FENCED"), "fence 期间 live 不得进入 parser。got=\(text)")
        XCTAssertFalse(text.contains("118;"), "半截 SGR suffix 不得进入 SwiftTerm。got=\(text)")
    }

    func testBackgroundOutputOverflowRejectedRequestUsesOnlySafeBaseline() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }
        let paneId: UInt32 = 7
        var requested: [UInt32] = []
        manager.onAuthoritativeSnapshotRequired = { pane in
            requested.append(pane)
            return false
        }

        manager.setViewCreationEnabled(false)
        manager.handleSnapshot(paneId: paneId, data: Data("SAFE_BASELINE\r\n".utf8))
        manager.handleOutput(
            paneId: paneId,
            data: Data(repeating: 0x78, count: 300 * 1024)
        )

        manager.setViewCreationEnabled(true)
        let view = manager.view(for: paneId)
        manager.flushSeedsNow(paneIds: [paneId])

        let text = view.visibleScreenText()
        XCTAssertEqual(requested, [paneId])
        XCTAssertTrue(manager.isSurfaceReady(for: paneId))
        XCTAssertTrue(text.contains("SAFE_BASELINE"), "拒绝重拍时只能显示旧安全 baseline。got=\(text)")
        XCTAssertFalse(text.contains("xxxxxxxx"), "溢出的任意 suffix 都不得显示。got=\(text)")
    }

    func testBackgroundOutputOverflowRecoversFromFullFrameAndFollowingLive() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }
        let paneId: UInt32 = 7
        var requested: [UInt32] = []
        manager.onAuthoritativeSnapshotRequired = { pane in
            requested.append(pane)
            return true
        }

        manager.setViewCreationEnabled(false)
        manager.handleSnapshot(paneId: paneId, data: Data("STALE_BASELINE\r\n".utf8))
        manager.handleOutput(
            paneId: paneId,
            data: Data(repeating: 0x79, count: 300 * 1024)
        )
        manager.setViewCreationEnabled(true)
        let view = manager.view(for: paneId)

        manager.beginEventBatch()
        manager.handleFrame(
            paneId: paneId,
            data: Data("\u{1b}[2J\u{1b}[HFULL_FRAME\r\n".utf8)
        )
        manager.handleOutput(paneId: paneId, data: Data("LIVE_AFTER_FRAME\r\n".utf8))
        manager.endEventBatch()
        manager.testFlushFeeds()

        let text = view.visibleScreenText()
        XCTAssertEqual(requested, [paneId])
        XCTAssertTrue(manager.isSurfaceReady(for: paneId))
        XCTAssertTrue(text.contains("FULL_FRAME"), "Herdr full frame 应建立新 baseline。got=\(text)")
        XCTAssertTrue(text.contains("LIVE_AFTER_FRAME"), "full frame 后的 diff/live 必须保留。got=\(text)")
        XCTAssertFalse(text.contains("STALE_BASELINE"), "旧 snapshot 不得在 full frame 后复活。got=\(text)")
        XCTAssertFalse(text.contains("yyyyyyyy"), "溢出的 byte suffix 不得显示。got=\(text)")
    }

    func testProductionOverflowRequestRoundTripsThroughFfiAndTmux() throws {
        AppE2E.ensureApp()
        AppE2E.requireTmux()
        let socket = Tmux.uniqueSocket("surface-overflow")
        let session = "surface-overflow"
        Tmux.killServer(socket)
        defer { Tmux.killServer(socket) }
        Tmux.ok(socket: socket, args: [
            "-f", "/dev/null", "new-session", "-d", "-s", session,
            "-x", "100", "-y", "30", "--", "/bin/cat",
        ])
        let target = Tmux.out(
            socket: socket,
            args: ["list-panes", "-t", session, "-F", "#{pane_id}"]
        )
        let token = "AUTHORITATIVE_TMUX_FRAME_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.sendLiteral(socket: socket, target: target, text: token)
        Tmux.sendHex(socket: socket, target: target, bytes: [0x0d])
        Tmux.waitCapture(socket: socket, target: target, needle: token)

        let bridge = try CoreBridge.connect(
            backendType: "tmux",
            socket: socket,
            session: session,
            initialClientSize: (100, 30)
        )
        defer { bridge.shutdown() }
        let manager = TerminalManager(bridge: bridge)
        manager.setViewCreationEnabled(false)
        let paneId = try XCTUnwrap(bridge.snapshot().panes.first?.id)
        manager.updatePaneSizes(bridge.snapshot().panes)

        var initialSnapshotSeen = false
        func route(_ events: [StateChange]) {
            manager.beginEventBatch()
            for event in events {
                if event.isPaneSnapshot {
                    initialSnapshotSeen = true
                    manager.handleSnapshot(paneId: event.paneId, data: event.data)
                } else if event.isPaneFrame {
                    manager.handleFrame(paneId: event.paneId, data: event.data)
                } else if event.isPaneHistory {
                    manager.handleHistory(paneId: event.paneId, data: event.data)
                } else if event.isPaneOutput {
                    manager.handleOutput(paneId: event.paneId, data: event.data)
                }
            }
            manager.endEventBatch()
        }

        XCTAssertTrue(AppE2E.wait(timeout: 8) {
            route(bridge.pollEvents())
            return initialSnapshotSeen
        }, "attach 必须先建立初始 pane snapshot")
        // 排空初始 capture 后的 history/live，避免把它误认成恢复请求的响应。
        for _ in 0..<8 {
            route(bridge.pollEvents())
            AppE2E.pump(20)
        }

        manager.handleOutput(
            paneId: paneId,
            data: Data(repeating: 0x7a, count: 300 * 1024)
        )
        initialSnapshotSeen = false
        manager.setViewCreationEnabled(true)
        let view = manager.view(for: paneId)

        XCTAssertTrue(AppE2E.wait(timeout: 8) {
            route(bridge.pollEvents())
            return initialSnapshotSeen
        }, "生产 CoreBridge task 必须经 FFI 触发 tmux 权威 snapshot")
        manager.flushSeedsNow(paneIds: [paneId])
        manager.testFlushFeeds()

        let text = view.visibleScreenText()
        XCTAssertTrue(manager.isSurfaceReady(for: paneId))
        XCTAssertTrue(text.contains(token), "恢复后的 tmux capture 应包含真实 pane 内容。got=\(text)")
        XCTAssertFalse(text.contains("zzzzzzzz"), "本地溢出的任意 suffix 不得进入 SwiftTerm。got=\(text)")
    }

    func testVisibleSeedIsNotStuckBehindBackgroundSeeds() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }
        var background = Data(repeating: 0x20, count: 96 * 1024)
        background.append(contentsOf: Array("BG_DONE\r\n".utf8))
        let bgView = MuxTerminalView(paneId: 2, frame: NSRect(x: 0, y: 0, width: 320, height: 180))
        let fgView = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 320, height: 180))
        manager.testQueueSurfaceSeed(paneId: 2, view: bgView, data: background)
        manager.testQueueSurfaceSeed(paneId: 1, view: fgView, data: Data("FG_READY\r\n".utf8))
        manager.flushSeedsNow(paneIds: [1])
        XCTAssertTrue(manager.isSurfaceReady(for: 1), "可见 pane 不得排在后台 seed 后面")
        XCTAssertFalse(manager.isSurfaceReady(for: 2), "后台大 seed 可以下一拍再跑")
    }

    func testCopySelectionAndPasteSendsBytes() throws {
        AppE2E.ensureApp()
        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        view.getTerminal().resize(cols: 80, rows: 24)
        view.feedOutput(Data("COPY_ME hello\n".utf8), isSnapshot: true)
        view.selectAll()
        XCTAssertTrue(view.getSelection()?.contains("COPY_ME") == true)

        view.copy(nil)
        XCTAssertTrue(
            NSPasteboard.general.string(forType: .string)?.contains("COPY_ME") == true,
            "选区必须进系统剪贴板"
        )

        let handler = ClipboardRecordingHandler()
        view.inputHandler = handler
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("PASTE_ME", forType: .string)
        view.paste(nil)
        XCTAssertTrue(
            String(bytes: handler.bytes, encoding: .utf8)?.contains("PASTE_ME") == true,
            "粘贴必须把剪贴板字节发给 pane"
        )
    }

    func testPrependHistoryDoesNotResetOrHideVisibleTail() throws {
        AppE2E.ensureApp()
        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        view.getTerminal().resize(cols: 80, rows: 24)
        view.feedOutput(Data("HIST_TAIL\n".utf8), isSnapshot: true)
        XCTAssertEqual(view.snapshotResetCount, 1)
        view.prependHistoryLines(["HIST_OFFSCREEN", "pad-01"])
        XCTAssertEqual(view.snapshotResetCount, 1, "历史回填不得再 reset")
        XCTAssertTrue(view.historyPrepended)
        XCTAssertTrue(view.canScroll, "补上 attach 前历史后必须能上划")
        view.scrollLines(80)
        XCTAssertTrue(
            view.visibleScreenText().contains("HIST_OFFSCREEN"),
            "上划后必须看见离屏 token。got=\(view.visibleScreenText())"
        )
        view.scrollToLatest()
        XCTAssertTrue(
            view.visibleScreenText().contains("HIST_TAIL"),
            "回底后必须看见尾标。got=\(view.visibleScreenText())"
        )
    }
}

private final class ClipboardRecordingHandler: TerminalInputHandler {
    var bytes: [UInt8] = []
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        bytes.append(contentsOf: data)
    }
    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {}
}
