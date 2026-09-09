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

    /// SwiftTerm 入口接收的是 [UInt8]，而不是先转成 String；把一个包含
    /// htop 常见 CSI/SO/CR 控制序列的 frame 按不同边界拆开，模型仍应完成
    /// 同一帧的定位和重绘。这里断言的是模型状态，不臆测 Metal 绘制结果。
    func testHtopLikeFramePreservesByteFeedAcrossChunkBoundaries() {
        let esc: [UInt8] = [0x1b]
        let prefix = esc + Array("[2J[H".utf8)
        var frame = prefix
        frame += Array("htop".utf8)
        frame.append(0x0f)
        frame += esc
        frame += Array("[2K".utf8)
        frame.append(0x0d)
        frame += esc
        frame += Array("[1A".utf8)
        frame += Array("CPU".utf8)

        for chunkSize in [1, 2, 3, 7, 16] {
            let terminal = Terminal(delegate: SilentDelegate())
            terminal.resize(cols: 40, rows: 8)
            for chunk in stride(from: 0, to: frame.count, by: chunkSize) {
                let end = min(chunk + chunkSize, frame.count)
                terminal.feed(byteArray: Array(frame[chunk..<end]))
            }

            var visible = ""
            for y in 0..<terminal.rows {
                for x in 0..<terminal.cols {
                    visible.append(terminal.getCharacter(col: x, row: y) ?? " ")
                }
            }
            XCTAssertTrue(visible.contains("CPU"), "chunkSize=\(chunkSize)")
            XCTAssertFalse(visible.contains("\u{fffd}"), "chunkSize=\(chunkSize)")
        }
    }

    /// dogfood fixture：Nerd Font 私用区、Powerline、盒线、Unicode/emoji
    /// 和 ASCII 必须在同一个字节流里保留；列数由终端 cell width 决定，
    /// 不能因为字体 fallback 的像素 advance 把 emoji/PUA 当成额外换行。
    func testNerdFontUnicodeAndAsciiFixturePreservesCellColumns() {
        let terminal = Terminal(delegate: SilentDelegate())
        terminal.resize(cols: 80, rows: 8)
        let fixture = "1 Nerd Font \u{f000} \u{e0b0} │╭─╮ ├─ ◆ ✔ ✖\r\n"
            + "2 Unicode ✔ ✖ 📁 ⬢ ╭─╮ ├─ • ⠋ →\r\n"
            + "3 ASCII [ok] [x] > + [D] +-+ |-- * ->\r\n"
        terminal.feed(byteArray: Array(fixture.utf8))

        var visible = ""
        for row in 0..<terminal.rows {
            for col in 0..<terminal.cols {
                visible.append(terminal.getCharacter(col: col, row: row) ?? " ")
            }
        }
        XCTAssertTrue(visible.contains("Nerd Font"))
        XCTAssertTrue(visible.contains("Unicode"))
        XCTAssertTrue(visible.contains("[ok] [x] > + [D]"))
        XCTAssertFalse(visible.contains("\u{fffd}"), "fixture 不能产生 replacement glyph")

        // SwiftTerm 的 cell 模型把宽 emoji 计为两列；后面的 B 应落在
        // 第 4 列（0-based x=4），而不是被字体 fallback 的 advance 推偏。
        terminal.feed(byteArray: Array("\u{1b}[5;1HA📁B".utf8))
        XCTAssertEqual(terminal.getCursorLocation().x, 4)
    }
}
