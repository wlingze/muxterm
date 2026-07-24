import AppKit
import SwiftTerm

/// 单个 pane 的 SwiftTerm 终端视图；输入经 delegate 回传到 FFI。
final class MuxTerminalView: TerminalView {
    /// 对应 muxterm pane id。
    let paneId: UInt32
    weak var inputHandler: TerminalInputHandler?

    init(paneId: UInt32, frame: NSRect = .zero) {
        self.paneId = paneId
        super.init(frame: frame)
        terminalDelegate = self
        wantsLayer = true
        nativeForegroundColor = NSColor.textColor
        nativeBackgroundColor = NSColor.textBackgroundColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// 将 FFI 输出喂给终端引擎。
    func feedOutput(_ data: Data) {
        guard !data.isEmpty else { return }
        let bytes = [UInt8](data)
        feed(byteArray: bytes[...])
    }
}

/// 键盘/输入回传协议。
protocol TerminalInputHandler: AnyObject {
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>)
    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int)
}

extension MuxTerminalView: TerminalViewDelegate {
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
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
