import AppKit
import MuxtermChrome

/// 管理多个 pane 对应的 `MuxTerminalView`，并把输出/输入接到 CoreBridge。
final class TerminalManager: TerminalInputHandler {
    private weak var bridge: CoreBridge?
    private var views: [UInt32: MuxTerminalView] = [:]
    /// 后端报告的 pane 字符格尺寸：视图创建时先按它 resize SwiftTerm 模型，
    /// 再喂快照/增量。否则模型默认 80 列，codex 的 93 列帧会折行、erase-up
    /// 行数对不上，输入内容逐帧滚出屏幕（1745）。
    private var expectedPaneSizes: [UInt32: (cols: Int, rows: Int)] = [:]
    private var fontFamily: String
    private var fontSize: CGFloat
    /// 本轮 poll 批次内新建的视图：播种快照已覆盖该批次所有已入队的
    /// PaneOutput 事件，本批次剩余事件必须跳过，否则同一批字节会双写。
    private var viewsCreatedThisBatch = Set<UInt32>()
    /// 已经用可见网格 / 末屏给 SwiftTerm 做过首屏的 pane。
    /// 之后再来的 capture 历史必须走 firstPaint，不能当 live 重放。
    private var swiftTermSeeded = Set<UInt32>()
    private var inEventBatch = false
    /// 最近喂给终端的 UTF-8 片段（供 UITest / 状态栏无障碍查询）。
    private(set) var recentOutputSnippet: String = ""
    /// 上次成功同步到 PTY 的行列，避免无意义重复 resize。
    private var lastPtySize: [UInt32: (UInt16, UInt16)] = [:]
    /// 上次发送给 tmux control client 的整体尺寸。
    private var lastClientSize: (UInt16, UInt16)?
    /// 已排队但尚未发送的整体尺寸；窗口 live resize 期间只保留最后一帧。
    private var pendingClientSize: (UInt16, UInt16)?
    private var clientResizeWorkItem: DispatchWorkItem?
    /// 同一 pane 待合并的增量输出：codex/agent 一帧会被拆成多个 PaneOutput
    /// 事件，分帧喂给 SwiftTerm 会让中间态把输入行逐帧推走（一直换行），
    /// 也造成高频重绘；合并后一次 feed 保持稳定。
    private var pendingFeeds: [UInt32: Data] = [:]
    private var feedFlushWorkItem: DispatchWorkItem?
    private static let feedFlushInterval: TimeInterval = 0.033 // 合并同一 pane 短窗口输出（约 30fps），减少中间态闪烁
    /// 同一 pane 的 resize 失败只报告一次，避免轮询/重绘时刷屏。
    private var reportedResizeFailures = Set<UInt32>()
    private var reportedClientResizeFailure = false

    /// SSH 流量统计：累计接收字节数 + 最近窗口速率（bytes/s）。
    /// 供 statusbar 的 SSH Traffic Monitor 显示实时下行速率。
    private(set) var totalBytesReceived: UInt64 = 0
    private var trafficTimestamps: [TimeInterval] = []
    private var trafficByteCounts: [UInt64] = []
    private var lastTrafficRate: UInt64 = 0
    /// 最近一次统计更新时间，用于周期刷新 statusbar 流量显示。
    private(set) var trafficRate: UInt64 = 0

    weak var focusTarget: MuxTerminalView?
    var onOutputSnippetChanged: ((String) -> Void)?
    var onError: ((String) -> Void)?
    /// viewport 变化（滚轮 / 回底）：窗口用来显示跳转最新按钮。
    var onViewportChanged: ((UInt32, UInt32) -> Void)?
    /// 离开底部期间新增的行数变化；窗口用来显示 `↓ +N`。
    var onUnseenLinesChanged: ((UInt32, UInt32) -> Void)?

    private var unseenLines: [UInt32: UInt32] = [:]
    /// `applyViewport`/回底是程序主动改变 native scroll position，回调只做
    /// 重绘通知，不再把 native 的浮点位置反算回 core，避免搜索跳转漂移。
    private var applyingNativeScroll = Set<UInt32>()

    deinit {
        clientResizeWorkItem?.cancel()
    }

