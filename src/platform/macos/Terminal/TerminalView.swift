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
    private var fontFamily: String
    private(set) var fontSize: CGFloat
    weak var inputHandler: TerminalInputHandler?
    /// tmux 控制模式下，SwiftTerm 解析 pane 输出时生成的查询应答（OSC 10/11、
    /// CSI DA/DSR、DCS 等）必须丢弃：tmux 拥有 pane 的 PTY 与终端协议，应答
    /// 经 `send-keys -l` 回写会被 pane 回显并执行，造成 `git lg` 的
    /// `10;rgb:...` / `65;...c` 泄漏成 shell 字面命令。
    ///
    /// 仅本地模式保持转发（前端就是该 PTY 的终端模拟器，写回 pty 是正确
    /// 行为）；tmux / daemon 代理一律丢弃。
    var suppressOutputDrivenResponses = false
    /// 正在 feed 远端 pane 输出（解析器应答只在这个窗口内产生）。
    private var isFeedingRemoteOutput = false
    /// 供 XCUITest 读取的可见输出片段（与 feed 同步）。
    private(set) var accessibilityOutput: String = ""
    /// AX 屏幕文本的刷新节流：全屏逐格读取有开销，无需每个 chunk 都更新。
    private var lastAccessibilityUpdate = Date.distantPast
    private static let accessibilityUpdateInterval: TimeInterval = 1.0
    /// 渲染诊断输出：设置 `MUXTERM_DEBUG_RENDER=1` 后，把每帧 feed 的长度、
    /// ESC/CR/LF 计数、模型尺寸与光标前后位置追加到
    /// `~/.config/muxterm/ui-render.log`，用于定位 agent 输入框逐行堆叠。
    private static let renderDebugURL: URL? = {
        guard ProcessInfo.processInfo.environment["MUXTERM_DEBUG_RENDER"] != nil else {
            return nil
        }
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/muxterm", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("ui-render.log")
    }()
    /// 上一次按像素驱动后的模型行列，用于检测窗口 resize 这一瞬间。
    private var lastModelCols = 0
    private var lastModelRows = 0

    init(
        paneId: UInt32,
        fontFamily: String = MuxtermTerminalFont.defaultFamily,
        fontSize: CGFloat = MuxtermTerminalFont.defaultSize,
        frame: NSRect = .zero
    ) {
        self.paneId = paneId
        self.fontFamily = fontFamily
        self.fontSize = MuxtermTerminalFont.clamp(fontSize)
        super.init(frame: frame)
        // SwiftTerm 为每条可见滚动条预留 ~16pt，模型列数会比 tmux pane 少
        // 1–2 列（`processSizeChange` 用 getEffectiveWidth 扣掉滚动条宽度）。
        // agent 帧按 pane 宽度折行，SwiftTerm 提前折行，erase-up 行数对不上，
        // 换行后输入区/提示内容被擦掉（1721/1740/1745 同族根因）。Muxterm
        // 不需要可见滚动条指示器（触控板/滚轮滚动照常），隐藏它让模型宽度
        // = pane 宽度，与 `refresh-client -C` 发给 tmux 的列数一致。
        subviews.first(where: { $0 is NSScroller })?.isHidden = true
        terminalDelegate = self
        wantsLayer = true
        font = Self.makeFont(family: fontFamily, size: self.fontSize)
        // 主题与终端内所有颜色绑定：新建视图用当前 activePalette
        // （默认浅色白底黑字；dark 才是 Mocha 深色）。
        applyPalette(MuxtermTerminalColors.activePalette)
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
        if isSnapshot {
            // 快照替换当前屏：清掉误喂的 capture 历史，避免滚动条里
            // 一万行旧输出、切 tab 时从很早刷到现在。
            getTerminal().resetToInitialState()
        }
        let cursorBefore = getTerminal().getCursorLocation()
        let bytes = [UInt8](data)
        // SwiftTerm 同步解析输出：查询应答经 `Terminal.sendResponse` 在 feed
        // 调用栈内同步发出。用这个标记把「解析 pane 输出产生的应答」与
        // 「用户输入 / 鼠标上报」区分开；tmux 镜像下一律丢弃（tmux 自己
        // 代答查询，前端回写会被 pane 回显成 git lg 字面乱码）。
        isFeedingRemoteOutput = true
        feed(byteArray: bytes[...])
        isFeedingRemoteOutput = false
        if Self.renderDebugURL != nil {
            let after = getTerminal().getCursorLocation()
            let dims = getTerminal().getDims()
            appendRenderDebug(
                "pane=\(paneId) snapshot=\(isSnapshot) len=\(data.count) "
                    + "esc=\(bytes.filter { $0 == 0x1b }.count) "
                    + "cr=\(bytes.filter { $0 == 0x0d }.count) "
                    + "lf=\(bytes.filter { $0 == 0x0a }.count) "
                    + "dims=\(dims.cols)x\(dims.rows) "
                    + "cursor=\(cursorBefore.x),\(cursorBefore.y)->\(after.x),\(after.y)"
            )
        }
        // AX 反映「当前屏幕」而不是 feed 历史：之前累积所有 feed 文本，
        // 输入/状态区的每一帧中间状态都会留在 AX 值里，看起来像逐帧堆叠。
        let now = Date()
        if now.timeIntervalSince(lastAccessibilityUpdate) >= Self.accessibilityUpdateInterval {
            lastAccessibilityUpdate = now
            updateAccessibilityOutput()
        }
    }

    private func appendRenderDebug(_ line: String) {
        guard let url = Self.renderDebugURL else { return }
        if !FileManager.default.fileExists(atPath: url.path) {
            FileManager.default.createFile(atPath: url.path, contents: nil)
        }
        guard let handle = try? FileHandle(forWritingTo: url) else { return }
        handle.seekToEndOfFile()
        handle.write(Data((line + "\n").utf8))
        try? handle.close()
    }

    /// 当前屏幕可见文本（测试 / AX）。立即刷新，不受 1s 节流限制。
    func visibleScreenText() -> String {
        updateAccessibilityOutput()
        return accessibilityOutput
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

    /// 测试用：把当前视图画进 bitmap，扫第一行字符格的亮度范围。
    /// 黑底黑字未抬亮时 max≈0；对比度修正后字形像素会明显亮于背景。
    func sampleFirstRowLuminanceRange() -> (min: Int, max: Int)? {
        layoutSubtreeIfNeeded()
        displayIfNeeded()
        let bounds = self.bounds
        guard bounds.width > 8, bounds.height > 8 else { return nil }
        guard let rep = bitmapImageRepForCachingDisplay(in: bounds) else { return nil }
        cacheDisplay(in: bounds, to: rep)
        let cellH = max(8, terminalCellSizeInPixels()?.height ?? 16)
        let height = min(cellH, rep.pixelsHigh)
        guard height > 0, rep.pixelsWide > 0 else { return nil }
        func scan(from y0: Int, to y1: Int) -> (min: Int, max: Int) {
            var minL = 255 * 3
            var maxL = 0
            for y in y0..<y1 {
                for x in 0..<rep.pixelsWide {
                    guard let color = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else {
                        continue
                    }
                    let s = Int((color.redComponent * 255).rounded())
                        + Int((color.greenComponent * 255).rounded())
                        + Int((color.blueComponent * 255).rounded())
                    minL = min(minL, s)
                    maxL = max(maxL, s)
                }
            }
            return (minL, maxL)
        }
        let top = scan(from: 0, to: height)
        let bottom = scan(from: max(0, rep.pixelsHigh - height), to: rep.pixelsHigh)
        // 黑底行更暗；bitmap 原点可能在顶部或底部。
        return top.min <= bottom.min ? top : bottom
    }

    /// 当前 SwiftTerm 字符格的 backing pixel 尺寸。
    func terminalCellSizeInPixels() -> (width: Int, height: Int)? {
        cellSizeInPixels(source: getTerminal())
    }

    /// 当前外观下的前景/背景 hex（`rrggbb`），供 tmux `refresh-client -r`
    /// 上报，让 tmux 代答 pane 的 OSC 10/11 颜色查询。
    func themeHexColors() -> (fg: String, bg: String) {
        (hexString(nativeForegroundColor), hexString(nativeBackgroundColor))
    }

    private func hexString(_ color: NSColor) -> String {
        guard let c = color.usingColorSpace(.sRGB) else { return "000000" }
        let r = Int((c.redComponent * 255.0).rounded())
        let g = Int((c.greenComponent * 255.0).rounded())
        let b = Int((c.blueComponent * 255.0).rounded())
        return String(format: "%02x%02x%02x", r, g, b)
    }

    private static func color(hex: String) -> NSColor {
        let value = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        guard value.count == 6,
              let rgb = UInt32(value, radix: 16)
        else {
            return NSColor.textColor
        }
        return NSColor(
            srgbRed: CGFloat((rgb >> 16) & 0xff) / 255.0,
            green: CGFloat((rgb >> 8) & 0xff) / 255.0,
            blue: CGFloat(rgb & 0xff) / 255.0,
            alpha: 1.0
        )
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
        // 窗口 resize 时模型行列变化才做一次全屏重绘：清除 resize 后的残留行。
        // SwiftTerm 的 queuePendingDisplay 是 internal 无法跨模块调用，
        // 所以用 setNeedsDisplay 触发 AppKit 渲染循环（只在 resize 时，
        // 不在每次 feed 时——避免高频输出时刷屏和滚动闪烁）。
        // 渲染纪律（§2.15.2 追加 B）：输出直接渲染到末尾位置，不逐帧滚动刷新。
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
        return true
    }

    func forceRedraw() {
        needsDisplay = true
        // 触达 Metal/CG 显示路径
        if let layer {
            layer.setNeedsDisplay()
        }
    }

    /// 运行期切换主题：把默认前景/背景/光标/选区/ANSI 16 色全部写成当前 palette。
    func applyPalette(_ palette: MuxtermPalette) {
        let painted = palette.contrasted()
        nativeForegroundColor = Self.color(hex: painted.fg)
        nativeBackgroundColor = Self.color(hex: painted.bg)
        caretColor = Self.color(hex: painted.cursor)
        caretTextColor = Self.color(hex: painted.bg)
        selectedTextBackgroundColor = Self.color(hex: painted.cursor)
            .withAlphaComponent(0.35)
        layer?.backgroundColor = nativeBackgroundColor.cgColor
        let ansi = painted.ansi.prefix(16).map { Self.terminalColor(hex: $0) }
        if ansi.count == 16 {
            installColors(Array(ansi))
        }
        forceRedraw()
    }

    /// 运行期切换主题（浅色/深色）：把传入 hex 写进终端默认色并重绘。
    func setThemeColors(fgHex: String, bgHex: String) {
        let pair = ColorContrast.themeColors(fg: fgHex, bg: bgHex)
        nativeForegroundColor = Self.color(hex: pair.fg)
        nativeBackgroundColor = Self.color(hex: pair.bg)
        layer?.backgroundColor = nativeBackgroundColor.cgColor
        forceRedraw()
    }

    private static func terminalColor(hex: String) -> Color {
        let ns = color(hex: hex)
        guard let c = ns.usingColorSpace(.sRGB) else {
            return Color(red: 0, green: 0, blue: 0)
        }
        return Color(
            red: UInt16((c.redComponent * 65535.0).rounded()),
            green: UInt16((c.greenComponent * 65535.0).rounded()),
            blue: UInt16((c.blueComponent * 65535.0).rounded())
        )
    }

    /// 运行期修改字体（Cmd +/- / Cmd 0）；SwiftTerm 会重算字符格并 resize 模型。
    func setFont(family: String? = nil, size: CGFloat? = nil) {
        if let family, !family.isEmpty {
            fontFamily = family
        }
        if let size {
            fontSize = MuxtermTerminalFont.clamp(size)
        }
        font = Self.makeFont(family: fontFamily, size: fontSize)
    }

    private static func makeFont(family: String, size: CGFloat) -> NSFont {
        NSFont(name: family, size: size)
            ?? NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
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
