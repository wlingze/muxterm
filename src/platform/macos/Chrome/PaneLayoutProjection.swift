import Foundation

/// GUI 应用布局前对 pane 叶子做的纯一致性检查。
///
/// 后端切 tab、split 是异步的；在过渡快照里，layout 和 panes 可能暂时来自
/// 不同版本。此时不能把旧 tab 的叶子拿来渲染新 tab。
public enum PaneLayoutProjection {
    public static func accepts(treePaneIDs: [UInt32], paneIDs: [UInt32]) -> Bool {
        guard treePaneIDs.count == paneIDs.count else { return false }
        return Set(treePaneIDs) == Set(paneIDs)
    }
}

/// FFI 状态事件的渲染策略，和 `muxterm.h` 的常量保持一致。
public enum StateEventPolicy {
    public static func requiresLayoutReload(_ type: UInt32) -> Bool {
        switch type {
        case 1, 2, 3, 4, 5, 6: // tab add/close, layout, pane add/close, active tab
            return true
        default:
            return false
        }
    }

    public static func changesActivePane(_ type: UInt32) -> Bool {
        type == 7
    }
}