    init(
        bridge: CoreBridge,
        fontFamily: String = MuxtermTerminalFont.defaultFamily,
        fontSize: CGFloat = MuxtermTerminalFont.defaultSize
    ) {
        self.bridge = bridge
        self.fontFamily = fontFamily
        self.fontSize = MuxtermTerminalFont.clamp(fontSize)
    }

    /// 连接面板切换 local / SSH session 后更新桥接对象。
    func updateBridge(_ bridge: CoreBridge) {
        self.bridge = bridge
        for view in views.values {
            view.removeFromSuperview()
        }
        views.removeAll()
        expectedPaneSizes.removeAll()
        viewsCreatedThisBatch.removeAll()
        swiftTermSeeded.removeAll()
        inEventBatch = false
        lastPtySize.removeAll()
        lastClientSize = nil
        pendingClientSize = nil
        clientResizeWorkItem?.cancel()
        clientResizeWorkItem = nil
        feedFlushWorkItem?.cancel()
        feedFlushWorkItem = nil
        pendingFeeds.removeAll()
        reportedResizeFailures.removeAll()
        reportedClientResizeFailure = false
        recentOutputSnippet = ""
        totalBytesReceived = 0
        trafficTimestamps.removeAll()
        trafficByteCounts.removeAll()
        trafficRate = 0
        unseenLines.removeAll()
        applyingNativeScroll.removeAll()
        onOutputSnippetChanged?(recentOutputSnippet)
    }

    /// 获取或创建指定 pane 的终端视图。
    func view(for paneId: UInt32) -> MuxTerminalView {
        if let existing = views[paneId] {
            return existing
        }
        let view = MuxTerminalView(
            paneId: paneId,
            fontFamily: fontFamily,
            fontSize: fontSize
        )
        view.inputHandler = self
        view.onScrollPositionChanged = { [weak self] paneId, position, _ in
            self?.handleNativeScroll(paneId: paneId, position: position)
        }
        // 非直接 PTY 终端模拟器（tmux 控制模式 / daemon 代理）下禁止把
        // SwiftTerm 解析 pane 输出时生成的查询应答回写 pane。
        view.suppressOutputDrivenResponses = !isDirectPtyTerminal
        views[paneId] = view
        view.applyPalette(MuxtermTerminalColors.activePalette)
        // 先按 pane 真实尺寸 resize 模型：codex/cursor 的 erase-up 重绘按
        // 实际列数生成，模型宽度不一致会折行导致输入行逐帧漂移。
        if let size = expectedPaneSizes[paneId], size.cols >= 2, size.rows >= 1 {
            view.setMinimumModelSize(cols: size.cols, rows: size.rows)
            view.getTerminal().resize(cols: size.cols, rows: size.rows)
        }
        syncHistoryCapacity(paneId: paneId, view: view)
        // 首屏把内置 VT 的带样式历史一次性种进 SwiftTerm 原生 scrollback。
        // 之后只喂 `%output` 增量；滚轮/搜索不再拿历史 dump 重置屏幕。
        let rows = expectedPaneSizes[paneId]?.rows ?? 24
        let seed = bridge?.paneSurfaceSeedANSI(paneId: paneId) ?? Data()
        let snapshot: Data
        let hasSurfaceSeed = !seed.isEmpty
        if seed.isEmpty {
            let visible = bridge?.paneVisibleANSI(paneId: paneId) ?? Data()
            let raw = bridge?.getPaneOutput(paneId: paneId) ?? Data()
            snapshot = PanePaintPolicy.firstPaint(visible: visible, raw: raw, rows: rows)
        } else {
            snapshot = seed
        }
        if !snapshot.isEmpty {
            view.feedOutput(snapshot, isSnapshot: true)
            appendSnippet(snapshot)
            if hasSurfaceSeed {
                // 写入大量历史行时 SwiftTerm 的 yDisp 在某些 resize/布局
                // 时序会停在最上方；Surface 新建完成的契约是从 live 尾部开始。
                applyingNativeScroll.insert(paneId)
                view.scrollToLatest()
                applyingNativeScroll.remove(paneId)
                _ = bridge?.setPaneViewport(paneId: paneId, offset: 0)
                swiftTermSeeded.insert(paneId)
                if inEventBatch {
                    viewsCreatedThisBatch.insert(paneId)
                }
            }
        }
        return view
    }

