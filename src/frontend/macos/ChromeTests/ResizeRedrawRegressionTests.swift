import AppKit
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

/// SwiftTerm 视图层回归：模型列数必须等于 Muxterm 发给 tmux 的 pane 列数。
///
/// 根因：SwiftTerm 的 `processSizeChange` 用 `getEffectiveWidth` 扣掉滚动条
/// 预留宽度（overlay 下 16pt），而 Muxterm 的 `refresh-client -C` 用容器全宽
/// 计算列数。于是 tmux pane = 87 列、SwiftTerm 模型 = 85 列：codex 的 87 列
/// 帧提前折行，erase-up 行数对不上，输入换行后内容消失（1058 日志复现）。
/// MuxTerminalView 在 init 时隐藏滚动条，让两边都用全宽 → 模型 = pane。
final class SwiftTermGridSyncRegressionTests: XCTestCase {
    private func makeView(width: CGFloat, height: CGFloat) -> TerminalView {
        TerminalView(
            frame: NSRect(x: 0, y: 0, width: width, height: height),
            font: NSFont(name: "Menlo", size: 18)
        )
    }

    /// 与 1058 日志相同的几何：Muxterm 算出的 client 87×29。
    /// 隐藏滚动条后 SwiftTerm 模型必须是 87×29，不允许再少 1–2 列。
    func testHiddenScrollerKeepsModelEqualToTmuxPaneColumns() {
        let view = makeView(width: 957, height: 609)
        view.subviews.first(where: { $0 is NSScroller })?.isHidden = true
        view.setFrameSize(NSSize(width: 957, height: 609))
        let dims = view.getTerminal().getDims()
        XCTAssertEqual(dims.cols, 87, "隐藏滚动条后模型列数必须等于 tmux pane 列数")
        XCTAssertEqual(dims.rows, 29)
    }

    /// 记录 bug 机制：滚动条可见时 SwiftTerm 模型会缩水（这里应为 85 列）。
    /// 该断言同时防止未来改动把滚动条重新打开而不修尺寸计算。
    func testVisibleScrollerShrinksModelColumns() {
        let view = makeView(width: 957, height: 609)
        view.setFrameSize(NSSize(width: 957, height: 609))
        let dims = view.getTerminal().getDims()
        XCTAssertLessThan(dims.cols, 87, "可见滚动条会让模型列数少于 pane 列数")
    }
}
