import Foundation

/// 当前 tab 内 pane 的循环导航纯逻辑。
///
/// macOS 前端不要把 Cmd+[ / Cmd+] 交给全局 active pane 推断；
/// 当前 tab 的布局叶子才是唯一有效的导航范围。
public enum PaneNavigation {
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
