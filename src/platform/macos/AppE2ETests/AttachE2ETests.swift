import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// W13：attach 已有 2tab/3pane，SwiftTerm 非空、几何 ≥ 40px、切 tab 像素还在、CUP 洪水有上界。
final class AttachE2ETests: XCTestCase {
    func testSizedAttachPublishesSurfaceSnapshotWithoutFrontendResize() throws {
        try assertSizedAttachPublishesSnapshot(
            size: (125, 51),
            label: "attach-sized"
        )
    }

    func testSmallSizedAttachPublishesSurfaceSnapshotWithoutFrontendResize() throws {
        try assertSizedAttachPublishesSnapshot(
            size: (42, 12),
            label: "attach-small"
        )
    }

    /// 回归 2026-08-28 真机白屏：Quick Panel attach 后只有手动改变窗口
    /// 大小才出现内容。这里先固定窗口，再走生产 Existing attach；attach
    /// 开始后绝不 setFrame，且断言最终 host 与 SwiftTerm 都已有首帧。
    func testExistingAttachPaintsSurfaceWithoutPostAttachWindowResize() throws {
        try assertExistingAttachPaintsWithoutResize(
            frame: AppE2E.fixedWindowFrame(width: 620, height: 360),
            label: "attach-no-resize"
        )
    }

    func testSmallWindowExistingAttachPaintsWithoutPostAttachResize() throws {
        try assertExistingAttachPaintsWithoutResize(
            frame: AppE2E.fixedWindowFrame(width: 500, height: 320),
            label: "attach-small-window"
        )
    }

    /// 单 token 只能证明“有字节”，抓不住 agent 风格画面的历史和输入区
    /// 因列宽错位而混乱。这里用真实隔离 tmux + python3 动态全屏 TUI
    /// 验证通用历史机制；真实 `pi` 进程身份由下面两条回归覆盖。
    /// 测试走生产 Existing attach，并在 attach 后禁止改窗口尺寸。
    func testExistingAttachPaintsAgentHistoryAndInputWithoutResize() throws {
        let fixture = AgentScreenWorkspace(label: "attach-agent-no-resize")
        AppE2E.ensureApp()
        let startupBridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(
            bridge: startupBridge,
            debug: true,
            quickConnectStore: QuickConnectStore()
        )
        defer { app.testShutdown() }

        let fixedFrame = AppE2E.fixedWindowFrame(width: 620, height: 360)
        app.window?.setFrame(fixedFrame, display: true)
        app.window?.orderFront(nil)
        AppE2E.pump(160)

        app.testAttachExistingConnection(ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(
                name: fixture.session,
                windowCount: 1,
                attached: false
            ),
            socket: fixture.socket
        ))

