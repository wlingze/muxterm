import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 生产日志 `test-2026-0817-1457.log`：attach 后 `refresh-client -r` 上报
/// OSC 10/11 = 黑字白底。cursor/codex 按查询色画深色输入框，再用「默认前景」
/// 画正文和 ▌，结果文字/光标时有时无。
final class AgentRenderE2ETests: XCTestCase {
    func testReportedOscColorsAreLightTextOnDarkBackground() throws {
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
        AppE2E.pump(40)

        let colors = view.themeHexColors()
        let fg = luminance(colors.fg)
        let bg = luminance(colors.bg)
        XCTAssertGreaterThan(
            fg,
            bg,
            "上报给 tmux 的 OSC 10 必须比 OSC 11 亮（agent 深色输入框 + 默认前景）。got fg=\(colors.fg) bg=\(colors.bg)"
        )
        XCTAssertNotEqual(
            colors.fg.lowercased(),
            MuxtermTerminalColors.lightForegroundHex,
            "禁止把浅色主题的 000000 前景报给 tmux 代答 OSC 10"
        )
        XCTAssertNotEqual(
            colors.bg.lowercased(),
            MuxtermTerminalColors.lightBackgroundHex,
            "禁止把浅色主题的 ffffff 背景报给 tmux 代答 OSC 11"
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
}

private func luminance(_ hex: String) -> Int {
    let value = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
    guard value.count == 6, let rgb = UInt32(value, radix: 16) else { return 0 }
    let r = Int((rgb >> 16) & 0xff)
    let g = Int((rgb >> 8) & 0xff)
    let b = Int(rgb & 0xff)
    return r + g + b
}
