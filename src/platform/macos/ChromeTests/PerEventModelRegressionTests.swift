import XCTest
import SwiftTerm

/// 回归：cursor agent 的「擦除 + 上移 + 原地重绘」帧，按后端事件边界
/// 逐段 feed 时，SwiftTerm 模型必须原地覆盖、不堆叠。
///
/// 这固化「模型层正确」的契约：macOS app 的事件直喂路径（以及任何未来
/// 的重构）都不能破坏它；若再出现用户可见的逐帧堆叠，问题在视图渲染层
/// 而不是字节流/模型。
final class PerEventModelRegressionTests: XCTestCase {
    private final class SilentDelegate: TerminalDelegate {
        func send(source: Terminal, data: ArraySlice<UInt8>) {}
        func sizeChanged(source: Terminal, newCols: Int, newRows: Int) {}
    }

    private func inputRowCount(_ terminal: Terminal) -> Int {
        var count = 0
        for y in 0..<terminal.rows {
            var line = ""
            for x in 0..<terminal.cols {
                line.append(terminal.getCharacter(col: x, row: y) ?? " ")
            }
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.contains("→") || trimmed.contains("BOX") {
                count += 1
            }
        }
        return count
    }

    /// 模拟 agent 输入：每敲一个字符，发一帧「向上擦除 9 行 + 原地重绘」。
    /// 每帧作为独立事件 feed（与后端 %output 事件边界一致）。
    func testEraseAndRedrawFramesDoNotStack() {
        let terminal = Terminal(delegate: SilentDelegate())
        terminal.resize(cols: 80, rows: 24)
        let erase9 =
            "\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A\u{1b}[G"
        let inputs = ["h", "ha", "hao", "hao ", "hao 好"]
        for input in inputs {
            let frame = "\(erase9)→ \(input)\r\nTIP\r\n\r\nBOX\r\n\r\nFOOTER\r\n"
            terminal.feed(byteArray: Array(frame.utf8))
            // 每帧之后，屏幕上只能有一行 STATUS 内容（原地覆盖，不堆叠）
            XCTAssertEqual(
                inputRowCount(terminal),
                2,
                "输入帧 \(input) 后不得堆叠"
            )
        }
        // 最后一帧内容可见，历史帧不得残留
        var last = ""
        for x in 0..<terminal.cols {
            for y in 0..<terminal.rows {
                if terminal.getCharacter(col: x, row: y) == "好" {
                    last += "好"
                }
            }
        }
        XCTAssertTrue(last.contains("好"), "最终输入应显示在屏幕上")
    }
}
