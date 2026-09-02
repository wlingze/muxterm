import Foundation

/// GUI 应用布局前对 pane 叶子做的纯一致性检查。
///
/// 后端切 tab、split 是异步的；在过渡快照里，layout 和 panes 可能暂时来自
/// 不同版本。此时不能把旧 tab 的叶子拿来渲染新 tab。
public enum PaneLayoutProjection {
    public static func accepts(treePaneIDs: [UInt32], paneIDs: [UInt32]) -> Bool {
        if treePaneIDs.count == paneIDs.count {
            return Set(treePaneIDs) == Set(paneIDs)
        }
        // tmux zoom（resize-pane -Z）：布局塌成单叶，pane 快照仍保留全部 pane。
        if treePaneIDs.count == 1, paneIDs.count > 1, let id = treePaneIDs.first, paneIDs.contains(id) {
            return true
        }
        return false
    }
}

/// 当前 pane 全屏的纯布局策略（本地 shell 用；tmux 走 `resize-pane -Z`）。
public enum PaneFullscreenPolicy {
    /// 识别 tmux `resize-pane -Z` 造成的单叶投影。
    ///
    /// 本地 shell 的全屏目标由 `PaneLayoutView` 保存；这里只处理 Core
    /// 快照中“布局单叶、pane 快照多叶”的 tmux/控制模式形状。
    public static func zoomedPaneID(
        layoutPaneIDs: [UInt32],
        paneIDs: [UInt32]
    ) -> UInt32? {
        guard layoutPaneIDs.count == 1,
              paneIDs.count > 1,
              let paneId = layoutPaneIDs.first,
              paneIDs.contains(paneId)
        else {
            return nil
        }
        return paneId
    }

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
        case 1, 2, 3, 4, 5: // tab add/close, layout, pane add/close
            return true
        default:
            // 6 = active tab changed：只挂缓存树，同批 PaneOutput 不必推迟。
            return false
        }
    }

    /// 是否需要重建当前 UI 布局。
    ///
    /// 后台 tab 的 layout/pane 事件（例如其它 tab 的 codex 刷新引起 tmux
    /// 对每个 window 发 %layout-change）不应触发当前 tab 重建/forceRedraw，
    /// 否则前台 htop 会被其它 tab 的刷新反复重绘（闪烁/乱屏）。
    public static func shouldReloadUI(
        type: UInt32,
        tabId: UInt32,
        activeTabId: UInt32
    ) -> Bool {
        switch type {
        case 1, 2: // tab add/close：总是处理
            return true
        case 6: // active tab：缓存命中时 MainWindow 走轻量路径，不重建
            return true
        case 3, 4, 5: // layout / pane add / pane close：只看当前 tab
            return tabId == activeTabId
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
