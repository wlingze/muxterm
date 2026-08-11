import XCTest
import SwiftTerm

/// resize 后重绘堆叠的回归测试。
///
/// 真实路径：agent/htop 类全屏程序在窗口 resize 后按新尺寸用「擦除 + 上移 +
/// 原地重绘」刷新局部区域。SwiftTerm 缩小模型时保留旧屏幕 + 光标，若前端
/// 不处理，旧行残留在屏幕上导致逐帧堆叠。
final class ResizeRedrawRegressionTests: XCTestCase {
    private final class SilentDelegate: TerminalDelegate {
        func send(source: Terminal, data: ArraySlice<UInt8>) {}
        func sizeChanged(source: Terminal, newCols: Int, newRows: Int) {}
    }

    private func gridCount(_ t: Terminal, _ needle: String) -> Int {
        (0..<t.rows).filter { y in
            let line = (0..<t.cols).map { x in t.getCharacter(col: x, row: y) ?? " " }.map(String.init).joined()
            return line.contains(needle)
        }.count
    }

    /// 模拟真实 agent 输入区重绘：旧尺寸喂一帧，resize 后继续喂「向上擦除 +
    /// 原地重绘」的多帧。断言输入行不堆叠（每帧只有一行）。
    func testResizeThenEraseRedrawFramesDoNotStack() {
        let t = Terminal(delegate: SilentDelegate())
        t.resize(cols: 120, rows: 37)
        // 旧尺寸第一帧：两行输入区
        t.feed(byteArray: Array("STATUS-A\r\nTIP\r\n".utf8))

        // resize 到新尺寸（模拟窗口变小后 tmux 重排）
        t.resize(cols: 112, rows: 37)
        // 每帧：向上擦除 2 行 + 原地重绘 2 行（真实 agent 输入区模式）
        for frame in ["B", "C", "D"] {
            let eraseRedraw =
                "\u{1b}[2K\u{1b}[1A\u{1b}[2K\u{1b}[1A" +
                "\u{1b}[GSTATUS-\(frame)\r\nTIP\r\n"
            t.feed(byteArray: Array(eraseRedraw.utf8))
        }

        // 最终只有 STATUS-D 一行，STATUS-A/B/C 都被覆盖
        XCTAssertEqual(gridCount(t, "STATUS-D"), 1, "最终帧应恰好一行")
        XCTAssertEqual(gridCount(t, "STATUS-A"), 0, "旧帧 STATUS-A 不得残留")
        XCTAssertEqual(gridCount(t, "STATUS-B"), 0, "旧帧 STATUS-B 不得残留")
        XCTAssertEqual(gridCount(t, "STATUS-C"), 0, "旧帧 STATUS-C 不得残留")
    }
}
