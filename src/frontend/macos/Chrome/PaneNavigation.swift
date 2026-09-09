import Foundation

/// 当前 tab 内 pane 的循环导航纯逻辑。
///
/// macOS 前端不要把 Cmd+[ / Cmd+] 交给全局 active pane 推断；
/// 当前 tab 的布局叶子才是唯一有效的导航范围。
public enum PaneNavigation {
    /// 返回当前 tab 用于导航的 pane 顺序。
    ///
    /// 正常布局的叶子顺序对应屏幕上的几何顺序。tmux zoom 后布局树会
    /// 只保留被放大的一个叶子，但 pane 快照仍包含当前 tab 的全部 pane；
    /// 这时必须恢复完整快照顺序，否则 Cmd/Alt+[ ] 没有目标可切换。
    public static func navigationPaneIDs(
        layoutPaneIDs: [UInt32]?,
        paneIDs: [UInt32]
    ) -> [UInt32] {
        guard let layoutPaneIDs, !layoutPaneIDs.isEmpty else {
            return paneIDs
        }

        let layoutSet = Set(layoutPaneIDs)
        if layoutPaneIDs.count == paneIDs.count, layoutSet == Set(paneIDs) {
            return layoutPaneIDs
        }

        // tmux zoom：布局只剩一个叶子，而 snapshot 仍保留当前 tab 的全部
        // pane。只有这个形状才扩大导航范围，避免过渡快照串入旧布局顺序。
        if layoutPaneIDs.count == 1,
           paneIDs.count > 1,
           let visiblePane = layoutPaneIDs.first,
           paneIDs.contains(visiblePane)
        {
            return paneIDs
        }

        // 布局与 pane 快照处于异步过渡态时，快照 pane 集合仍属于当前 tab，
        // 用它保证快捷键不会卡死在旧布局的单一叶子上。
        return paneIDs
    }

    public static func target(
        paneIDs: [UInt32],
        activePaneID: UInt32,
        offset: Int
    ) -> UInt32? {
        guard !paneIDs.isEmpty else { return nil }
        guard let current = paneIDs.firstIndex(of: activePaneID) else {
            return paneIDs.first
        }
        let count = paneIDs.count
        let index = ((current + offset) % count + count) % count
        return paneIDs[index]
    }
}