        let painted = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            let pane = app.testActivePaneID()
            let text = app.testActivePaneTerminalText()
            AppE2E.pump(30)
            return app.testActiveWorkspaceSession() == fixture.session
                && app.testPaneSurfaceReady(pane)
                && text.contains("TOKEN_HEADER")
                && text.contains("TOKEN_BODY")
                && text.contains("TOKEN_PROMPT")
                && app.testNativeCanScroll()
        }
        let live = app.testActivePaneTerminalText()
        XCTAssertTrue(
            painted,
            "Existing attach 首屏必须同时保留 agent 顶栏/正文/输入区与历史。got=\(live)"
        )
        XCTAssertEqual(app.window?.frame, fixedFrame, "agent attach 后测试没有 resize 窗口")

        app.testScrollHistory(deltaLines: 10_000)
        let historyVisible = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            AppE2E.pump(20)
            return app.testActivePaneTerminalText().contains(fixture.historyToken)
        }
        XCTAssertTrue(
            historyVisible,
            "attach 前 agent 历史必须在 SwiftTerm native scrollback 中可读"
        )

        app.testScrollHistory(deltaLines: -10_000)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                let text = app.testActivePaneTerminalText()
                return text.contains("TOKEN_HEADER") && text.contains("TOKEN_PROMPT")
            },
            "回到尾部后 agent 顶栏和输入区仍必须正确"
        )
        XCTAssertEqual(app.window?.frame, fixedFrame, "历史验证全程没有 resize 窗口")
    }

    /// 1320/1437 的组合边界：pi 在 primary screen 且不开 mouse。它的当前
    /// Surface 必须保持完整，离屏消息则要清洗成文本后进入 native scrollback。
    func testPrimaryPiUpperPanePublishesSanitizedHistoryBackfill() throws {
        let fixture = try PrimaryPiSplitWorkspace(label: "primary-pi-history")
        let bridge = try CoreBridge.connect(
            backendType: "tmux",
            socket: fixture.socket,
            session: fixture.session,
            initialClientSize: (94, 51)
        )
        defer { bridge.shutdown() }

        var snapshots: [UInt32: Data] = [:]
        var histories: [UInt32: Data] = [:]
        let observed = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            for event in bridge.pollEvents() {
                if event.isPaneSnapshot {
                    snapshots[event.paneId] = event.data
                } else if event.isPaneHistory {
                    histories[event.paneId] = event.data
                }
            }
            let top = String(
                decoding: snapshots[fixture.topPaneId] ?? Data(),
                as: UTF8.self
            )
            let bottom = String(
                decoding: snapshots[fixture.bottomPaneId] ?? Data(),
                as: UTF8.self
            )
            return top.contains("PI_E2E_HEADER")
                && top.contains("PI_E2E_BODY")
                && top.contains("PI_E2E_PROMPT")
                && bottom.contains(fixture.bottomToken)
                && String(decoding: histories[fixture.topPaneId] ?? Data(), as: UTF8.self)
                    .contains("PI_E2E_HISTORY_200")
                && histories[fixture.bottomPaneId] != nil
        }

        XCTAssertTrue(
            observed,
            "真实上下分屏 attach 应播种两个 pane，并给 pi/cat 回填历史；snapshots=\(snapshots.mapValues(\.count)) histories=\(histories.mapValues(\.count))"
        )
        let topHistory = String(
            decoding: histories[fixture.topPaneId] ?? Data(),
            as: UTF8.self
        )
        XCTAssertFalse(
            topHistory.contains("\u{1b}"),
            "pi history 必须剥离 OSC/CSI，不能把控制序列当 VT 流重放"
        )
    }

    /// 与上面的 core 事件断言配套，验证生产 MainWindow/SwiftTerm 路径：
    /// 上方 pi 和下方 cat 在 attach 后不 resize 窗口也都直接显示。
    func testPrimaryPiUpperSplitPaintsWithoutPostAttachResize() throws {
        let fixture = try PrimaryPiSplitWorkspace(label: "primary-pi-surface")
        AppE2E.ensureApp()
        let startupBridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(
            bridge: startupBridge,
            debug: true,
            quickConnectStore: QuickConnectStore()
        )
        defer { app.testShutdown() }

        let fixedFrame = AppE2E.fixedWindowFrame(width: 620, height: 360)
        app.window?.setFrame(fixedFrame, display: true)
        app.window?.orderFront(nil)
        AppE2E.pump(160)
        app.testAttachExistingConnection(ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(
                name: fixture.session,
                windowCount: 1,
                attached: false
            ),
            socket: fixture.socket
        ))

        let painted = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            let top = app.testPaneTerminalText(fixture.topPaneId)
            let bottom = app.testPaneTerminalText(fixture.bottomPaneId)
            AppE2E.pump(30)
            return app.testActiveWorkspaceSession() == fixture.session
                && app.testLayoutLeafIDs().count == 2
                && app.testPaneSurfaceReady(fixture.topPaneId)
                && app.testPaneSurfaceReady(fixture.bottomPaneId)
                && top.contains("PI_E2E_HEADER")
                && top.contains("PI_E2E_BODY")
                && top.contains("PI_E2E_PROMPT")
                && bottom.contains(fixture.bottomToken)
                && app.testNativeCanScroll()
        }
        let top = app.testPaneTerminalText(fixture.topPaneId)
        XCTAssertTrue(painted, "上方真实 pi 首屏不得乱屏/白屏：\(top)")
        XCTAssertEqual(app.window?.frame, fixedFrame, "pi split attach 后测试没有 resize 窗口")

        // 再跨过 feed coalesce/history backfill 窗口，确认随后到达的后台事件
        // 没有把已经正确的 pi Surface 改坏。
        AppE2E.pump(600)
        app.testPollOnce()
        app.testFlushFeeds()
        let stableTop = app.testPaneTerminalText(fixture.topPaneId)
        XCTAssertTrue(stableTop.contains("PI_E2E_HEADER"), "后续事件不能覆盖 pi 顶栏：\(stableTop)")
        XCTAssertTrue(stableTop.contains("PI_E2E_PROMPT"), "后续事件不能覆盖 pi 输入区：\(stableTop)")

        XCTAssertTrue(app.testDispatchScrollWheel(deltaLines: 10_000), "pi pane 必须接收真实 AppKit 滚轮")
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                AppE2E.pump(20)
                return app.testPaneTerminalText(fixture.topPaneId)
                    .contains("PI_E2E_HISTORY_")
            },
            "上划后必须能看到 pi 的历史消息"
        )
        XCTAssertLessThan(app.testNativeScrollPosition(), 0.999, "上划后 pi native scrollback 必须离底")

        XCTAssertTrue(app.testDispatchScrollWheel(deltaLines: -10_000), "pi pane 必须能滚回最新消息")
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                let text = app.testPaneTerminalText(fixture.topPaneId)
                return text.contains("PI_E2E_HEADER") && text.contains("PI_E2E_PROMPT")
            },
            "滚回底部后 pi 顶栏与输入框必须保持正确"
        )
        XCTAssertEqual(app.window?.frame, fixedFrame, "稳定性检查也没有 resize 窗口")
    }

    private func assertExistingAttachPaintsWithoutResize(
        frame fixedFrame: NSRect,
        label: String
    ) throws {
        let fixture = OnePaneCat(label: label)
        AppE2E.ensureApp()
        let startupBridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(
            bridge: startupBridge,
            debug: true,
            quickConnectStore: QuickConnectStore()
        )
        defer { app.testShutdown() }

        app.window?.setFrame(fixedFrame, display: true)
        app.window?.orderFront(nil)
        AppE2E.pump(160)

        app.testAttachExistingConnection(ExistingConnectionChoice(
            target: .local,
            session: TmuxSessionInfo(
                name: fixture.session,
                windowCount: 1,
                attached: false
            ),
            socket: fixture.socket
        ))

        let hasVisibleText = AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            guard app.testActiveWorkspaceSession() == fixture.session else {
                AppE2E.pump(30)
                return false
            }
            let pane = app.testActivePaneID()
            // tmux 的 `%0` 是合法 pane id，不能拿 0 当“尚未就绪”哨兵。
            let ready = app.testPaneSurfaceReady(pane)
            let hasToken = app.testActivePaneTerminalText().contains(fixture.token)
            AppE2E.pump(30)
            return ready && hasToken
        }

        let activePane = app.testActivePaneID()
        let diagnostics = "session=\(app.testActiveWorkspaceSession() ?? "nil") "
            + "pane=\(activePane) ready=\(app.testPaneSurfaceReady(activePane)) "
            + "text=\(app.testActivePaneTerminalText())"
        XCTAssertTrue(
            painted,
            "Existing attach 必须在不改变窗口大小时直接显示首帧；\(diagnostics)"
        )
        XCTAssertEqual(app.window?.frame, fixedFrame, "attach 后测试没有 resize 窗口")
    }

    func testAttachPreexist2Tab3PaneIsUsable() throws {
        let painted = PaintedWorkspace(label: "gtk-attach")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(
            app.waitReady(minTabs: 2, minLeaves: 3),
            "attach 后应有 2 tab / 3 pane 布局，实际 \(app.testTabAndPaneCounts()) leaves=\(app.testLayoutLeafIDs())"
        )

        app.testFlushFeeds()
        for id in app.testLayoutLeafIDs() {
            let size = app.testPaneAllocation(id)
            XCTAssertGreaterThanOrEqual(
                size.width,
                AppE2E.minPanePx,
                "pane \(id) 宽 \(size.width) < \(AppE2E.minPanePx)px（白屏/错布局）"
            )
            XCTAssertGreaterThanOrEqual(
                size.height,
                AppE2E.minPanePx,
                "pane \(id) 高 \(size.height) < \(AppE2E.minPanePx)px（白屏/错布局）"
            )
        }

        let painted = AppE2E.wait(timeout: AppE2E.attachTimeout) {
            app.testPollOnce()
            app.testFlushFeeds()
            return !app
                .testAllVisibleTerminalText()
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .isEmpty
        }
        XCTAssertTrue(hasVisibleText, "attach 后 SwiftTerm 不能空（1820 白屏）")
        for token in painted.tab1Tokens {
            XCTAssertTrue(app.waitTerminalContains(token), "可见 pane 应含播种 token \(token)。vte=\(app.testAllVisibleTerminalText())")
        }

        let tabs = app.testTabIDs()
        let current = app.testActiveTabID()
        let other = try XCTUnwrap(tabs.first { $0 != current }, "应有第二个 tab")
        app.testClickStatusTab(other)
        XCTAssertTrue(
            app.waitTerminalContains(painted.tab2Token),
            "切到 tab 2 应看到 \(painted.tab2Token)"
        )

        app.testClickStatusTab(current)
        for token in painted.tab1Tokens {
            XCTAssertTrue(
                app.waitTerminalContains(token),
                "切回 tab 1 像素缓存应仍有 \(token)"
            )
        }

        Tmux.ok(socket: painted.socket, args: [
            "respawn-pane", "-k", "-t", painted.tab1Panes[0],
            "bash -c 'for i in $(seq 1 \(AppE2E.cupFloodFrames)); do printf \"\\033[H\\033[2Jframe-%s\\n\" \"$i\"; done; printf \"FLOOD_DONE\\n\"; exec /bin/cat'",
        ])

        let start = Date()
        var outputEvents = 0
        while Date().timeIntervalSince(start) < 1 {
            outputEvents += app.testPollOutputEventCount()
            AppE2E.pump(16)
        }
        XCTAssertLessThanOrEqual(
            outputEvents,
            AppE2E.maxOutputEventsPerSec,
            "AppKit 1s 内 PaneOutput=\(outputEvents) > \(AppE2E.maxOutputEventsPerSec)（1820 CPU）"
        )

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.attachTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                let t = app.testAllVisibleTerminalText()
                return t.contains("FLOOD_DONE") || t.contains("frame-")
            },
            "CUP 洪水后 SwiftTerm 应留下末帧，不能白屏"
        )
        XCTAssertFalse(
            app.testAllVisibleTerminalText().trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            "洪水后 SwiftTerm 仍不能空"
        )
    }

    private func assertSizedAttachPublishesSnapshot(
        size: (UInt16, UInt16),
        label: String
    ) throws {
        let fixture = OnePaneCat(label: label)
        let bridge = try CoreBridge.connect(
            backendType: "tmux",
            socket: fixture.socket,
            session: fixture.session,
            initialClientSize: size
        )
        defer { bridge.shutdown() }

        var snapshots: [StateChange] = []
        let received = AppE2E.wait(timeout: AppE2E.attachTimeout) {
            snapshots.append(contentsOf: bridge.pollEvents().filter(\.isPaneSnapshot))
            return snapshots.contains { event in
                !event.data.isEmpty &&
                    String(decoding: event.data, as: UTF8.self).contains(fixture.token)
            }
        }

        XCTAssertTrue(
            received,
            "带初始尺寸 \(size.0)x\(size.1) attach 后，即使前端不 resize，也必须发布含可见内容的 PaneSnapshot；实际 snapshots=\(snapshots.map { ($0.paneId, $0.data.count) })"
        )
    }
}
