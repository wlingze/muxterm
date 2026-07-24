import AppKit
import SwiftTerm

/// 单个 pane 的 SwiftTerm 终端视图；输入经 delegate 回传到 FFI。
///
/// 不重写 `keyDown`：交给 SwiftTerm → `interpretKeyEvents` → `insertText`（NSTextInputClient）
/// 单路径发送，避免 keyDown 与 insertText 双写。
final class MuxTerminalView: TerminalView {
    /// 对应 muxterm pane id。
    let paneId: UInt32
    weak var inputHandler: TerminalInputHandler?
    /// 供 XCUITest 读取的可见输出片段（与 feed 同步）。
    private(set) var accessibilityOutput: String = ""

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
        setAccessibilityLabel("Terminal Pane \(paneId)")
        setAccessibilityValue("")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// 将 FFI 输出喂给终端引擎，并更新 AX 值供 UITest 断言「确实渲染到了」。
    func feedOutput(_ data: Data) {
        guard !data.isEmpty else { return }
        let bytes = [UInt8](data)
        feed(byteArray: bytes[...])
        if let text = String(data: data, encoding: .utf8), !text.isEmpty {
            accessibilityOutput += text
            if accessibilityOutput.count > 800 {
                accessibilityOutput = String(accessibilityOutput.suffix(800))
            }
            setAccessibilityValue(accessibilityOutput)
        }
        needsDisplay = true
    }

    /// 布局完成后：按当前像素尺寸驱动 SwiftTerm 行列，并通知 PTY resize。
    /// 返回是否成功同步到合法行列（≥2×1）。
    @discardableResult
    func syncSizeToPty() -> Bool {
        layoutSubtreeIfNeeded()
        let size = bounds.size
        guard size.width >= 40, size.height >= 24 else { return false }

        // 走 SwiftTerm setFrameSize → processSizeChange（会 callback sizeChanged）
        setFrameSize(NSSize(width: size.width + 0.5, height: size.height))
        setFrameSize(size)

        let term = getTerminal()
        guard term.cols >= 2, term.rows >= 1 else { return false }
        inputHandler?.terminal(self, sizeChanged: term.cols, rows: term.rows)
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
