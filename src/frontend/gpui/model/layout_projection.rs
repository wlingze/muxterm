//! 布局一致性、全屏识别、事件策略、自动切分方向、拖拽与移动菜单。
//!
//! 移植自 `chrome/PaneLayoutProjection.swift`。原文件里有七个 enum，这里
//! 合并成五个模块级函数组，语义一一对应。

/// 布局树是否属于当前 tab 的 pane 快照。
///
/// 后端切 tab、split 是异步的；过渡快照里 layout 和 panes 可能来自不同版本，
/// 此时不能把旧 tab 的叶子拿来渲染新 tab。
///
/// 两种形状接受：
/// 1. 数量相等且集合相等（顺序无关）；
/// 2. 单叶且该叶子在多叶快照里 —— tmux `resize-pane -Z` 的 zoom 形状。
pub fn accepts(tree_pane_ids: &[u32], pane_ids: &[u32]) -> bool {
    if tree_pane_ids.len() == pane_ids.len() {
        let mut a = tree_pane_ids.to_vec();
        let mut b = pane_ids.to_vec();
        a.sort_unstable();
        b.sort_unstable();
        return a == b;
    }
    if let [only] = tree_pane_ids {
        return pane_ids.len() > 1 && pane_ids.contains(only);
    }
    false
}

/// 识别 tmux `resize-pane -Z` 造成的单叶投影，返回被全屏的 pane。
pub fn zoomed_pane_id(layout_pane_ids: &[u32], pane_ids: &[u32]) -> Option<u32> {
    if let [only] = layout_pane_ids {
        if pane_ids.len() > 1 && pane_ids.contains(only) {
            return Some(*only);
        }
    }
    None
}

/// 返回应当全屏的 pane id；目标不存在时返回 `None`（保持原布局）。
pub fn resolved_fullscreen_id(fullscreen_pane_id: Option<u32>, pane_ids: &[u32]) -> Option<u32> {
    fullscreen_pane_id.filter(|id| pane_ids.contains(id))
}

/// 该 FFI 状态事件是否需要重载布局。
///
/// 类型常量与 `muxterm.h` 保持一致：1/2 = tab add/close，3/4/5 = layout /
/// pane add / pane close，6 = active tab。6 只挂缓存树，同批 `PaneOutput`
/// 不必推迟。
pub fn requires_layout_reload(event_type: u32) -> bool {
    matches!(event_type, 1..=5)
}

/// 是否需要重建当前 UI 布局。
///
/// 后台 tab 的 layout/pane 事件（其它 tab 的 codex 刷新会让 tmux 对每个
/// window 发 `%layout-change`）不应触发当前 tab 重建，否则前台 htop 会被
/// 反复重绘（闪烁/乱屏）。
pub fn should_reload_ui(event_type: u32, tab_id: u32, active_tab_id: u32) -> bool {
    match event_type {
        1 | 2 | 6 => true,
        // layout / pane add / pane close：只看当前 tab。
        3..=5 => tab_id == active_tab_id,
        _ => false,
    }
}

/// 该事件是否改变 active pane（类型 7）。
pub fn changes_active_pane(event_type: u32) -> bool {
    event_type == 7
}

/// 默认字高 ≈ 字宽 × 2。
pub const DEFAULT_CELL_HEIGHT_OVER_WIDTH: f64 = 2.0;

/// 按当前 pane 的**像素**长边决定切分方向：返回 true 表示左右切。
///
/// tmux 社区脚本用 `pane_width > pane_height * 2`，因为字符格大约高是宽的
/// 两倍。这里用像素比，半屏竖条会上下切，宽屏会左右切。
pub fn auto_split_horizontal(cols: i32, rows: i32, cell_width: f64, cell_height: f64) -> bool {
    let width = f64::from(cols.max(1)) * cell_width.max(1.0);
    let height = f64::from(rows.max(1)) * cell_height.max(1.0);
    width >= height
}

/// 拖标题栏改布局的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAction {
    /// 落在另一个 pane 上：交换两者。
    Swap(u32),
    /// 落在空白处：拆到新 tab。
    MoveToNewTab,
    /// 落在自己身上：取消。
    Cancel,
}

/// 落到自己取消，落到另一个 pane 交换，落到空白处移去新 tab。
pub fn drag_action(source: u32, destination: Option<u32>) -> DragAction {
    match destination {
        Some(pane) if pane == source => DragAction::Cancel,
        Some(pane) => DragAction::Swap(pane),
        None => DragAction::MoveToNewTab,
    }
}

/// 标题栏「移动」菜单：最上面固定「新建 tab」（`None`），其余是其它 tab。
///
/// 单 pane tab 没有可移动对象，返回空菜单。
pub fn move_menu_destinations(
    tab_ids: &[u32],
    current_tab_id: u32,
    pane_count_in_current_tab: usize,
) -> Vec<Option<u32>> {
    if pane_count_in_current_tab <= 1 {
        return Vec::new();
    }
    let mut items = vec![None];
    items.extend(
        tab_ids
            .iter()
            .copied()
            .filter(|id| *id != current_tab_id)
            .map(Some),
    );
    items
}

