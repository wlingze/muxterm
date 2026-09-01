import AppKit
import MuxtermChrome
import SwiftTerm

/// 与 SwiftTerm `computeFontDimensions()` 相同的首屏字符格估算。
/// 已创建的 view 直接读取 SwiftTerm 实际 backing-pixel 尺寸；这里只供
/// attach spawn 前尚无 Surface 时计算初始 client hint。
enum MuxTerminalGridMetrics {
    static func makeFont(family: String, size: CGFloat) -> NSFont {
        NSFont(name: family, size: size)
            ?? NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    static func cellSizeInPoints(
        family: String,
        size: CGFloat,
        backingScale: CGFloat
    ) -> (width: CGFloat, height: CGFloat) {
        let font = makeFont(family: family, size: size)
        let glyph = font.glyph(withName: "W")
        let rawWidth = font.advancement(forGlyph: glyph).width
        let rawHeight = ceil(font.ascender - font.descender + font.leading)
        let scale = max(backingScale, 1)
        return (
            max(ceil(rawWidth * scale) / scale, 1),
            max(ceil(rawHeight * scale) / scale, 1)
        )
    }

    static func clientSize(
        bounds: NSSize,
        family: String,
        size: CGFloat,
        backingScale: CGFloat
    ) -> (UInt16, UInt16)? {
        guard bounds.width >= 16, bounds.height >= 17 else { return nil }
        let cell = cellSizeInPoints(
            family: family,
            size: size,
            backingScale: backingScale
        )
        let cols = Int(floor(bounds.width / cell.width))
        let rows = Int(floor(bounds.height / cell.height))
        guard cols >= 2, rows >= 1, cols < 10_000, rows < 10_000 else { return nil }
        return (UInt16(cols), UInt16(rows))
    }
}

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
    /// 原生 SwiftTerm scrollback 位置变化；TerminalManager 将其镜像到 core。
    var onScrollPositionChanged: ((UInt32, Double, Bool) -> Void)?
    /// 诊断/回归测试：Surface seed 之外不允许发生 reset。
    private(set) var snapshotResetCount = 0
    /// 已经写入过 attach 前历史。snapshot reset 后清掉，允许再 prepend。
    private(set) var historyPrepended = false
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
    /// `scrollWheel` / 点击临时放行的用户 mouse report。SwiftTerm 把鼠标
    /// 上报也交给 `send(source: Terminal)`；它不能和 pane 输出解析器应答
    /// 走同一条丢弃策略，否则 htop 点击、TUI 滚轮都到不了 tmux。
    private var isSendingUserMouseReport = false
    /// 供 XCUITest 读取的可见输出片段（与 feed 同步）。
    private(set) var accessibilityOutput: String = ""
    private(set) var lastScrollWheelRoutedToRuntime = false
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
    /// headless/布局过渡期间 AppKit 可能短暂给 pane 一个 0~2 列的 frame；
    /// tmux 报告的字符格是模型的下限，不能让它被临时像素尺寸缩成 2×1。
    private var minimumModelCols = 2
    private var minimumModelRows = 1

