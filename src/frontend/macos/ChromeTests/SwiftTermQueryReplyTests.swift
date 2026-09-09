import XCTest
import SwiftTerm

/// 记录 SwiftTerm 经 `send(source:data:)` 发出的所有字节。
///
/// 这正是 `MuxTerminalView.terminalDelegate?.send` 的路径：应用收到这些
/// 应答字节后经 `CoreBridge.sendInput` 写回 tmux/shell，`git lg` 的
/// OSC 10/11 与 CSI DA 才不会泄漏成字面文本。
private final class RecordingTerminalDelegate: TerminalDelegate {
    var sent: [UInt8] = []

    func send(source: Terminal, data: ArraySlice<UInt8>) {
        sent.append(contentsOf: data)
    }
}

final class SwiftTermQueryReplyTests: XCTestCase {
    private func terminal(
        foreground: (UInt16, UInt16, UInt16) = (0, 0, 0),
        background: (UInt16, UInt16, UInt16) = (65_535, 65_535, 65_535)
    ) -> (Terminal, RecordingTerminalDelegate) {
        let delegate = RecordingTerminalDelegate()
        let terminal = Terminal(delegate: delegate)
        terminal.foregroundColor = Color(red: foreground.0, green: foreground.1, blue: foreground.2)
        terminal.backgroundColor = Color(red: background.0, green: background.1, blue: background.2)
        return (terminal, delegate)
    }

    /// OSC 10 前景色查询：SwiftTerm 应回 `ESC ] 10 ; rgb:RRRR/GGGG/BBBB ESC \`，
    /// 且经 `send` 委托原样到达宿主。
    func testOsc10ForegroundQueryReply() {
        let (terminal, delegate) = terminal()
        terminal.feed(byteArray: Array("\u{1b}]10;?\u{1b}\\".utf8))

        let reply = String(bytes: delegate.sent, encoding: .utf8)
        XCTAssertEqual(reply, "\u{1b}]10;rgb:0000/0000/0000\u{1b}\\")
    }

    /// OSC 11 背景色查询：默认白色背景。
    func testOsc11BackgroundQueryReply() {
        let (terminal, delegate) = terminal()
        terminal.feed(byteArray: Array("\u{1b}]11;?\u{07}".utf8))

        let reply = String(bytes: delegate.sent, encoding: .utf8)
        XCTAssertEqual(reply, "\u{1b}]11;rgb:ffff/ffff/ffff\u{1b}\\")
    }

    /// OSC 12 光标色查询：未设置光标色时回退到前景色。
    func testOsc12CursorQueryReplyFallsBackToForeground() {
        let (terminal, delegate) = terminal(foreground: (0x1234, 0x3456, 0x5678))
        terminal.feed(byteArray: Array("\u{1b}]12;?\u{1b}\\".utf8))

        let reply = String(bytes: delegate.sent, encoding: .utf8)
        XCTAssertEqual(reply, "\u{1b}]12;rgb:1234/3456/5678\u{1b}\\")
    }

    /// CSI Primary DA 查询：SwiftTerm 应回 `ESC [ ? 65 ; 4 ; 1 ; 2 ; 6 ; 21 ; 22 ; 17 ; 28 c`
    /// （与 MuxtermChrome 的 TerminalQueryReply.csiDeviceAttributes 一致）。
    func testPrimaryDeviceAttributesReply() {
        let (terminal, delegate) = terminal()
        terminal.feed(byteArray: Array("\u{1b}[c".utf8))

        let reply = String(bytes: delegate.sent, encoding: .utf8)
        XCTAssertEqual(reply, "\u{1b}[?65;4;1;2;6;21;22;17;28c")
    }

    /// 连续两个查询的回复按顺序出现在 send 通道，且字节原样保留。
    func testMultipleQueryRepliesPreserveOrderAndBytes() {
        let (terminal, delegate) = terminal()
        terminal.feed(byteArray: Array("\u{1b}]10;?\u{1b}\\\u{1b}[c".utf8))

        let reply = String(bytes: delegate.sent, encoding: .utf8)
        XCTAssertEqual(
            reply,
            "\u{1b}]10;rgb:0000/0000/0000\u{1b}\\\u{1b}[?65;4;1;2;6;21;22;17;28c"
        )
    }
}
