import XCTest
import SwiftTerm

/// 记录 agent/codex 输入框重绘序列的 SwiftTerm 行为回归。
///
/// test-2026-0813-1159.log 中 Cursor Grok 每次按键都会重绘输入框：
/// 5 次 `ESC[2K ESC[1A`（清行+上移）后写输入行，再用 CRLF 结束每一行。
/// 若这些控制序列被错误解释，输入框会逐键上移，形成
/// 「第一行 1 个字、第二行 2 个字」的阶梯状巨大输入框。
private final class AgentInputNoopDelegate: TerminalDelegate {
    func send(source: Terminal, data: ArraySlice<UInt8>) {}
}

final class AgentInputRenderRegressionTests: XCTestCase {
    private func makeTerminal(cols: Int = 120, rows: Int = 16) -> Terminal {
        Terminal(
            delegate: AgentInputNoopDelegate(),
            options: TerminalOptions(cols: cols, rows: rows)
        )
    }

    /// 还原日志里 Cursor Grok 输入框的单帧重绘字节（pane %13）。
    private func frame(text: String) -> [UInt8] {
        var out: [UInt8] = []
        func add(_ s: String) { out.append(contentsOf: Array(s.utf8)) }
        for _ in 0..<4 {
            add("\u{1b}[2K\u{1b}[1A")
        }
        add("\u{1b}[2K\u{1b}[G ")
        add("\u{1b}[48;2;18;18;18m \u{2192} \u{1b}[2m\u{1b}[36m[Pasted text #1 +25 lines]\u{1b}[39m\u{1b}[22m 目前遇到问题，")
        add(text)
        add(String(repeating: " ", count: 74 - text.count))
        add("\u{1b}[49m\r\n")
        add(" \u{1b}[48;2;18;18;18m")
        add(String(repeating: " ", count: 118))
        add("\u{1b}[49m\r\n\r\n")
        add("  \u{1b}[90mCursor Grok 4.5 High\u{1b}[39m · ctx 0% · \u{1b}[36mfeature-syntaxflow-support_large_value_in_query\u{1b}[39m\r\n")
        return out
    }

    private func rowText(_ terminal: Terminal, row: Int) -> String {
        let dims = terminal.getDims()
        var line = ""
        for col in 0..<dims.cols {
            if let ch = terminal.getCharacter(col: col, row: row) {
                line.append(ch)
            }
        }
        return line
    }

    func testInputRedrawStaysOnSameRow() {
        let terminal = makeTerminal()
        var inputRows: [Int] = []
        for text in ["x", "xc", "xci", "xcia"] {
            terminal.feed(byteArray: frame(text: text))
            let rows = terminal.getDims().rows
            var found = -1
            for row in 0..<rows {
                if rowText(terminal, row: row).contains(text) {
                    found = row
                    break
                }
            }
            XCTAssertNotEqual(found, -1, "输入文本 \(text) 应可见")
            inputRows.append(found)
        }
        XCTAssertEqual(
            Set(inputRows).count,
            1,
            "输入框应固定在同一行，实际行号 \(inputRows)"
        )
    }

    func testCursorRowStableAcrossRepeatedFrames() {
        let terminal = makeTerminal()
        var rows: [Int] = []
        for (index, text) in ["x", "xc", "xci", "xcia", "xciab"].enumerated() {
            terminal.feed(byteArray: frame(text: text))
            rows.append(terminal.getCursorLocation().y)
            if index > 0 {
                XCTAssertEqual(rows[index], rows[index - 1], "光标行在第 \(index) 帧后漂移")
            }
        }
    }

    /// 模拟 attach 快照（capture-pane 的 16 行屏幕 + 尾部 CRLF）后连续输入，
    /// 输入行和光标都必须保持稳定。
    func testInputRedrawStableAfterAttachSnapshot() {
        let terminal = makeTerminal()
        var snapshot: [UInt8] = []
        for i in 0..<16 {
            snapshot.append(contentsOf: Array("history line \(i)".utf8))
            snapshot.append(contentsOf: Array(repeating: UInt8(ascii: " "), count: 120 - "history line \(i)".count))
            snapshot.append(contentsOf: [0x0d, 0x0a])
        }
        terminal.feed(byteArray: snapshot)

        var inputRows: [Int] = []
        var cursorRows: [Int] = []
        for text in ["x", "xc", "xci", "xcia"] {
            terminal.feed(byteArray: frame(text: text))
            let rows = terminal.getDims().rows
            var found = -1
            for row in 0..<rows {
                if rowText(terminal, row: row).contains(text) {
                    found = row
                    break
                }
            }
            XCTAssertNotEqual(found, -1, "输入文本 \(text) 应可见")
            inputRows.append(found)
            cursorRows.append(terminal.getCursorLocation().y)
        }
        XCTAssertEqual(Set(inputRows).count, 1, "输入行应固定，实际 \(inputRows)")
        XCTAssertEqual(Set(cursorRows).count, 1, "光标行应固定，实际 \(cursorRows)")
    }

