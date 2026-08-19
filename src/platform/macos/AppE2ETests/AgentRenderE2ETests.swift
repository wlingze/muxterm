import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 生产日志 `test-2026-0817-1457.log`：attach 后 `refresh-client -r` 上报
/// OSC 10/11。主题与终端颜色绑定：默认浅色是黑字白底；深色才是浅字深底。
final class AgentRenderE2ETests: XCTestCase {
    func testReportedOscColorsFollowActivePalette() throws {
        AppE2E.ensureApp()
        MuxtermTerminalColors.activePalette = .light
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 640, height: 360))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 360),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        AppE2E.pump(40)

        let colors = view.themeHexColors()
        XCTAssertEqual(
            colors.fg.lowercased(),
            MuxtermPalette.light.fg,
            "未切主题时 OSC 10 必须是浅色前景。got fg=\(colors.fg)"
        )
        XCTAssertEqual(
            colors.bg.lowercased(),
            MuxtermPalette.light.bg,
            "未切主题时 OSC 11 必须是浅色背景（白色）。got bg=\(colors.bg)"
        )
        let fg = luminance(colors.fg)
        let bg = luminance(colors.bg)
        XCTAssertGreaterThan(
            bg,
            fg,
            "浅色主题必须是深字浅底。got fg=\(colors.fg) bg=\(colors.bg)"
        )

        view.applyPalette(.dark)
        let dark = view.themeHexColors()
        XCTAssertEqual(dark.fg.lowercased(), MuxtermPalette.dark.fg)
        XCTAssertEqual(dark.bg.lowercased(), MuxtermPalette.dark.bg)
        XCTAssertGreaterThan(
            luminance(dark.fg),
            luminance(dark.bg),
            "深色主题必须是浅字深底。got fg=\(dark.fg) bg=\(dark.bg)"
        )
        window.orderOut(nil)
    }

    func testCaretIsOnScreenAfterPrompt() throws {
        AppE2E.ensureApp()
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 640, height: 360))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 360),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        view.feedOutput(Data("prompt> ".utf8))
        view.forceRedraw()
        AppE2E.pump(80)

        let caret = view.caretFrame
        XCTAssertGreaterThan(caret.width, 1, "SwiftTerm caret 宽度必须 > 1pt。frame=\(caret)")
        XCTAssertGreaterThan(caret.height, 1, "SwiftTerm caret 高度必须 > 1pt。frame=\(caret)")
        XCTAssertTrue(
            view.bounds.intersects(caret),
            "caret 必须落在终端 bounds 内。caret=\(caret) bounds=\(view.bounds)"
        )
        window.orderOut(nil)
    }

    func testEraseUpRedrawKeepsLastAgentFrameOnView() {
        AppE2E.ensureApp()
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 400),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        AppE2E.pump(40)

        view.feedOutput(Data("STATUS-A\r\nTIP\r\nBOX\r\nFOOTER-A\r\n".utf8))
        for frame in ["B", "C"] {
            let payload =
                "\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A" +
                "\u{1b}[GSTATUS-\(frame)\r\nTIP\r\nBOX\r\nFOOTER-\(frame)\r\n"
            view.feedOutput(Data(payload.utf8))
        }
        AppE2E.pump(40)
        let text = view.visibleScreenText()
        XCTAssertTrue(text.contains("STATUS-C"), "应停在末帧 STATUS-C。got=\(text)")
        XCTAssertTrue(text.contains("FOOTER-C"), "末帧 FOOTER-C 必须在。got=\(text)")
        XCTAssertFalse(text.contains("STATUS-A"), "旧帧 STATUS-A 不得残留/堆叠。got=\(text)")
        XCTAssertFalse(text.contains("FOOTER-A"), "旧帧 FOOTER-A 不得残留。got=\(text)")
        window.orderOut(nil)
    }

    func testAttachedCatPaneShowsCaret() throws {
        let one = OnePaneCat(label: "caret")
        let app = try AppE2E.attachWindow(socket: one.socket, session: one.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        XCTAssertTrue(app.waitTerminalContains(one.token))
        app.window?.makeKeyAndOrderFront(nil)
        AppE2E.pump(80)
        if let view = app.window?.firstResponder as? MuxTerminalView {
            app.window?.makeFirstResponder(view)
        }
        AppE2E.pump(80)
        let caret = app.testActiveCaretFrame()
        XCTAssertGreaterThan(caret.width, 1, "attach 后活动 pane 必须有可见 caret。frame=\(caret)")
        XCTAssertGreaterThan(caret.height, 1, "attach 后活动 pane 必须有可见 caret。frame=\(caret)")
    }

    /// 窗口先矮后高：PaneBuf.resize 与 agent DECSTBM 同框。
    /// 若 soft-wrap 不同步，poll panic，SwiftTerm 看不到 FULL_AGENT_FRAME。
    func testDecstbmFrameAfterWindowGrowReachesSwiftTerm() throws {
        let fx = OnePaneCat(label: "decstbm")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))

        app.window?.setFrame(NSRect(x: 40, y: 40, width: 520, height: 280), display: true)
        AppE2E.pump(120)
        app.testPollOnce()
        app.window?.setFrame(NSRect(x: 40, y: 40, width: 1280, height: 860), display: true)
        AppE2E.pump(200)
        for _ in 0..<20 {
            app.testPollOnce()
            AppE2E.pump(20)
        }

        let py = AppE2E.repoRoot.appendingPathComponent("tests/scripts/agent_decstbm_frame.py")
        XCTAssertTrue(FileManager.default.isReadableFile(atPath: py.path), "缺少 \(py.path)")
        Tmux.ok(socket: fx.socket, args: [
            "respawn-pane", "-k", "-t", fx.pane, "python3 -u \(py.path)",
        ])
        Tmux.waitCapture(
            socket: fx.socket,
            target: fx.pane,
            needle: "FULL_AGENT_FRAME",
            timeout: AppE2E.featureTimeout
        )
        XCTAssertTrue(
            app.waitTerminalContains("FULL_AGENT_FRAME", timeout: AppE2E.featureTimeout),
            "DECSTBM 画面必须进 SwiftTerm（poll 不得因 emulate panic 丢事件）。vte=\(app.testActivePaneTerminalText())"
        )
        XCTAssertTrue(
            app.waitTerminalContains("AGENT_TOP", timeout: 3),
            "顶部 AGENT_TOP 必须还在，不能只剩输入行。vte=\(app.testActivePaneTerminalText())"
        )
    }

    func testFirstPaintOfLongHistoryDoesNotReplayOldestLines() {
        AppE2E.ensureApp()
        var raw = Data()
        for i in 0..<200 {
            raw.append(contentsOf: Array("line-\(i)\r\n".utf8))
        }
        let painted = PanePaintPolicy.firstPaint(visible: Data(), raw: raw, rows: 24)
        XCTAssertFalse(
            String(data: painted, encoding: .utf8)?.contains("line-0") ?? true,
            "策略层就必须丢掉最早行，不能把 200 行历史交给 SwiftTerm"
        )
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 400),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        view.getTerminal().resize(cols: 80, rows: 24)
        view.feedOutput(painted, isSnapshot: true)
        AppE2E.pump(40)
        let text = view.visibleScreenText()
        XCTAssertTrue(text.contains("line-199"), "应停在最近缓冲。got=\(text.suffix(80))")
        XCTAssertFalse(text.contains("line-0"), "不得刷出最早输出。got=\(text.prefix(80))")
        window.orderOut(nil)
    }

    /// Surface seed 后历史属于 SwiftTerm 原生 scrollback；用户上划时继续 feed
    /// live，不得通过 reset/RIS 覆盖当前历史位置。
    func testNativeScrollbackKeepsHistoryWhileLiveContinues() {
        AppE2E.ensureApp()
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 640, height: 240))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 240),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        view.getTerminal().resize(cols: 60, rows: 8)
        var seed = Data()
        for i in 0..<24 {
            seed.append(contentsOf: Array("HISTORY_NATIVE_\(i)\r\n".utf8))
        }
        seed.append(contentsOf: Array(String(repeating: "\r\n", count: 8).utf8))
        seed.append(contentsOf: Array("\u{1b}[H\u{1b}[1;1HLIVE_SCREEN\u{1b}[8;1HPROMPT>\u{1b}[8;9H".utf8))
        view.feedOutput(seed, isSnapshot: true)
        AppE2E.pump(40)
        XCTAssertTrue(view.canScroll, "seed 后必须存在 native scrollback")

        view.scrollUp(lines: 6)
        let position = view.scrollPosition
        XCTAssertLessThan(position, 0.999, "上划后 native position 必须离开底部")
        view.feedOutput(Data("LIVE_AFTER_SCROLL\r\n".utf8))
        AppE2E.pump(40)
        XCTAssertLessThan(view.scrollPosition, 0.999, "live feed 不能把用户强制拉回底部")
        XCTAssertTrue(view.visibleScreenText().contains("HISTORY_NATIVE_"), "历史视口必须保持可读")

        view.scrollToLatest()
        XCTAssertGreaterThanOrEqual(view.scrollPosition, 0.999)
        XCTAssertTrue(view.visibleScreenText().contains("LIVE_AFTER_SCROLL"), "回底后必须看到新输出")
        window.orderOut(nil)
    }

    /// 真实 AppKit 事件路径回归：不能只调用 `scrollUp()`，必须由
    /// `NSWindow.sendEvent` 命中 terminal view 后进入 SwiftTerm 的
    /// `scrollWheel(with:)`。
    func testAppKitScrollWheelReachesTerminalView() {
        AppE2E.ensureApp()
        let view = MuxTerminalView(
            paneId: 2,
            frame: NSRect(x: 0, y: 0, width: 640, height: 240)
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 240),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        view.getTerminal().resize(cols: 60, rows: 8)

        var seed = Data()
        for i in 0..<32 {
            seed.append(contentsOf: Array("APPKIT_HISTORY_\(i)\r\n".utf8))
        }
        seed.append(contentsOf: Array(String(repeating: "\r\n", count: 8).utf8))
        seed.append(contentsOf: Array("\u{1b}[H\u{1b}[1;1HAPPKIT_LIVE\u{1b}[8;1HPROMPT>".utf8))
        view.feedOutput(seed, isSnapshot: true)
        AppE2E.pump(40)
        XCTAssertTrue(view.canScroll, "seed 后必须有 native scrollback")

        let localPoint = NSPoint(x: view.bounds.midX, y: view.bounds.midY)
        let screenPoint = view.convert(localPoint, to: nil)
        guard let cgEvent = CGEvent(
            scrollWheelEvent2Source: CGEventSource(stateID: .hidSystemState),
            units: .line,
            wheelCount: 1,
            wheel1: 6,
            wheel2: 0,
            wheel3: 0
        ) else {
            XCTFail("无法构造 AppKit scroll wheel CGEvent")
            window.orderOut(nil)
            return
        }
        cgEvent.setIntegerValueField(
            .mouseEventWindowUnderMousePointer,
            value: Int64(window.windowNumber)
        )
        cgEvent.setIntegerValueField(
            .mouseEventWindowUnderMousePointerThatCanHandleThisEvent,
            value: Int64(window.windowNumber)
        )
        let displayMaxY = NSScreen.screens
            .first(where: { $0.frame.contains(screenPoint) })?
            .frame
            .maxY ?? 0
        // CGEvent uses a top-left origin while AppKit screen points use a
        // bottom-left origin.
        cgEvent.location = CGPoint(x: screenPoint.x, y: displayMaxY - screenPoint.y)
        guard let event = NSEvent(cgEvent: cgEvent) else {
            XCTFail("无法把 CGEvent 转成 NSEvent")
            window.orderOut(nil)
            return
        }
        window.sendEvent(event)
        AppE2E.pump(40)

        XCTAssertLessThan(
            view.scrollPosition,
            0.999,
            "真实 AppKit 滚轮上划后必须离开 native scrollback 底部"
        )
        window.orderOut(nil)
    }

    func testLightThemeOscReportsTrueBlackNotGray() {
        let osc = ColorContrast.oscColors(fg: MuxtermPalette.light.fg, bg: MuxtermPalette.light.bg)
        XCTAssertEqual(osc.bg.lowercased(), MuxtermPalette.light.bg)
        XCTAssertEqual(
            osc.fg.lowercased(),
            MuxtermPalette.light.fg,
            "OSC 10 必须是主题黑字，不能报 595959 污染普通 tmux attach。got=\(osc.fg)"
        )
    }

    func testBlackOnBlackCellsAreDrawnReadable() {
        AppE2E.ensureApp()
        MuxtermTerminalColors.activePalette = .light
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 640, height: 360))
        view.applyPalette(.light)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 360),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        AppE2E.pump(40)
        view.getTerminal().resize(cols: 40, rows: 12)
        // 黑底 + 显式黑字：OSC 10 改不了这种格子，必须在绘制时抬亮。
        let line = "\u{1b}[40m\u{1b}[30m" + String(repeating: "X", count: 40) + "\u{1b}[0m\r\n"
        view.feedOutput(Data(line.utf8))
        view.forceRedraw()
        AppE2E.pump(80)
        guard let range = view.sampleFirstRowLuminanceRange() else {
            XCTFail("无法采样终端像素")
            window.orderOut(nil)
            return
        }
        XCTAssertGreaterThan(
            range.max,
            80 * 3,
            "黑底黑字必须被抬亮，否则 Cursor 输入框看不见。range=\(range)"
        )
        XCTAssertLessThan(
            range.min,
            80,
            "黑底本身应仍接近黑。range=\(range)"
        )
        window.orderOut(nil)
    }

    func testDeleteToBeginningOfLineSendsCtrlU() {
        AppE2E.ensureApp()
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 320, height: 180))
        let handler = RecordingInputHandler()
        view.inputHandler = handler
        view.doCommand(by: Selector(("deleteToBeginningOfLine:")))
        XCTAssertEqual(handler.bytes, [0x15], "Ctrl-U 必须发给 pane，不能 Unhandle selector")
        view.doCommand(by: Selector(("noop:")))
        XCTAssertEqual(handler.bytes, [0x15], "noop 必须静默忽略")
    }
}

private final class RecordingInputHandler: TerminalInputHandler {
    var bytes: [UInt8] = []
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        bytes.append(contentsOf: data)
    }
    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {}
}

private func luminance(_ hex: String) -> Int {
    let value = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
    guard value.count == 6, let rgb = UInt32(value, radix: 16) else { return 0 }
    let r = Int((rgb >> 16) & 0xff)
    let g = Int((rgb >> 8) & 0xff)
    let b = Int(rgb & 0xff)
    return r + g + b
}