    /// 处理 PaneOutput 增量事件。
    /// 记录后端报告的 pane 尺寸（供新视图创建时先 resize 模型再喂帧）。
    func updatePaneSizes(_ panes: [Pane]) {
        expectedPaneSizes = Dictionary(
            uniqueKeysWithValues: panes.map { ($0.id, (Int($0.cols), Int($0.rows))) }
        )
        for (paneId, size) in expectedPaneSizes {
            views[paneId]?.setMinimumModelSize(cols: size.cols, rows: size.rows)
            if let view = views[paneId] {
                syncHistoryCapacity(paneId: paneId, view: view)
            }
        }
    }

    /// SwiftTerm 的 native scrollback 必须至少覆盖 core 当前可滚动窗口。
    /// `history_max_offset` 是“离底行数”，加上 pane 行数后可覆盖 core
    /// 可能保留的空尾行；输出继续增长时在 flush 前再次扩容。只扩不缩，
    /// 因此不会因为 core 快照暂时变短而破坏用户当前历史视口。
    private func syncHistoryCapacity(paneId: UInt32, view: MuxTerminalView) {
        guard let bridge else { return }
        let rows = UInt32(max(1, expectedPaneSizes[paneId]?.rows ?? view.getTerminal().rows))
        let rawMax = bridge.paneHistoryMaxOffset(paneId: paneId, rows: rows)
        guard rawMax >= 0 else { return }
        let (sum, overflow) = Int(rawMax).addingReportingOverflow(Int(rows))
        let desired = overflow ? Int.max : sum
        view.ensureHistoryCapacity(atLeast: max(1, desired))
    }

    /// headless/未布局时 SwiftTerm 模型可能只有 0~1 行，喂字节会丢；
    /// 至少保证 80x24 的合法模型尺寸。
    private func ensureValidModelSize(_ view: MuxTerminalView) {
        let dims = view.getTerminal().getDims()
        if dims.cols < 2 || dims.rows < 2 {
            view.getTerminal().resize(cols: 80, rows: 24)
        }
    }

