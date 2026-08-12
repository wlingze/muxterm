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
    /// 本次 feed 字节里是否包含终端查询（OSC 10/11/12、CSI DA/DSR、kitty）。
    /// tmux 镜像只放行「确实有查询」的应答，避免 git lg 等泄漏成 shell 字面命令。
    private var feedContainsQuery = false
    /// 供 XCUITest 读取的可见输出片段（与 feed 同步）。
    private(set) var accessibilityOutput: String = ""
    /// AX 屏幕文本的刷新节流：全屏逐格读取有开销，无需每个 chunk 都更新。
    private var lastAccessibilityUpdate = Date.distantPast
    private static let accessibilityUpdateInterval: TimeInterval = 1.0
    /// 上一次按像素驱动后的模型行列，用于检测窗口 resize 这一瞬间。
    private var lastModelCols = 0
    private var lastModelRows = 0

    init(paneId: UInt32, frame: NSRect = .zero) {
        self.paneId = paneId
        super.init(frame: frame)
        terminalDelegate = self
        wantsLayer = true
        nativeForegroundColor = NSColor.textColor
        nativeBackgroundColor = NSColor.textBackgroundColor
        // 关闭 SwiftTerm 的 mouse reporting 转发，保证鼠标点击/拖拽优先做文本
        // 选择（选中复制）。codex/htop 等应用启用 mouse 协议后，SwiftTerm 默认
        // 会把点击/拖拽当 mouse 序列发给程序，导致「选不中、一直闪烁」。需要
        // 向应用发送鼠标事件时仍可用 Shift+拖拽（SwiftTerm 的
        // shiftBypassesMouseReporting 兜底）。
        allowMouseReporting = false
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
    func feedOutput(_ data: Data, isSnapshot: Bool = false) {
        guard !data.isEmpty else { return }
        let bytes = [UInt8](data)
        // SwiftTerm 同步解析输出：查询应答经 `Terminal.sendResponse` 在 feed
        // 调用栈内同步发出。用这个标记把「解析 pane 输出产生的应答」与
        // 「用户输入 / 鼠标上报」区分开；tmux 镜像只放行本次确有查询的应答，
        // 其余丢弃（否则 codex 收不到颜色/能力查询应答会退化成黑底黑字）。
        isFeedingRemoteOutput = true
        feedContainsQuery = !isSnapshot && TerminalQueryDetector.containsQuery(in: bytes)
        feed(byteArray: bytes[...])
        isFeedingRemoteOutput = false
        feedContainsQuery = false
        // AX 反映「当前屏幕」而不是 feed 历史：之前累积所有 feed 文本，
        // 输入/状态区的每一帧中间状态都会留在 AX 值里，看起来像逐帧堆叠。
        let now = Date()
        if now.timeIntervalSince(lastAccessibilityUpdate) >= Self.accessibilityUpdateInterval {
            lastAccessibilityUpdate = now
            updateAccessibilityOutput()
        }
        // 只做普通标记：SwiftTerm 会按自己的 refresh 范围节流绘制。
        // 不要在每次 feed 后强制全屏重绘或同步 displayIfNeeded——agent 在
        // resize 后会发出一连串纯擦除序列（每事件 1KB），逐事件全屏重绘
        // 会占满主线程（tab 快捷键失效、IMK mach port 报错）。
        needsDisplay = true
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
        // 窗口 resize 时 SwiftTerm 缩小模型会保留旧屏幕 + 光标位置，agent/htop
        // 会按新尺寸重绘，旧行残留在屏幕上造成堆叠/下半空白。这里只在模型行列
        // 真正变化（resize 瞬间）做一次全屏重绘清掉残留；不逐 feed、不强制同步
        // displayIfNeeded，避免高频输出时刷屏。
        if term.cols != lastModelCols || term.rows != lastModelRows {
            lastModelCols = term.cols
            lastModelRows = term.rows
            getTerminal().updateFullScreen()
            setNeedsDisplay(bounds)
            if let layer { layer.setNeedsDisplay() }
        }
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
    }

    /// 覆写 SwiftTerm 模拟器输出通道：只有 `Terminal.sendResponse` /
    /// `sendFocusReport` 等模拟器生成的事件走这里（`source: Terminal`）；
    /// 键盘/粘贴/kitty 用户输入走 `TerminalViewDelegate.send(source: TerminalView)`
    /// 的 `send(data:)` 路径，不受影响。
    override func send(source: Terminal, data: ArraySlice<UInt8>) {
        guard TerminalMirrorPolicy.shouldForwardParserResponse(
            duringRemoteOutputFeed: isFeedingRemoteOutput,
            isTmuxMirror: suppressOutputDrivenResponses,
            feedContainsQuery: feedContainsQuery
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
