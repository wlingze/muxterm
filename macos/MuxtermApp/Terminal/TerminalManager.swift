import AppKit

/// 管理多个 pane 对应的 `MuxTerminalView`，并把输出/输入接到 CoreBridge。
final class TerminalManager: TerminalInputHandler {
    private weak var bridge: CoreBridge?
    private var views: [UInt32: MuxTerminalView] = [:]
    /// 已喂给终端的累计输出长度（按 pane），避免 snapshot 全量重复 feed。
    private var fedLengths: [UInt32: Int] = [:]

    weak var focusTarget: MuxTerminalView?

    init(bridge: CoreBridge) {
        self.bridge = bridge
    }

    /// 获取或创建指定 pane 的终端视图。
    func view(for paneId: UInt32) -> MuxTerminalView {
        if let existing = views[paneId] {
            return existing
        }
        let view = MuxTerminalView(paneId: paneId)
        view.inputHandler = self
        views[paneId] = view
        // 首次创建时拉取历史输出
        if let data = bridge?.getPaneOutput(paneId: paneId), !data.isEmpty {
            view.feedOutput(data)
            fedLengths[paneId] = data.count
        }
        return view
    }

    /// 处理 PaneOutput 增量事件。
    func handleOutput(paneId: UInt32, data: Data) {
        guard !data.isEmpty else { return }
        let view = view(for: paneId)
        view.feedOutput(data)
        fedLengths[paneId, default: 0] += data.count
    }

    /// 丢弃已关闭 pane 的视图。
    func retainOnly(paneIds: Set<UInt32>) {
        let obsolete = views.keys.filter { !paneIds.contains($0) }
        for id in obsolete {
            views[id]?.removeFromSuperview()
            views.removeValue(forKey: id)
            fedLengths.removeValue(forKey: id)
        }
    }

    // MARK: - TerminalInputHandler

    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) {
        bridge?.sendInput(paneId: view.paneId, data: Data(data))
    }

    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {
        // 当前 FFI 头未导出 muxterm_resize；尺寸变化暂记日志，待核心补齐。
        _ = (cols, rows)
    }
}