    /// 视图尚未拿到 core pane 配置时的兜底历史容量。tmux/SSH surface
    /// 创建后由 `TerminalManager` 按 core 的实际历史上限动态扩容；local
    /// PTY 没有这个查询，继续保留原来的 10k 行体验。
    private static let fallbackHistoryCapacity = 10_000

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
        // 隐藏滚动条让模型宽度 = pane 宽度，与 `refresh-client -C` 列数一致。
        // 触控板交给 SwiftTerm：htop/Cursor 在 alt-screen 里自己消化滚动；
        // 普通 shell 滚 attach 之后的本地 scrollback。禁止 RIS 喂历史 dump。
        subviews.first(where: { $0 is NSScroller })?.isHidden = true
        getTerminal().changeHistorySize(Self.fallbackHistoryCapacity)
        terminalDelegate = self
        // OSC 133 是 shell/agent 的 FinalTerm command lifecycle 标记。
        // Attention/index 在 core 侧消费同一字节流；SwiftTerm 只需要吞掉该
        // 非绘制序列，避免每个命令都打印 `Unknown OSC code: 133`，不能把
        // 原始字节从 Surface feed 路径移除或改写。
        getTerminal().registerOscHandler(code: 133) { _ in }
        wantsLayer = true
        font = Self.makeFont(family: fontFamily, size: self.fontSize)
        // 主题与终端内所有颜色绑定：新建视图用当前 activePalette
        // （默认浅色白底黑字；dark 才是 Mocha 深色）。
        applyPalette(MuxtermTerminalColors.activePalette)
        // 应用打开 mouse 协议时（htop/vim/Codex），点击必须回写 pane。
        // 未开 mouse 时 SwiftTerm 仍做本地选区；Shift 继续绕过上报。
        allowMouseReporting = true
        setAccessibilityIdentifier("muxterm.terminal.\(paneId)")
        setAccessibilityElement(true)
        setAccessibilityRole(.textArea)
        setAccessibilityLabel(
            MuxtermI18n.shared.tr(.terminalPane, arguments: ["id": "\(paneId)"])
        )
        setAccessibilityValue("")
    }

    /// 用户鼠标走 `send(source: Terminal)`，tmux 镜像默认会丢掉解析器应答。
    /// 在点击/拖拽/滚轮期间打开上报并标记，才能把 CSI 送进 pane。
    private func withUserMouseReporting(_ body: () -> Void) {
        let previous = allowMouseReporting
        allowMouseReporting = true
        isSendingUserMouseReport = true
        defer {
            isSendingUserMouseReport = false
            allowMouseReporting = previous
        }
        body()
    }

    override func scrollWheel(with event: NSEvent) {
        // tmux 的 agent TUI（Codex/Cursor）在 alternate screen 中自己维护
        // 历史。此时本地 SwiftTerm scrollback 没有意义，把鼠标上报保持打开，
        // 让 tmux/agent 的滚轮绑定处理；不回写 core viewport。
        if getTerminal().isCurrentBufferAlternate {
            lastScrollWheelRoutedToRuntime = true
            let previous = allowMouseReporting
            allowMouseReporting = true
            isSendingUserMouseReport = true
            super.scrollWheel(with: event)
            isSendingUserMouseReport = false
            allowMouseReporting = previous
            return
        }
        lastScrollWheelRoutedToRuntime = false
        withUserMouseReporting { super.scrollWheel(with: event) }
    }

    override func mouseDown(with event: NSEvent) {
        withUserMouseReporting { super.mouseDown(with: event) }
    }

    override func mouseUp(with event: NSEvent) {
        withUserMouseReporting { super.mouseUp(with: event) }
    }

    override func mouseDragged(with event: NSEvent) {
        withUserMouseReporting { super.mouseDragged(with: event) }
    }

    override func rightMouseDown(with event: NSEvent) {
        withUserMouseReporting { super.rightMouseDown(with: event) }
    }

    override func rightMouseUp(with event: NSEvent) {
        withUserMouseReporting { super.rightMouseUp(with: event) }
    }

    override func rightMouseDragged(with event: NSEvent) {
        withUserMouseReporting { super.rightMouseDragged(with: event) }
    }

    override func otherMouseDown(with event: NSEvent) {
        withUserMouseReporting { super.otherMouseDown(with: event) }
    }

    override func otherMouseUp(with event: NSEvent) {
        withUserMouseReporting { super.otherMouseUp(with: event) }
    }

    override func otherMouseDragged(with event: NSEvent) {
        withUserMouseReporting { super.otherMouseDragged(with: event) }
    }

    /// SwiftTerm 的 copy/paste 签名是 `Any`，对不上 NSResponder 的 `Any?`。
    /// Edit 菜单和 Cmd-C/V 走的是 NSResponder，必须在这里写剪贴板 / 发给 pane。
    override func copy(_ sender: Any?) {
        guard let text = getSelection() else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
    }

    override func paste(_ sender: Any?) {
        let text = NSPasteboard.general.string(forType: .string) ?? ""
        guard !text.isEmpty else { return }
        if getTerminal().bracketedPasteMode {
            send(data: EscapeSequences.bracketedPasteStart[...])
            send(txt: text)
            send(data: EscapeSequences.bracketedPasteEnd[...])
        } else {
            insertText(text, replacementRange: NSRange(location: 0, length: 0))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    /// 将 FFI 输出喂给终端引擎，并更新 AX 值供 UITest 断言「确实渲染到了」。
    func feedOutput(_ data: Data, isSnapshot: Bool = false) {
        if isSnapshot {
            // 只允许新建 Surface 的一次性 seed reset；历史 seed 随后进入
            // SwiftTerm 原生 scrollback，不能在 live/滚轮路径重复调用。
            snapshotResetCount += 1
            historyPrepended = false
            getTerminal().resetToInitialState()
        }
        // A zero-byte snapshot is meaningful: it clears an authoritative blank
        // pane. Incremental empty output remains a no-op.
        guard !data.isEmpty else { return }
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

    /// 将 Runtime 的完整 frame 替换到当前屏幕。
    ///
    /// `PaneFrame` 不是 SwiftTerm 的 reset/seed：它只清当前可见网格并把
    /// 原始 ANSI frame 从光标起点重画，保留 native scrollback、终端模式和
    /// 用户当前历史容量。Herdr 的 full frame 通常不自带清屏序列，若直接
    /// append 到旧屏幕，Cursor/Pi 的历史帧就会逐帧堆叠。
    func feedFull(_ data: Data) {
        var frame = Data(capacity: data.count + 7)
        frame.append(contentsOf: [0x1b, 0x5b, 0x32, 0x4a, 0x1b, 0x5b, 0x48])
        frame.append(data)
        feedOutput(frame)
    }

    /// attach 前历史写入 native scrollback。不得 reset，也不得当 VT 流重放。
    func prependHistoryLines(_ lines: [String]) {
        guard !PaneHistorySeedPolicy.shouldResetTerminal() else { return }
        guard !lines.isEmpty, !historyPrepended else { return }
        historyPrepended = true
        ensureHistoryCapacity(atLeast: historyCapacity + lines.count)
        getTerminal().muxtermPrependHistoryLines(lines)
        updateAccessibilityOutput()
    }

    /// 当前 native VT 是否位于最新输出尾部。
    func isAtLatest() -> Bool {
        !canScroll || scrollPosition >= 0.999
    }

    /// 将 core 的历史 offset 映射到 SwiftTerm 原生 scrollback。
    func scrollToHistoryOffset(_ offset: UInt32, maxOffset: UInt32) {
        guard maxOffset > 0 else {
            scroll(toPosition: 1.0)
            return
        }
        let clamped = min(offset, maxOffset)
        let position = 1.0 - (Double(clamped) / Double(maxOffset))
        scroll(toPosition: position)
    }

    func scrollToLatest() {
        scroll(toPosition: 1.0)
    }

    func scrollLines(_ lines: Int) {
        if lines > 0 {
            scrollUp(lines: lines)
        } else if lines < 0 {
            scrollDown(lines: -lines)
        }
    }

    /// 当前 normal VT buffer 的 scrollback 容量（alternate screen 不使用）。
    var historyCapacity: Int {
        getTerminal().options.scrollback
    }

    /// 增大原生 scrollback 容量而不 reset / 重播 Surface。SwiftTerm 的
    /// `changeHistorySize` 只扩容时会保留现有行，适合 core 历史继续增长；
    /// 缩容永远不在 live 路径做，避免用户正在上划时丢掉视口。
    func ensureHistoryCapacity(atLeast minimum: Int) {
        let desired = max(1, minimum)
        guard desired > historyCapacity else { return }
        getTerminal().changeHistorySize(desired)
    }

    func setMinimumModelSize(cols: Int, rows: Int) {
        minimumModelCols = max(2, cols)
        minimumModelRows = max(1, rows)
    }

    /// 把 SwiftTerm 模型对齐到 tmux pane 格子。必须允许缩小：attach 用
    /// 8×17 估出来的 128×63 会比窗口真实格子大，只涨不缩会把 prompt
    /// 留在可见区域下面。
    func applyGridSize(cols: Int, rows: Int, followTail: Bool) {
        guard let target = PaneGridSyncPolicy.modelSize(tmuxCols: cols, tmuxRows: rows) else {
            return
        }
        minimumModelCols = target.cols
        minimumModelRows = target.rows
        let term = getTerminal()
        let shouldFollow = followTail || isAtLatest()
        let didResize = PaneGridSyncPolicy.shouldResize(
            currentCols: term.cols,
            currentRows: term.rows,
            tmuxCols: target.cols,
            tmuxRows: target.rows
        )
        guard didResize else { return }
        term.resize(cols: target.cols, rows: target.rows)
        lastModelCols = target.cols
        lastModelRows = target.rows
        term.updateFullScreen()
        setNeedsDisplay(bounds)
        if let layer { layer.setNeedsDisplay() }
        if PaneGridFollowPolicy.shouldScrollToLatest(didResize: true, followTail: shouldFollow) {
            scrollToLatest()
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

    /// 字符格的 point 尺寸。`refresh-client -C` 必须用这个去除以
    /// `bounds`（也是 point）。再除一次 backingScale 会把 93×51 变成 186×102。
    func terminalCellSizeInPoints() -> (width: CGFloat, height: CGFloat)? {
        guard let pixels = terminalCellSizeInPixels() else { return nil }
        let scale = max(window?.backingScaleFactor ?? 1, 1)
        return (
            CGFloat(pixels.width) / scale,
            CGFloat(pixels.height) / scale
        )
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
    /// `exactGrid` 存在时（tmux pane 格子）模型必须等于它，允许缩小。
    /// 返回是否成功同步到合法行列（≥2×1）。
    @discardableResult
    func syncSizeToPty(
        notifyResize: Bool = true,
        exactGrid: (cols: Int, rows: Int)? = nil
    ) -> Bool {
        layoutSubtreeIfNeeded()
        let size = bounds.size
        guard size.width >= 40, size.height >= 24 else { return false }

        let positionBeforeResize = scrollPosition
        let atLatestBeforeResize = isAtLatest()

        // SwiftTerm setFrameSize 会 processSizeChange；这里只调用一次，避免重复回调。
        if frame.size != size {
            setFrameSize(size)
        }

        let term = getTerminal()
        let modelCols: Int
        let modelRows: Int
        if let exact = exactGrid,
           let target = PaneGridSyncPolicy.modelSize(tmuxCols: exact.cols, tmuxRows: exact.rows)
        {
            // tmux 格子是真相。processSizeChange 可能刚按像素写成另一套
            // 行列；随后必须改回 pane 尺寸，否则 prompt 会掉到窗口外。
            modelCols = target.cols
            modelRows = target.rows
            minimumModelCols = target.cols
            minimumModelRows = target.rows
        } else {
            modelCols = max(term.cols, minimumModelCols)
            modelRows = max(term.rows, minimumModelRows)
        }
        if term.cols != modelCols || term.rows != modelRows {
            term.resize(cols: modelCols, rows: modelRows)
        }
        guard term.cols >= 2, term.rows >= 1 else { return false }
        // AppKit 初次挂载/窗口 resize 可能让 SwiftTerm 的 Buffer 把 yDisp
        // 暂时归零。若用户原来在底部，恢复到最新；若用户正在看历史，
        // 保留原来的相对位置，不能被尺寸同步偷偷拉到底部或顶端。
        if atLatestBeforeResize {
            scroll(toPosition: 1.0)
        } else if positionBeforeResize > 0 {
            scroll(toPosition: positionBeforeResize)
        }
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
        if let family, !family.isEmpty, family != fontFamily {
            fontFamily = family
        }
        if let size {
            fontSize = MuxtermTerminalFont.clamp(size)
        }
        let next = Self.makeFont(family: fontFamily, size: fontSize)
        if font == next { return }
        font = next
    }

    private static func makeFont(family: String, size: CGFloat) -> NSFont {
        MuxTerminalGridMetrics.makeFont(family: family, size: size)
    }

    /// 覆写 SwiftTerm 模拟器输出通道：只有 `Terminal.sendResponse` /
    /// `sendFocusReport` 等模拟器生成的事件走这里（`source: Terminal`）；
    /// 键盘/粘贴/kitty 用户输入走 `TerminalViewDelegate.send(source: TerminalView)`
    /// 的 `send(data:)` 路径，不受影响。
    override func send(source: Terminal, data: ArraySlice<UInt8>) {
        if isSendingUserMouseReport {
            guard TerminalMirrorPolicy.shouldForwardUserInitiatedMouseReport(
                isTmuxMirror: suppressOutputDrivenResponses
            ) else { return }
            super.send(source: source, data: data)
            return
        }
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

    func scrolled(source: TerminalView, position: Double) {
        onScrollPositionChanged?(paneId, position, isAtLatest())
    }

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