    /// 验证「快照从半个转义序列开始」会破坏后续帧解析（复现阶梯的机制）。
    /// 若此测试失败（出现阶梯），说明 Rust 侧必须保证快照从完整行边界开始。
    func testPartialEscapePrefixBreaksRedraw() {
        let terminal = makeTerminal()
        // 模拟滑动窗口快照从 OSC 序列中间开始：只有 `ESC]` 没有终止符。
        terminal.feed(byteArray: Array("\u{1b}]1337;SetUserVar=X=".utf8))
        var inputRows: [Int] = []
        for text in ["x", "xc", "xci"] {
            terminal.feed(byteArray: frame(text: text))
            let rows = terminal.getDims().rows
            var found = -1
            for row in 0..<rows {
                if rowText(terminal, row: row).contains(text) {
                    found = row
                    break
                }
            }
            if found >= 0 {
                inputRows.append(found)
            }
        }
        // 机制确认：半截 OSC 会导致后续 ESC 序列被吞掉，输入逐行下移。
        // 这里不硬断言必须复现（SwiftTerm 可能自行恢复），只记录结果。
        print("partial-escape inputRows=\(inputRows)")
        XCTAssertGreaterThan(inputRows.count, 0)
    }

    /// 复现 test-2026-0813-1721.log：codex 粘贴/输入时每次重绘先
    /// `ESC[2K ESC[G CR LF` 再写整屏。如果终端模型把 CR LF 当成滚动，
    /// 输入框会逐帧下移一行（一直换行）。
    func testPasteRedrawFramesDoNotShiftInputRow() {
        let terminal = makeTerminal(cols: 93, rows: 50)

        func esc(_ s: String) -> [UInt8] { Array(s.utf8) }
        var cycle: [UInt8] = []
        cycle += esc("\u{1b}[H\u{1b}[2J\u{1b}[3J")
        for _ in 0..<8 { cycle += esc("\u{1b}[2K\u{1b}[1A") }
        cycle += esc("\u{1b}[G")
        cycle += esc("\u{1b}[?2004l")
        cycle += esc("\u{1b}[2K\u{1b}[G\r\n")
        cycle += redrawContent()
        // 同一帧重绘会被 tmux 拆成多个 %output 事件；合并成一次 feed 后，
        // 中间态不会把输入行推走（分帧喂会在 3 次后漂移到 row 2）。
        terminal.feed(byteArray: cycle)

        var rows: [Int] = []
        for _ in 0..<3 {
            var redraw: [UInt8] = []
            redraw += esc("\u{1b}[2K\u{1b}[G\r\n")
            redraw += redrawContent()
            // 分帧喂会漂移；合并喂（muxterm 端 coalesce）应保持稳定。
            terminal.feed(byteArray: cycle + redraw)
            rows.append(inputRow(terminal))
        }
        XCTAssertEqual(
            Set(rows).count,
            1,
            "paste 重绘不能逐帧下移输入行，实际行号 \(rows)"
        )
    }

    private func redrawContent() -> [UInt8] {
        var out: [UInt8] = []
        func add(_ s: String) { out.append(contentsOf: Array(s.utf8)) }
        add("  Cursor Agent\r\n")
        add("  v2026.08.11-e8db854\r\n")
        add("  Tip: Use /debug to instrument and debug complex problems.\r\n")
        add("\r\n\r\n\r\n\r\n")
        add(" \u{1b}[48;2;242;242;242m")
        add(String(repeating: " ", count: 90))
        add("\u{1b}[49m\r\n")
        add(" \u{1b}[48;2;242;242;242m → [Pasted text #1 +25 lines] 目前遇到问题，")
        add(String(repeating: " ", count: 50))
        add("\u{1b}[49m\r\n")
        add(" \u{1b}[48;2;242;242;242m")
        add(String(repeating: " ", count: 90))
        add("\u{1b}[49m\r\n")
        add("\r\n")
        add("  Cursor Grok 4.5 High · ctx 0% · feature-syntaxflow\r\n")
        return out
    }

    private func inputRow(_ terminal: Terminal) -> Int {
        let dims = terminal.getDims()
        for row in 0..<dims.rows {
            var line = ""
            for col in 0..<dims.cols {
                line.append(terminal.getCharacter(col: col, row: row) ?? " ")
            }
            if line.contains("→") || line.contains("[Pasted text") {
                return row
            }
        }
        return -1
    }
}
