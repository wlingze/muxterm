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

/// 当前 pane 全屏的纯布局策略（本地 shell 用；tmux 走 `resize-pane -Z`）。
public enum PaneFullscreenPolicy {
    /// 返回应当全屏的 pane id；目标不存在时返回 nil（保持原布局）。
    public static func resolvedFullscreenId(
        fullscreenPaneId: UInt32?,
        paneIDs: [UInt32]
    ) -> UInt32? {
        guard let fullscreenPaneId, paneIDs.contains(fullscreenPaneId) else {
            return nil
        }
        return fullscreenPaneId
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

/// 一批事件里是否存在结构/布局类事件。
///
/// 存在时，同一批的 PaneOutput 必须延迟到 `refreshUI` 完成模型尺寸同步后
/// 再喂入，否则 htop/codex 的新尺寸重绘帧会先进旧尺寸模型（拖窗口乱屏）。
public enum EventBatchPlan {
    public static func hasStructuralEvent(
        types: [UInt32],
        requiresLayoutReload: (UInt32) -> Bool
    ) -> Bool {
        types.contains { requiresLayoutReload($0) }
    }
}
