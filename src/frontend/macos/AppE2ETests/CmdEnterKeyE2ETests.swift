import AppKit
import XCTest
@testable import MuxtermAppLib

/// 用户路径：Cmd-Enter 切换当前 pane 全屏。不要只测 KeyChord 表，要走 handleKey。
final class CmdEnterKeyE2ETests: XCTestCase {
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
}

private final class KeyInputRecorder: TerminalInputHandler {
    var bytes: [UInt8] = []

    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        bytes.append(contentsOf: data)
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {}
}
