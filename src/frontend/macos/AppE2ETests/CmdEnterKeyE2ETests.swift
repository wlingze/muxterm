import AppKit
import XCTest
@testable import MuxtermAppLib

/// 用户路径：Cmd-Enter 切换当前 pane 全屏。不要只测 KeyChord 表，要走 handleKey。
final class CmdEnterKeyE2ETests: XCTestCase {
    func testImePlaceholderNeverReachesRemoteShell() throws {
        AppE2E.ensureApp()
        for kitty in [false, true] {
            let terminal = MuxTerminalView(
                paneId: 603, frame: NSRect(x: 0, y: 0, width: 800, height: 400)
            )
            let recorder = KeyInputRecorder()
            terminal.inputHandler = recorder
            if kitty { terminal.feedOutput(Data("\u{1b}[>1u".utf8)) }
            for characters in ["", "\u{ffff}"] {
                let event = try XCTUnwrap(NSEvent.keyEvent(
                    with: .keyDown, location: .zero, modifierFlags: [],
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0,
                    context: nil, characters: characters,
                    charactersIgnoringModifiers: "\u{ffff}", isARepeat: false, keyCode: 0
                ))
                terminal.keyDown(with: event)
            }
            XCTAssertTrue(recorder.bytes.isEmpty,
                          "IME placeholder must never become raw bytes; kitty=\(kitty), bytes=\(recorder.bytes)")
            terminal.insertText("中文", replacementRange: NSRange(location: NSNotFound, length: 0))
            XCTAssertEqual(recorder.bytes, Array("中文".utf8))
            recorder.bytes = []
            terminal.insertText("\u{ffff}", replacementRange: NSRange(location: NSNotFound, length: 0))
            XCTAssertTrue(recorder.bytes.isEmpty, "IME insertText placeholder must not reach the pane")
            terminal.insertText("中\u{ffff}文", replacementRange: NSRange(location: NSNotFound, length: 0))
            XCTAssertEqual(recorder.bytes, Array("中文".utf8), "IME commit must retain valid Chinese text")
        }
    }

    func testPlainArrowsReachPagerInNormalAndApplicationCursorModes() throws {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(paneId: 602, frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        let recorder = KeyInputRecorder()
        terminal.inputHandler = recorder
        terminal.feedOutput(Data("\u{1b}[?1049h".utf8))
        for (mode, prefix) in [("\u{1b}[?1l", "\u{1b}["), ("\u{1b}[?1h", "\u{1b}O")] {
            terminal.feedOutput(Data(mode.utf8))
            recorder.bytes = []
            for (code, character) in [(UInt16(126), "\u{F700}"), (UInt16(125), "\u{F701}")] {
                let event = try XCTUnwrap(NSEvent.keyEvent(
                    with: .keyDown, location: .zero, modifierFlags: .function,
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0,
                    context: nil, characters: character, charactersIgnoringModifiers: character,
                    isARepeat: false, keyCode: code
                ))
                terminal.keyDown(with: event)
            }
            XCTAssertEqual(recorder.bytes, Array("\(prefix)A\(prefix)B".utf8),
                           "pager arrows must reach the remote PTY in both cursor modes")
        }
    }

    func testPageKeysReachAlternateScreenApplication() throws {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(paneId: 600, frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        let recorder = KeyInputRecorder()
        terminal.inputHandler = recorder
        terminal.feedOutput(Data("\u{1b}[?1049h\u{1b}[?1l".utf8))
        XCTAssertTrue(terminal.getTerminal().isCurrentBufferAlternate)
        XCTAssertFalse(terminal.getTerminal().applicationCursor)
        for (code, character) in [(UInt16(116), "\u{F72C}"), (UInt16(121), "\u{F72D}")] {
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: .function,
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0,
                context: nil, characters: character, charactersIgnoringModifiers: character,
                isARepeat: false, keyCode: code
            ))
            terminal.keyDown(with: event)
        }
        XCTAssertEqual(recorder.bytes, Array("\u{1b}[5~\u{1b}[6~".utf8))
    }

