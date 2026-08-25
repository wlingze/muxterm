import AppKit
import XCTest
@testable import MuxtermAppLib

/// Surface seed 的 AppKit 可见性契约：host 不能在 SwiftTerm 分块恢复期间
/// 暴露空白/半截帧，seed 与同期间 live catch-up 完成后才一次性显示。
final class SurfaceVisibilityE2ETests: XCTestCase {
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
