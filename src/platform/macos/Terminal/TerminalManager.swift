import AppKit
import MuxtermChrome

/// 管理多个 pane 对应的 `MuxTerminalView`，并把输出/输入接到 CoreBridge。
final class TerminalManager: TerminalInputHandler {
    private weak var bridge: CoreBridge?
    private var views: [UInt32: MuxTerminalView] = [:]
    /// 本轮 poll 批次内新建的视图：播种快照已覆盖该批次所有已入队的
    /// PaneOutput 事件，本批次剩余事件必须跳过，否则同一批字节会双写。
    private var viewsCreatedThisBatch = Set<UInt32>()
    /// 两次 poll 之间（批次外）新建的视图：其播种快照覆盖了队列里尚未派发的
    /// 事件，下一批开始时要把它们结转到 `viewsCreatedThisBatch` 继续抑制。
    private var pendingSeedPanes = Set<UInt32>()
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
    /// 同一 pane 的 resize 失败只报告一次，避免轮询/重绘时刷屏。
    private var reportedResizeFailures = Set<UInt32>()
    private var reportedClientResizeFailure = false

    weak var focusTarget: MuxTerminalView?
    var onOutputSnippetChanged: ((String) -> Void)?
    var onError: ((String) -> Void)?

    deinit {
        clientResizeWorkItem?.cancel()
    }

    init(bridge: CoreBridge) {
        self.bridge = bridge
    }

    /// 连接面板切换 local / SSH session 后更新桥接对象。
    func updateBridge(_ bridge: CoreBridge) {
        self.bridge = bridge
        for view in views.values {
            view.removeFromSuperview()
        }
        views.removeAll()
        viewsCreatedThisBatch.removeAll()
        pendingSeedPanes.removeAll()
        inEventBatch = false
        lastPtySize.removeAll()
        lastClientSize = nil
        pendingClientSize = nil
        clientResizeWorkItem?.cancel()
        clientResizeWorkItem = nil
        reportedResizeFailures.removeAll()
        reportedClientResizeFailure = false
        recentOutputSnippet = ""
        onOutputSnippetChanged?(recentOutputSnippet)
    }

    /// 获取或创建指定 pane 的终端视图。
    func view(for paneId: UInt32) -> MuxTerminalView {
        if let existing = views[paneId] {
            return existing
        }
        let view = MuxTerminalView(paneId: paneId)
        view.inputHandler = self
        // 非直接 PTY 终端模拟器（tmux 控制模式 / daemon 代理）下禁止把
        // SwiftTerm 解析 pane 输出时生成的查询应答回写 pane。
        view.suppressOutputDrivenResponses = !isDirectPtyTerminal
        views[paneId] = view
        // 首次创建时用最近快照播种（FFI 返回最近 256KB）。播种覆盖了后端已
        // 入队但尚未派发的事件，这些事件必须在接下来的批次里跳过。
        let snapshot = bridge?.getPaneOutput(paneId: paneId) ?? Data()
        if !snapshot.isEmpty {
            view.feedOutput(snapshot)
            appendSnippet(snapshot)
            if inEventBatch {
                viewsCreatedThisBatch.insert(paneId)
            } else {
                pendingSeedPanes.insert(paneId)
            }
        }
        return view
    }

    /// 处理 PaneOutput 增量事件。
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
        if PaneOutputFeedPolicy.shouldFeedEvent(
            viewExistedBeforeEvent: existed,
            seedCoveredEvent: viewsCreatedThisBatch.contains(paneId)
        ) {
            // 视图早已存在：事件就是增量；或视图刚创建但快照为空（新 pane
            // 首批字节，种子没有覆盖任何事件），必须原样喂入。
            view.feedOutput(data)
            appendSnippet(data)
        }
    }

    /// 每轮 poll 事件处理前调用，标记批次边界。
    func beginEventBatch() {
        inEventBatch = true
        // 批次外新建的视图：队列里已入队的事件都被其播种快照覆盖，本批抑制。
        viewsCreatedThisBatch = pendingSeedPanes
        pendingSeedPanes.removeAll()
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
        views[paneId]?.removeFromSuperview()
        views.removeValue(forKey: paneId)
        viewsCreatedThisBatch.remove(paneId)
        pendingSeedPanes.remove(paneId)
        lastPtySize.removeValue(forKey: paneId)
    }

    /// 当前连接是否由 tmux 控制 client 管理尺寸。
    var usesClientResize: Bool {
        bridge?.backendType == "tmux" || bridge?.backendType == "ssh"
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

    /// 把 GUI 根容器的 backing pixels 映射为 tmux client 字符格。
    private func syncClientSize(container: NSView, paneIds: Set<UInt32>) {
        guard let cell = cellSizeInPixels(paneIds: paneIds), cell.width > 0, cell.height > 0 else {
            return
        }
        let pixelSize = container.convertToBacking(container.bounds).size
        let cols = Int(floor(pixelSize.width / CGFloat(cell.width)))
        let rows = Int(floor(pixelSize.height / CGFloat(cell.height)))
        guard cols >= 2, rows >= 1, cols < 10000, rows < 10000 else { return }
        let size = (UInt16(cols), UInt16(rows))
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
        guard let cell = cellSizeInPixels(paneIds: paneIds), cell.width > 0, cell.height > 0 else {
            return
        }
        let pixelSize = container.convertToBacking(container.bounds).size
        let cols = Int(floor(pixelSize.width / CGFloat(cell.width)))
        let rows = Int(floor(pixelSize.height / CGFloat(cell.height)))
        guard cols >= 2, rows >= 1, cols < 10000, rows < 10000 else { return }
        let size = (UInt16(cols), UInt16(rows))
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

    // MARK: - TerminalInputHandler

    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        // 仅转发到 FFI；显示只走 pty 回显的 PaneOutput（修双写）。
        if bridge?.sendInput(paneId: view.paneId, data: Data(data)) != 0 {
            onError?(MuxtermI18n.shared.tr(.errorSendInput, arguments: ["id": "\(view.paneId)"]))
        }
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
}
