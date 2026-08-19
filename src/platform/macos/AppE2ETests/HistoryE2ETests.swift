import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// W16a：离屏历史可搜索、搜索跳转能看见、回底回到尾部。
///
/// 触控板由 SwiftTerm native scrollback 处理；attach 之后的 live 字节无论
/// 当前视口位置都必须继续进 SwiftTerm。
final class HistoryE2ETests: XCTestCase {
    func testAttachRestoresOffscreenHistoryAndJumpLatest() throws {
        let fx = OffscreenHistory(label: "gtk-hist")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 1), "attach 后应有 pane 控件")
        XCTAssertTrue(
            app.waitTerminalContains(fx.tailMark, timeout: AppE2E.featureTimeout),
            "可见尾标 \(fx.tailMark) 必须在 SwiftTerm 里"
        )

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                return !app.testSearchAll(fx.token).isEmpty
            },
            "search_all 必须找到滚出可见区的 \(fx.token)"
        )

        let pane = try XCTUnwrap(app.testLayoutLeafIDs().first, "至少 1 个 pane")
        app.testSetPaneViewport(1000)
        AppE2E.pump(80)
        app.testFlushFeeds()
        XCTAssertTrue(
            app.testPaneTerminalText(pane).contains(fx.token)
                || app.testAllVisibleTerminalText().contains(fx.token),
            "搜索跳转式滚到顶之后 SwiftTerm 必须能看见离屏历史 \(fx.token)"
        )
        XCTAssertTrue(app.testJumpLatestVisible(), "向上滚动后必须出现回底按钮 muxterm.jumpLatest")
        XCTAssertGreaterThan(app.testPaneViewport(), 0, "滚离底部后 viewport 应 > 0")

        app.testClickJumpLatest()
        AppE2E.pump(80)
        app.testFlushFeeds()
        let after = app.testPaneTerminalText(pane)
        XCTAssertFalse(
            after.contains(fx.token),
            "点回底之后可见区应回到尾部，不应再显示离屏 token。got=\(after)"
        )
        XCTAssertTrue(after.contains(fx.tailMark), "点回底之后可见区应含尾标 \(fx.tailMark)")
        XCTAssertFalse(app.testJumpLatestVisible(), "回底后按钮应隐藏")
    }

    /// 1124 数据回归：pane 里已有离屏历史时，后续 live 字节仍必须进 SwiftTerm。
    func testLiveBytesReachSwiftTermOnHistoryFilledPane() throws {
        let fx = OffscreenHistory(label: "hist-live")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 1), "attach 后应有 pane 控件")
        XCTAssertTrue(
            app.waitTerminalContains(fx.tailMark, timeout: AppE2E.featureTimeout),
            "可见尾标必须在"
        )
        XCTAssertEqual(app.testPaneViewport(), 0, "attach 后必须在底部 live，不能误进历史冻结")

        // 触控板/native scrollback 路径必须真正改变视口；不能再是空操作。
        app.testScrollHistory(deltaLines: 80)
        XCTAssertGreaterThan(app.testPaneViewport(), 0, "滚轮上划必须进入 native scrollback")

        let live = "LIVE_AFTER_ATTACH_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.sendLiteral(socket: fx.socket, target: fx.pane, text: "\(live)\n")
        Tmux.waitCapture(socket: fx.socket, target: fx.pane, needle: live)
        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.featureTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testPaneViewport() > 0
            },
            "停在历史位置时 live feed 仍必须继续处理，不能让 core/VT 冻结"
        )
        XCTAssertFalse(
            app.testAllVisibleTerminalText().contains(live),
            "用户停在历史位置时新输出不应覆盖当前历史屏"
        )
        app.testScrollHistory(deltaLines: -10_000)
        XCTAssertEqual(app.testPaneViewport(), 0, "向下滚到底部必须回到最新")
        XCTAssertTrue(
            app.waitTerminalContains(live, timeout: AppE2E.featureTimeout),
            "回底后必须看到停留历史期间收到的 live 输出。got=\(app.testAllVisibleTerminalText())"
        )

        let cup = "CUP_LIVE_\(ProcessInfo.processInfo.processIdentifier)"
        Tmux.sendLiteral(
            socket: fx.socket,
            target: fx.pane,
            text: "\u{1b}[H\u{1b}[2J\(cup)\n"
        )
        Tmux.waitCapture(socket: fx.socket, target: fx.pane, needle: cup)
        XCTAssertTrue(
            app.waitTerminalContains(cup, timeout: AppE2E.featureTimeout),
            "CUP 末帧必须进 SwiftTerm（htop/Cursor 同路径）。got=\(app.testAllVisibleTerminalText())"
        )
        XCTAssertTrue(app.testAllVisibleTerminalText().contains(cup), "回底后应看到最新 CUP 画面")
    }

    /// 真实 macOS 用户路径：滚轮必须由 AppKit 分发到当前 pane，不能只靠
    /// `testScrollHistory()` 直接调用 SwiftTerm 的内部滚动方法。
    func testRealAppKitScrollWheelRevealsAttachHistory() throws {
        let fx = OffscreenHistory(label: "hist-wheel")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }

        XCTAssertTrue(app.waitReady(minLeaves: 1), "attach 后应有 pane")
        let pane = try XCTUnwrap(app.testLayoutLeafIDs().first)
        XCTAssertTrue(app.testNativeCanScroll(), "attach seed 后 SwiftTerm 必须有 native scrollback")
        XCTAssertEqual(app.testPaneViewport(), 0, "初始必须在 live 尾部")

        XCTAssertTrue(app.testDispatchScrollWheel(deltaLines: 80), "必须能构造并分发滚轮事件")
        AppE2E.pump(80)
        app.testPollOnce()
        app.testFlushFeeds()
        XCTAssertLessThan(app.testNativeScrollPosition(), 0.999, "真实上划后 native position 必须离底")
        XCTAssertGreaterThan(app.testPaneViewport(), 0, "真实上划后 core viewport 必须 > 0")
        XCTAssertTrue(
            app.testPaneTerminalText(pane).contains(fx.token),
            "真实鼠标上划后必须看见 attach 前的离屏 token"
        )

        XCTAssertTrue(app.testDispatchScrollWheel(deltaLines: -10_000), "必须能分发下划事件")
        AppE2E.pump(80)
        app.testPollOnce()
        app.testFlushFeeds()
        XCTAssertEqual(app.testPaneViewport(), 0, "真实下划到底部后 core viewport 必须归零")
        XCTAssertGreaterThanOrEqual(app.testNativeScrollPosition(), 0.999, "真实下划到底部后 native 必须在最新位置")
        XCTAssertTrue(app.testPaneTerminalText(pane).contains(fx.tailMark), "回底后必须看见尾标")
    }

    func testPolicyForbidsStealingLiveScroll() {
        XCTAssertFalse(PaneHistoryScrollPolicy.stealsLiveTrackpad)
        XCTAssertFalse(PaneHistoryScrollPolicy.stealsLivePageKeys)
        XCTAssertFalse(PaneHistoryScrollPolicy.shouldReplaceLiveScreen(isSearchJump: false))
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedLive(viewport: 0))
        XCTAssertTrue(PaneOutputFeedPolicy.shouldFeedLive(viewport: 80))
    }
}