    func testChineseImeCommitSendsLiteralUtf8InKittyMode() throws {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(paneId: 601, frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        let recorder = KeyInputRecorder()
        terminal.inputHandler = recorder
        terminal.feedOutput(Data("\u{1b}[>1u".utf8))
        let compositionKey = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0,
            context: nil, characters: "", charactersIgnoringModifiers: "\u{ffff}",
            isARepeat: false, keyCode: 0
        ))
        terminal.keyDown(with: compositionKey)
        terminal.setMarkedText("中", selectedRange: NSRange(location: 1, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0))
        terminal.insertText("中", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertEqual(recorder.bytes, Array("中".utf8))
        XCTAssertFalse(terminal.hasMarkedText())
    }

    func testCloseMenuTargetsKeySettingsWindowInsteadOfMainPane() throws {
        let fixture = TwoPaneCat(label: "close-settings")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))
        let settings = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
            styleMask: [.titled, .closable], backing: .buffered, defer: false
        )
        settings.isReleasedWhenClosed = false
        defer { settings.close() }
        settings.makeKeyAndOrderFront(nil)
        AppE2E.pump(30)
        // XCTest 后台进程没有 key window，显式提供生产入口读取的那个窗口。
        MuxtermCloseRouting.close(window: settings)
        AppE2E.pump(30)
        XCTAssertFalse(settings.isVisible)
        XCTAssertTrue(app.window?.isVisible == true)
        XCTAssertEqual(app.testLayoutLeafIDs().count, 2)

        app.testOpenAttentionPanel()
        MuxtermCloseRouting.close(window: app.unifiedPanel.window)
        XCTAssertFalse(app.testAttentionPanelOpen())
        XCTAssertEqual(app.testLayoutLeafIDs().count, 2)
    }

    func testCmdWClosesPanelBeforePaneAndOnlyOnePaneAtATime() throws {
        let fixture = TwoPaneCat(label: "close-layer")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))
        app.testOpenAttentionPanel()
        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: .command,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: try XCTUnwrap(app.window).windowNumber, context: nil,
            characters: "w", charactersIgnoringModifiers: "w", isARepeat: false, keyCode: 13
        ))
        XCTAssertTrue(app.testDispatchKeyEvent(event))
        XCTAssertFalse(app.testAttentionPanelOpen())
        XCTAssertEqual(app.testLayoutLeafIDs().count, 2)
        XCTAssertTrue(app.testDispatchKeyEvent(event))
        XCTAssertTrue(AppE2E.wait(timeout: 5) {
            app.testPollOnce()
            return app.testLayoutLeafIDs().count == 1
        })
        XCTAssertTrue(app.window?.isVisible == true)
    }

    func testShiftArrowsReachTerminalExactlyOnceInLegacyAndKittyModes() throws {
        let fixture = OnePaneCat(label: "shift-arrows")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        let recorder = KeyInputRecorder()
        let terminal = app.testActiveTerminalView()
        terminal.inputHandler = recorder
        app.window?.makeKeyAndOrderFront(nil)
        app.testMakeActiveTerminalFirstResponder()
        let arrows: [(UInt16, String, String)] = [
            (123, "\u{f702}", "D"), (124, "\u{f703}", "C"),
            (125, "\u{f701}", "B"), (126, "\u{f700}", "A"),
        ]
        for kitty in [false, true] {
            if kitty { terminal.feed(byteArray: Array("\u{1b}[>1u".utf8)[...]) }
            let modifiers: [(NSEvent.ModifierFlags, Int)] = kitty ? [(.shift, 2)] : [
                (.shift, 2), (.option, 3), ([.shift, .option], 4),
                (.control, 5), ([.shift, .control], 6),
                ([.option, .control], 7), ([.shift, .option, .control], 8),
            ]
            for (flags, parameter) in modifiers {
                for (code, character, suffix) in arrows {
                    recorder.bytes = []
                    let event = try XCTUnwrap(NSEvent.keyEvent(
                        with: .keyDown, location: .zero,
                        modifierFlags: flags.union([.function, .numericPad]),
                        timestamp: ProcessInfo.processInfo.systemUptime,
                        windowNumber: try XCTUnwrap(app.window).windowNumber, context: nil,
                        characters: character, charactersIgnoringModifiers: character,
                        isARepeat: false, keyCode: code
                    ))
                    XCTAssertTrue(app.testRouteMonitoredKeyEvent(event) === event,
                        "非应用快捷键必须交回终端 responder")
                    NSApp.sendEvent(event)
                    AppE2E.pump(20)
                    XCTAssertEqual(recorder.bytes, Array("\u{1b}[1;\(parameter)\(suffix)".utf8),
                        "arrow \(code), modifier=\(parameter), kitty=\(kitty) must not be swallowed or duplicated")
                }
            }
        }
    }

    func testPlainTextImeCommitAndEnterUseResponderChainExactlyOnce() throws {
        let fixture = OnePaneCat(label: "key-responder")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 1), "输入测试前 terminal 必须 ready")

        let recorder = KeyInputRecorder()
        let terminal = app.testActiveTerminalView()
        terminal.inputHandler = recorder
        app.window?.makeKeyAndOrderFront(nil)
        app.testMakeActiveTerminalFirstResponder()
        XCTAssertTrue(app.testFirstResponderIsTerminalView())

        let samples: [(String, UInt16)] = [
            ("a", 0),
            ("中", 0),
            ("\r", 36),
        ]
        for (characters, keyCode) in samples {
            let event = try XCTUnwrap(NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: try XCTUnwrap(app.window).windowNumber,
                context: nil,
                characters: characters,
                charactersIgnoringModifiers: characters,
                isARepeat: false,
                keyCode: keyCode
            ))
            XCTAssertTrue(
                app.testRouteMonitoredKeyEvent(event) === event,
                "普通文字/IME/Enter 必须返回给 AppKit responder chain"
            )
            NSApp.sendEvent(event)
            AppE2E.pump(30)
        }

        XCTAssertEqual(
            recorder.bytes,
            Array("a中\r".utf8),
            "英文、中文提交和 Enter 都必须且只能发送一次"
        )
    }

    func testCmdEnterKeyEventZoomsTmuxAndGuiLeaf() throws {
        let painted = PaintedWorkspace(label: "cmd-enter")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minTabs: 2, minLeaves: 3), "zoom 前应有 3 leaf")

        app.window?.makeKeyAndOrderFront(nil)
        AppE2E.pump(40)
        let event = try XCTUnwrap(app.testMakeCmdEnterEvent(), "必须能构造 Cmd-Enter")
        XCTAssertTrue(app.testDispatchKeyEvent(event), "handleKey 必须消费 Cmd-Enter")

        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(30)
                return Tmux.out(
                    socket: painted.socket,
                    args: ["display-message", "-p", "-t", painted.session, "#{window_zoomed_flag}"]
                ) == "1"
            },
            "Cmd-Enter 后 tmux window_zoomed_flag 应为 1"
        )
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                app.testFlushFeeds()
                return app.testLayoutLeafIDs().count == 1
            },
            "Cmd-Enter 后 GUI 必须单 leaf。leaves=\(app.testLayoutLeafIDs())"
        )
    }

    func testCmdEnterTogglesTwoPaneDirectPtyLayout() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge, debug: true)
        defer { app.testShutdown() }
        app.showWindow(nil)
        XCTAssertTrue(app.waitReady(minLeaves: 1))
        app.testSplitHorizontal()
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return app.testLayoutLeafIDs().count == 2
        })
        let event = try XCTUnwrap(app.testMakeCmdEnterEvent())
        XCTAssertTrue(app.testDispatchKeyEvent(event))
        XCTAssertEqual(app.testLayoutLeafIDs().count, 1)
        app.testPollOnce()
        XCTAssertEqual(app.testLayoutLeafIDs().count, 1)
        XCTAssertTrue(app.testDispatchKeyEvent(event))
        XCTAssertTrue(AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            return app.testLayoutLeafIDs().count == 2
        })
    }
}

private final class KeyInputRecorder: TerminalInputHandler {
    var bytes: [UInt8] = []

    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        bytes.append(contentsOf: data)
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {}
}