/// 一批事件里是否存在结构/布局类事件。
///
/// 存在时，同批的 `PaneOutput` 必须延迟到模型尺寸同步之后再喂入，否则
/// htop/codex 的新尺寸重绘帧会先进旧尺寸模型（拖窗口乱屏）。
pub fn has_structural_event(event_types: &[u32]) -> bool {
    event_types.iter().copied().any(requires_layout_reload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `PaneLayoutProjectionTests.testLayoutMustContainExactlyCurrentTabPanes`。
    #[test]
    fn layout_must_contain_exactly_current_tab_panes() {
        assert!(accepts(&[11, 13, 12], &[11, 12, 13]));
        assert!(!accepts(&[11, 12, 13], &[21]));
        assert!(!accepts(&[11, 12], &[11, 12, 13]));
        assert!(!accepts(&[0], &[11]));
        assert!(accepts(&[0], &[0]));
        // zoom：单叶覆盖多 pane 快照。
        assert!(accepts(&[11], &[11, 12, 13]));
    }

    /// 对照 `testOnlyStructuralEventsRebuildLayout` 与 `changesActivePane`。
    #[test]
    fn only_structural_events_rebuild_layout() {
        for event_type in 1..=5u32 {
            assert!(
                requires_layout_reload(event_type),
                "event={event_type} 应当重载布局"
            );
        }
        assert!(!requires_layout_reload(6));
        assert!(!requires_layout_reload(7));
        assert!(changes_active_pane(7));
        assert!(!changes_active_pane(3));
    }

    /// 对照 `shouldReloadUI` 的六条断言：tab 事件全局生效，layout/pane
    /// 事件只看当前 tab。
    #[test]
    fn background_tab_layout_events_do_not_rebuild_foreground() {
        assert!(should_reload_ui(3, 2, 2));
        assert!(should_reload_ui(4, 2, 2));
        assert!(!should_reload_ui(3, 5, 2));
        assert!(!should_reload_ui(4, 5, 2));
        assert!(!should_reload_ui(5, 5, 2));
        assert!(should_reload_ui(1, 5, 2));
        assert!(should_reload_ui(6, 5, 2));
        assert!(!should_reload_ui(99, 5, 2));
    }

    /// 对照 `testTmuxZoomIdentifiesTheVisiblePaneAgainstFullSnapshot`。
    #[test]
    fn tmux_zoom_identifies_visible_pane_against_full_snapshot() {
        assert_eq!(zoomed_pane_id(&[61], &[61, 62]), Some(61));
        assert_eq!(zoomed_pane_id(&[61, 62], &[61, 62]), None);
        assert_eq!(zoomed_pane_id(&[63], &[61, 62]), None);
        assert_eq!(zoomed_pane_id(&[61], &[61]), None);
    }

    /// 对照 `resolvedFullscreenId` 的三条断言。
    #[test]
    fn fullscreen_target_must_still_exist() {
        assert_eq!(resolved_fullscreen_id(None, &[1, 2]), None);
        assert_eq!(resolved_fullscreen_id(Some(2), &[1, 2]), Some(2));
        assert_eq!(resolved_fullscreen_id(Some(9), &[1]), None);
    }

    /// 对照 `FlatChromeTests.testAutoSplitFollowsPixelAspectNotCellCount`。
    #[test]
    fn auto_split_follows_pixel_aspect_not_cell_count() {
        let h = |cols, rows| auto_split_horizontal(cols, rows, 1.0, DEFAULT_CELL_HEIGHT_OVER_WIDTH);
        assert!(h(178, 50));
        assert!(!h(88, 49));
        assert!(h(80, 24));
        assert!(!h(40, 80));
        assert!(!h(24, 24), "正方形格子数在真实字高下仍是竖条");
        assert!(
            auto_split_horizontal(88, 49, 11.0, 11.0),
            "正方形像素格按宽边左右切"
        );
    }

    #[test]
    fn auto_split_treats_non_positive_bounds_as_one() {
        assert!(!auto_split_horizontal(0, 0, 1.0, 2.0));
        assert!(!auto_split_horizontal(1, 0, 1.0, 2.0));
    }

    /// 对照 `PaneDragLayoutPolicyTests`。
    #[test]
    fn drag_swaps_or_moves_or_cancels() {
        assert_eq!(drag_action(1, Some(2)), DragAction::Swap(2));
        assert_eq!(drag_action(1, Some(1)), DragAction::Cancel);
        assert_eq!(drag_action(1, None), DragAction::MoveToNewTab);
    }

    /// 对照 `testMoveMenuPinsNewTabFirstAndSkipsCurrentTab`。
    #[test]
    fn move_menu_pins_new_tab_first_and_skips_current_tab() {
        assert_eq!(
            move_menu_destinations(&[1, 2, 3], 2, 1),
            Vec::new(),
            "单 pane tab 不能移动"
        );
        assert_eq!(
            move_menu_destinations(&[1, 2, 3], 2, 2),
            vec![None, Some(1), Some(3)],
            "最上面固定新建，其余是其它 tab"
        );
    }

    /// 对照 `EventBatchPlanTests`：1/2 结构、6 非结构。
    #[test]
    fn batch_is_structural_when_any_event_needs_reload() {
        assert!(has_structural_event(&[1]));
        assert!(has_structural_event(&[2]));
        assert!(!has_structural_event(&[6]));
        assert!(!has_structural_event(&[6, 7]));
        assert!(!has_structural_event(&[]));
    }
}
