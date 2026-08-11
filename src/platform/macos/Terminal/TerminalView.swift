import AppKit
import MuxtermChrome
import SwiftTerm

/// 单个 pane 的 SwiftTerm 终端视图；输入经 delegate 回传到 FFI。
///
/// 不重写 `keyDown`：交给 SwiftTerm → `interpretKeyEvents` → `insertText`（NSTextInputClient）
/// 单路径发送，避免 keyDown 与 insertText 双写。
final class MuxTerminalView: TerminalView {
    /// 对应 muxterm pane id。
    let paneId: UInt32
    weak var inputHandler: TerminalInputHandler?
    /// tmux 控制模式下，SwiftTerm 解析 pane 输出时生成的查询应答（OSC 10/11、
    /// CSI DA/DSR、DCS 等）必须丢弃：tmux 拥有 pane 的 PTY 与终端协议，应答
    /// 经 `send-keys -l` 回写会被 pane 回显并执行，造成 `git lg` 的
    /// `10;rgb:...` / `65;...c` 泄漏成 shell 字面命令。
    ///
    /// 本地 / daemon 模式下保持转发（前端就是该 PTY 的终端模拟器，写回 pty
    /// 是正确行为）。
    var suppressOutputDrivenResponses = false
    /// 正在 feed 远端 pane 输出（解析器应答只在这个窗口内产生）。
    private var isFeedingRemoteOutput = false
    /// 供 XCUITest 读取的可见输出片段（与 feed 同步）。
    private(set) var accessibilityOutput: String = ""
    /// AX 屏幕文本的刷新节流：全屏逐格读取有开销，无需每个 chunk 都更新。
    private var lastAccessibilityUpdate = Date.distantPast
    private static let accessibilityUpdateInterval: TimeInterval = 0.15

    init(paneId: UInt32, frame: NSRect = .zero) {
        self.paneId = paneId
        super.init(frame: frame)
        terminalDelegate = self
        wantsLayer = true
        nativeForegroundColor = NSColor.textColor
        nativeBackgroundColor = NSColor.textBackgroundColor
        setAccessibilityIdentifier("muxterm.terminal.\(paneId)")
        setAccessibilityElement(true)
        setAccessibilityRole(.textArea)
        setAccessibilityLabel(
            MuxtermI18n.shared.tr(.terminalPane, arguments: ["id": "\(paneId)"])
        )
        setAccessibilityValue("")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    /// 将 FFI 输出喂给终端引擎，并更新 AX 值供 UITest 断言「确实渲染到了」。
    func feedOutput(_ data: Data) {
        guard !data.isEmpty else { return }
        let bytes = [UInt8](data)
        // SwiftTerm 同步解析输出：查询应答经 `Terminal.sendResponse` 在 feed
        // 调用栈内同步发出。用这个标记把「解析 pane 输出产生的应答」与
        // 「用户输入 / 鼠标上报」区分开，tmux 镜像只丢弃前者。
        isFeedingRemoteOutput = true
        feed(byteArray: bytes[...])
        isFeedingRemoteOutput = false
        // AX 反映「当前屏幕」而不是 feed 历史：之前累积所有 feed 文本，
        // 输入/状态区的每一帧中间状态都会留在 AX 值里，看起来像逐帧堆叠。
        let now = Date()
        if now.timeIntervalSince(lastAccessibilityUpdate) >= Self.accessibilityUpdateInterval {
            lastAccessibilityUpdate = now
            updateAccessibilityOutput()
        }
        // SwiftTerm 默认按 Terminal.refreshStart/End 局部重绘；cursor agent
        // 这类「擦除 + 上移 + 原地重绘」的局部更新可能不在局部重绘范围内，
        // 旧帧像素残留在屏幕上。把 refresh 范围扩到全屏并标记整视图重绘
        // （不强制同步 displayIfNeeded：高频 feed 时同步重绘会与 SwiftTerm
        // 自身的 16.7ms 绘制节流竞争，导致 htop 全屏重写时闪烁/错乱）。
        getTerminal().updateFullScreen()
        setNeedsDisplay(bounds)
    }

    private func updateAccessibilityOutput() {
        let term = getTerminal()
        let dims = term.getDims()
        let lines = ScreenText.lines(
            cols: dims.cols,
            rows: dims.rows,
            characterAt: { term.getCharacter(col: $0, row: $1) ?? " " }
        )
        accessibilityOutput = lines.joined(separator: "\n")
        setAccessibilityValue(accessibilityOutput)
    }

    /// 当前 SwiftTerm 字符格的 backing pixel 尺寸。
    func terminalCellSizeInPixels() -> (width: Int, height: Int)? {
        cellSizeInPixels(source: getTerminal())
    }

    /// 布局完成后：按当前像素尺寸驱动 SwiftTerm 行列。
    /// 返回是否成功同步到合法行列（≥2×1）。
    @discardableResult
    func syncSizeToPty(notifyResize: Bool = true) -> Bool {
        layoutSubtreeIfNeeded()
        let size = bounds.size
        guard size.width >= 40, size.height >= 24 else { return false }

        // SwiftTerm setFrameSize 会 processSizeChange；这里只调用一次，避免重复回调。
        if frame.size != size {
            setFrameSize(size)
        }

        let term = getTerminal()
        guard term.cols >= 2, term.rows >= 1 else { return false }
        if notifyResize {
            inputHandler?.terminal(self, sizeChanged: term.cols, rows: term.rows)
        }
        needsDisplay = true
        return true
    }

    func forceRedraw() {
        needsDisplay = true
        // 触达 Metal/CG 显示路径
        if let layer {
            layer.setNeedsDisplay()
        }
        displayIfNeeded()
    }

    /// 覆写 SwiftTerm 模拟器输出通道：只有 `Terminal.sendResponse` /
    /// `sendFocusReport` 等模拟器生成的事件走这里（`source: Terminal`）；
    /// 键盘/粘贴/kitty 用户输入走 `TerminalViewDelegate.send(source: TerminalView)`
    /// 的 `send(data:)` 路径，不受影响。
    override func send(source: Terminal, data: ArraySlice<UInt8>) {
        guard TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: isFeedingRemoteOutput,
            isTmuxMirror: suppressOutputDrivenResponses
        ) else { return }
        super.send(source: source, data: data)
    }
}

/// 键盘/输入回传协议。
protocol TerminalInputHandler: AnyObject {
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>)
    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int)
}

extension MuxTerminalView: TerminalViewDelegate {
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
        guard newCols >= 2, newRows >= 1 else { return }
        inputHandler?.terminal(self, sizeChanged: newCols, rows: newRows)
    }

    func setTerminalTitle(source: TerminalView, title: String) {}

    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        inputHandler?.terminal(self, send: data)
    }

    func scrolled(source: TerminalView, position: Double) {}

    func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
        if let url = URL(string: link) {
            NSWorkspace.shared.open(url)
        }
    }

    func bell(source: TerminalView) {
        NSSound.beep()
    }

    func clipboardCopy(source: TerminalView, content: Data) {
        let pb = NSPasteboard.general
        pb.clearContents()
        if let str = String(data: content, encoding: .utf8) {
            pb.setString(str, forType: .string)
        } else {
            pb.setData(content, forType: .string)
        }
    }

    func clipboardRead(source: TerminalView) -> Data? {
        guard let str = NSPasteboard.general.string(forType: .string) else { return nil }
        return str.data(using: .utf8)
    }

    func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}

    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
}