    func handleOutput(paneId: UInt32, data: Data) {
        guard !data.isEmpty else { return }
        // 事件字节就是真实增量（后端先 append 到累计缓冲、再入队事件）。
        // 不再拿累计缓冲快照做增量对账：前端缓冲（256KB）小于 pane 累计
        // 输出后，快照只是滑动窗口，按 fed_len 切片会追着陈旧头部，导致
        // codex/htop/agent 这类长运行 pane 冻结或乱码。
        if viewsCreatedThisBatch.contains(paneId) {
            // 本批次刚创建视图：播种快照已包含本批次所有已入队事件。
            return
        }
        let existed = views[paneId] != nil
        let view = view(for: paneId)
        // view(for:) 可能刚用可见网格播种；同一批 capture 事件必须丢掉。
        if viewsCreatedThisBatch.contains(paneId) {
            return
        }
        ensureValidModelSize(view)
        let rows = expectedPaneSizes[paneId]?.rows ?? 24
        if !swiftTermSeeded.contains(paneId) {
            // 首包：可见网格 / 末屏。attach 的 capture 历史绝不能当录像重放。
            // 之后的 live `%output` 即使很长（Codex 刷 GitHub 地址）也必须
            // 原样增量喂入，不能再 RIS 清屏。
            let surface = bridge?.paneSurfaceSeedANSI(paneId: paneId) ?? Data()
            let painted: Data
            if !surface.isEmpty {
                painted = surface
            } else {
                let visible = bridge?.paneVisibleANSI(paneId: paneId) ?? Data()
                painted = PanePaintPolicy.firstPaint(
                    visible: visible,
                    raw: data,
                    rows: rows
                )
            }
            if !painted.isEmpty {
                view.feedOutput(painted, isSnapshot: true)
                if !surface.isEmpty {
                    applyingNativeScroll.insert(paneId)
                    view.scrollToLatest()
                    applyingNativeScroll.remove(paneId)
                    _ = bridge?.setPaneViewport(paneId: paneId, offset: 0)
                }
            }
            swiftTermSeeded.insert(paneId)
            appendSnippet(painted)
            recordTraffic(bytes: data.count)
            return
        }
        // 即使用户正在看历史也必须继续 feed。SwiftTerm 的 native VT 会在
        // `userScrolling` 状态下保持当前 yDisp，同时把新行留在 scrollback；
        // 丢弃这里的数据会冻结 Cursor/htop/shell。
        if PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: existed,
            seedCoveredEvent: viewsCreatedThisBatch.contains(paneId)
        ) {
            // 视图早已存在：事件就是增量；或视图刚创建但快照为空（新 pane
            // 首批字节，种子没有覆盖任何事件），必须原样喂入。合并同一
            // pane 短窗口内的字节，减少 SwiftTerm 中间态漂移与重绘频率。
            pendingFeeds[paneId, default: Data()].append(data)
            appendSnippet(data)
            recordTraffic(bytes: data.count)
            scheduleFeedFlush()
        }
    }

    private func scheduleFeedFlush() {
        guard feedFlushWorkItem == nil else { return }
        let work = DispatchWorkItem { [weak self] in
            self?.feedFlushWorkItem = nil
            self?.flushPendingFeeds()
        }
        feedFlushWorkItem = work
        DispatchQueue.main.asyncAfter(
            deadline: .now() + Self.feedFlushInterval,
            execute: work
        )
    }

    func testFlushFeeds() {
        flushPendingFeeds()
    }

    private func flushPendingFeeds() {
        feedFlushWorkItem = nil
        let feeds = pendingFeeds
        pendingFeeds.removeAll()
        for (paneId, data) in feeds {
            let rows = expectedPaneSizes[paneId]?.rows ?? 24
            guard let view = views[paneId] else { continue }
            syncHistoryCapacity(paneId: paneId, view: view)
            let wasAtLatest = view.isAtLatest()
            if !wasAtLatest {
                let added = UInt32(data.reduce(into: 0) { count, byte in
                    if byte == 0x0a { count += 1 }
                })
                if added > 0 {
                    let current = unseenLines[paneId] ?? 0
                    unseenLines[paneId] = current.addingReportingOverflow(added).overflow
                        ? UInt32.max
                        : current + added
                    onUnseenLinesChanged?(paneId, unseenLines[paneId] ?? 0)
                }
            }
            view.feedOutput(PanePaintPolicy.live(data, visibleRows: rows))
            if wasAtLatest {
                view.scrollToLatest()
            }
        }
    }

    /// 每轮 poll 事件处理前调用，标记批次边界。
    func beginEventBatch() {
        inEventBatch = true
        // 只有本轮事件处理中创建的 view 才能确定 seed 覆盖了本批事件。
        // 批次外（例如切回 warm Workspace 后）创建的 view 不应抑制下一轮
        // 完整 poll，否则 seed 之后产生的新 prompt/token 会被整批丢掉。
        viewsCreatedThisBatch.removeAll()
    }

    /// 本轮 poll 事件处理完毕。
    func endEventBatch() {
        viewsCreatedThisBatch.removeAll()
        inEventBatch = false
    }

    /// 移除已关闭 pane 的视图（只在 STATE_PANE_CLOSED 时调用；
    /// 切 tab / 布局重建不得丢视图，否则 SwiftTerm 状态被清掉，
    /// 切回来重放被截断的累计输出会乱码 / 黑屏）。
    func removePane(_ paneId: UInt32) {
        pendingFeeds.removeValue(forKey: paneId)
        views[paneId]?.removeFromSuperview()
        views.removeValue(forKey: paneId)
        viewsCreatedThisBatch.remove(paneId)
        swiftTermSeeded.remove(paneId)
        unseenLines.removeValue(forKey: paneId)
        applyingNativeScroll.remove(paneId)
        lastPtySize.removeValue(forKey: paneId)
    }

    /// 当前连接是否由 tmux 控制 client 管理尺寸。
    var usesClientResize: Bool {
        switch bridge?.backendType {
        case "tmux", "ssh", "tmux-ssh":
            return true
        default:
            return false
        }
    }

    /// 前端是否为 pane PTY 的直接终端模拟器。
    ///
    /// 仅 `local` 模式是：SwiftTerm 就是该 PTY 的终端模拟器，查询应答写回
    /// pty 是正确行为。tmux 控制模式（`tmux` / `ssh`）以及 daemon 代理
    /// （daemon 可能代理 tmux，client 侧无法分辨）都不是，解析器应答
    /// 必须丢弃，否则经 send-keys 注入会泄漏成 shell 字面命令。
    var isDirectPtyTerminal: Bool {
        bridge?.backendType == "local"
    }

    /// 布局完成后：先更新各个 SwiftTerm 的本地渲染尺寸，再按后端类型同步尺寸。
    /// tmux 模式只发送一次整体 client resize，避免 pane resize 逐个触发布局反馈。
    func syncAllVisibleSizes(paneIds: Set<UInt32>, container: NSView? = nil) {
        for id in paneIds {
            guard let view = views[id] else { continue }
            view.layoutSubtreeIfNeeded()
            _ = view.syncSizeToPty(notifyResize: !usesClientResize)
        }
        if usesClientResize, let container {
            syncClientSize(container: container, paneIds: paneIds)
        }
    }

    /// 取任一可见终端的字符格 backing pixel 尺寸；同一窗口字体统一。
    func cellSizeInPixels(paneIds: Set<UInt32>) -> (width: Int, height: Int)? {
        paneIds.lazy.compactMap { self.views[$0]?.terminalCellSizeInPixels() }.first
    }

    /// 取任一可见终端的外观前景/背景 hex，用于 tmux `refresh-client -r`。
    func themeHexColors() -> (fg: String, bg: String)? {
        views.values.first?.themeHexColors()
    }

    /// 强制把 SwiftTerm 模型行列同步成 tmux 报告的 pane 尺寸。
    ///
    /// `cellSizeInPixels()` 是 backing pixel，AppKit `bounds` 是 point。
    /// 先用窗口的 backing scale 归一化成 point，再计算字符格；不能把
    /// 一个尚未完成 layer 初始化的 scale=1 cell 与 scale=2 container 混算。
    private func syncClientSize(container: NSView, paneIds: Set<UInt32>) {
        guard let size = clientGridSize(container: container, paneIds: paneIds) else {
            return
        }
        guard lastClientSize?.0 != size.0 || lastClientSize?.1 != size.1 else { return }
        guard pendingClientSize?.0 != size.0 || pendingClientSize?.1 != size.1 else { return }

        pendingClientSize = size
        clientResizeWorkItem?.cancel()
        let work = DispatchWorkItem { [weak self, weak container] in
            guard let self, let container else { return }
            self.pendingClientSize = nil
            self.clientResizeWorkItem = nil
            self.sendClientResize(container: container, paneIds: paneIds)
        }
        clientResizeWorkItem = work
        // live resize 每个像素都会触发 layout；延迟一个短帧，只把最终
        // 字符格尺寸写给 tmux，避免 refresh-client/layout-change 互相追赶。
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.04, execute: work)
    }

    private func sendClientResize(container: NSView, paneIds: Set<UInt32>) {
        guard let size = clientGridSize(container: container, paneIds: paneIds) else {
            return
        }
        guard lastClientSize?.0 != size.0 || lastClientSize?.1 != size.1 else { return }
        guard let bridge else { return }
        if bridge.resizeClient(cols: size.0, rows: size.1) == 0 {
            lastClientSize = size
            reportedClientResizeFailure = false
        } else if !reportedClientResizeFailure {
            reportedClientResizeFailure = true
            onError?(MuxtermI18n.shared.tr(.errorResizeClient))
        }
    }

    private func clientGridSize(
        container: NSView,
        paneIds: Set<UInt32>
    ) -> (UInt16, UInt16)? {
        guard let cell = cellSizeInPixels(paneIds: paneIds), cell.width > 0, cell.height > 0 else {
            return nil
        }
        let scale = max(container.window?.backingScaleFactor ?? 1, 1)
        let cellWidth = CGFloat(cell.width) / scale
        let cellHeight = CGFloat(cell.height) / scale
        let pointSize = container.bounds.size
        let cols = Int(floor(pointSize.width / cellWidth))
        let rows = Int(floor(pointSize.height / cellHeight))
        guard cols >= 2, rows >= 1, cols < 10000, rows < 10000 else { return nil }
        return (UInt16(cols), UInt16(rows))
    }

    /// 提交鼠标拖动后的单轴 pane 尺寸；tmux 会把结果保存到其窗口 layout。
    @discardableResult
    func resizePaneAxis(paneId: UInt32, horizontal: Bool, size: UInt16) -> Int32 {
        guard usesClientResize, let bridge else { return -1 }
        let rc = bridge.resizePaneAxis(paneId: paneId, horizontal: horizontal, size: size)
        if rc != 0 {
            onError?(MuxtermI18n.shared.tr(.errorResizeDivider, arguments: ["id": "\(paneId)"]))
        }
        return rc
    }

    /// 强制重绘（分割后 Metal/layer 偶发留黑）。
    func forceRedraw(paneIds: Set<UInt32>) {
        for id in paneIds {
            views[id]?.forceRedraw()
        }
    }

    /// 运行期切换主题：更新所有终端视图的默认色、光标、ANSI 16 色。
    func applyPalette(_ palette: MuxtermPalette) {
        for view in views.values {
            view.applyPalette(palette)
        }
    }

    /// 运行期切换主题：更新所有终端视图的默认前景/背景。
    func applyTheme(fgHex: String, bgHex: String) {
        for view in views.values {
            view.setThemeColors(fgHex: fgHex, bgHex: bgHex)
        }
    }

    /// 运行期调整所有终端视图的字体（Cmd +/- / Cmd 0），
    /// 并立即按新字符格重新同步 PTY / tmux client 尺寸。
    func setFont(family: String? = nil, size: CGFloat, container: NSView?) {
        if let family, !family.isEmpty {
            fontFamily = family
        }
        fontSize = MuxtermTerminalFont.clamp(size)
        for view in views.values {
            view.setFont(family: fontFamily, size: fontSize)
        }
        guard !views.isEmpty else { return }
        let ids = Set(views.keys)
        if usesClientResize, let container {
            // tmux/ssh：整体 client 尺寸变化由 syncClientSize 下发。
            syncClientSize(container: container, paneIds: ids)
        } else {
            // local：每个视图按新字符格重算行列并通知 PTY resize。
            for id in ids {
                views[id]?.syncSizeToPty(notifyResize: true)
            }
        }
        forceRedraw(paneIds: ids)
    }

    // MARK: - TerminalInputHandler

    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        // 仅转发到 FFI；显示只走 pty 回显的 PaneOutput（修双写）。
        if bridge?.sendInput(paneId: view.paneId, data: Data(data)) != 0 {
            onError?(MuxtermI18n.shared.tr(.errorSendInput, arguments: ["id": "\(view.paneId)"]))
        }
    }

    /// 测试/无障碍路径模拟 native SwiftTerm 的滚轮；生产滚轮直接由
    /// `TerminalView.scrollWheel` 处理。这里绝不能再喂 PaneBuf dump。
    func scrollPaneHistory(paneId: UInt32, deltaLines: Int) {
        guard let view = views[paneId] else { return }
        view.scrollLines(deltaLines)
    }

    /// 把 core viewport 映射到 SwiftTerm 原生 scrollback。只改变 yDisp，
    /// 不 reset、不 feed snapshot，因此不会破坏 live VT 状态。
    func applyViewport(paneId: UInt32, offset: UInt32) {
        let view = view(for: paneId)
        let rows = UInt32(max(1, expectedPaneSizes[paneId]?.rows ?? view.getTerminal().rows))
        let rawMax = bridge?.paneHistoryMaxOffset(paneId: paneId, rows: rows) ?? -1
        let maxOffset = rawMax < 0 ? 0 : UInt32(rawMax)
        _ = bridge?.setPaneViewport(paneId: paneId, offset: offset)
        applyingNativeScroll.insert(paneId)
        view.scrollToHistoryOffset(offset, maxOffset: maxOffset)
        applyingNativeScroll.remove(paneId)
        if offset == 0 {
            unseenLines[paneId] = 0
            onUnseenLinesChanged?(paneId, 0)
        }
        onViewportChanged?(paneId, offset)
    }

    func scrollToLatest(paneId: UInt32) {
        applyingNativeScroll.insert(paneId)
        views[paneId]?.scrollToLatest()
        applyingNativeScroll.remove(paneId)
        unseenLines[paneId] = 0
        onUnseenLinesChanged?(paneId, 0)
        _ = bridge?.setPaneViewport(paneId: paneId, offset: 0)
        onViewportChanged?(paneId, 0)
    }

    func unseenLineCount(paneId: UInt32) -> UInt32 {
        unseenLines[paneId] ?? 0
    }

    private func handleNativeScroll(paneId: UInt32, position: Double) {
        guard !applyingNativeScroll.contains(paneId) else { return }
        let rows = UInt32(max(1, expectedPaneSizes[paneId]?.rows ?? 24))
        let rawMax = bridge?.paneHistoryMaxOffset(paneId: paneId, rows: rows) ?? -1
        let maxOffset = rawMax < 0 ? 0 : UInt32(rawMax)
        let clamped = min(max(position, 0), 1)
        let offset = clamped >= 0.999 || maxOffset == 0
            ? 0
            : UInt32((Double(maxOffset) * (1 - clamped)).rounded())
        _ = bridge?.setPaneViewport(paneId: paneId, offset: offset)
        if offset == 0 {
            unseenLines[paneId] = 0
            onUnseenLinesChanged?(paneId, 0)
        }
        onViewportChanged?(paneId, offset)
    }

    /// 给窗口级快捷键监视器发送已经编码好的终端控制字节。
    func sendRawInput(to view: MuxTerminalView, byte: UInt8) {
        if bridge?.sendInput(paneId: view.paneId, data: Data([byte])) != 0 {
            onError?(MuxtermI18n.shared.tr(.errorSendControl, arguments: ["id": "\(view.paneId)"]))
        }
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {
        guard cols >= 2, rows >= 1, cols < 10000, rows < 10000 else { return }
        // tmux pane 的真实尺寸由 refresh-client -C 和 tmux layout 决定；
        // 这里不能把每个 SwiftTerm view 的尺寸再写回 tmux。
        guard !usesClientResize else { return }
        let c = UInt16(cols)
        let r = UInt16(rows)
        if let prev = lastPtySize[view.paneId], prev.0 == c, prev.1 == r {
            return
        }
        lastPtySize[view.paneId] = (c, r)
        guard let bridge else { return }
        if bridge.resizePane(paneId: view.paneId, cols: c, rows: r) != 0,
           reportedResizeFailures.insert(view.paneId).inserted
        {
            onError?(MuxtermI18n.shared.tr(.errorResizePane, arguments: ["id": "\(view.paneId)"]))
        }
    }

    private func appendSnippet(_ data: Data) {
        guard let text = String(data: data, encoding: .utf8), !text.isEmpty else { return }
        recentOutputSnippet += text
        if recentOutputSnippet.count > 400 {
            recentOutputSnippet = String(recentOutputSnippet.suffix(400))
        }
        onOutputSnippetChanged?(recentOutputSnippet)
    }

    /// 记录流量统计：在 2 秒滑动窗口内计算下行速率（bytes/s）。
    /// 供 statusbar 的 SSH Traffic Monitor 实时显示。
    func recordTraffic(bytes: Int) {
        guard bytes > 0 else { return }
        totalBytesReceived &+= UInt64(bytes)
        let now = Date().timeIntervalSince1970
        trafficTimestamps.append(now)
        trafficByteCounts.append(UInt64(bytes))
        // 淘汰 2 秒窗口外的样本
        let cutoff = now - 2.0
        while !trafficTimestamps.isEmpty, trafficTimestamps[0] < cutoff {
            trafficTimestamps.removeFirst()
            trafficByteCounts.removeFirst()
        }
        let windowBytes = trafficByteCounts.reduce(0, &+)
        trafficRate = windowBytes / 2
    }

    /// SSH 连接状态摘要（供 statusbar 显示）。
    /// 返回 backend 类型 + alias/session + 连接状态。
    var connectionSummary: (type: String, host: String?, status: String) {
        let bt = bridge?.backendType ?? "unknown"
        let host: String?
        switch bt {
        case "ssh":
            host = bridge?.sshAlias ?? bridge?.socket
        case "tmux":
            host = bridge?.session
        case "local":
            host = nil
        default:
            host = bridge?.session
        }
        let statusLabel: String
        switch bridge?.lastStatus {
        case 0: statusLabel = "disconnected"
        case 1: statusLabel = "connecting"
        case 2: statusLabel = "connected"
        case 3: statusLabel = "exited"
        default: statusLabel = "unknown"
        }
        return (bt, host, statusLabel)
    }
}
