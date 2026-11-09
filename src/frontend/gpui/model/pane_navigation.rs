//! 当前 tab 内的 pane 循环导航。
//!
//! 移植自 `chrome/PaneNavigation.swift`（对照 `PaneNavigationTests`）。
//!
//! `Cmd-[` / `Cmd-]` 不能交给 Core 的全局 active pane 推断：当前 tab 的
//! 布局叶子才是唯一有效的导航范围，否则分屏后按快捷键会跳到别的 tab 上
//! 的 pane，或者干脆没有目标。

use crate::frontend::utils::corebridge::ClientLayout;

/// 布局树的叶子 pane，按几何顺序（左上 → 右下）排列。
pub fn layout_leaf_ids(layout: &ClientLayout) -> Vec<u32> {
    match layout {
        ClientLayout::Leaf { pane_id } => vec![*pane_id],
        ClientLayout::Split { first, second, .. } => {
            let mut ids = layout_leaf_ids(first);
            ids.extend(layout_leaf_ids(second));
            ids
        }
    }
}

/// 返回当前 tab 用于导航的 pane 顺序。
///
/// 正常布局的叶子顺序对应屏幕上的几何顺序。tmux zoom 后布局树会只保留
/// 被放大的一个叶子，但 pane 快照仍包含当前 tab 的全部 pane；这时必须
/// 恢复完整快照顺序，否则 `Cmd-[` / `Cmd-]` 没有目标可切换。
pub fn navigation_pane_ids(layout_pane_ids: Option<&[u32]>, pane_ids: &[u32]) -> Vec<u32> {
    let Some(layout_pane_ids) = layout_pane_ids.filter(|ids| !ids.is_empty()) else {
        return pane_ids.to_vec();
    };
    if layout_pane_ids.len() == pane_ids.len()
        && layout_pane_ids.iter().all(|id| pane_ids.contains(id))
    {
        return layout_pane_ids.to_vec();
    }
    // tmux zoom：布局只剩一个叶子，而快照仍保留当前 tab 的全部 pane。
    // 只有这个形状才扩大导航范围，避免过渡快照串入旧布局顺序。
    if let [visible] = layout_pane_ids {
        if pane_ids.len() > 1 && pane_ids.contains(visible) {
            return pane_ids.to_vec();
        }
    }
    // 布局与 pane 快照处于异步过渡态时，快照 pane 集合仍属于当前 tab，
    // 用它保证快捷键不会卡死在旧布局的单一叶子上。
    pane_ids.to_vec()
}

/// 循环取相对 `active_pane_id` 偏移 `offset` 的 pane。
///
/// 当前 pane 不在列表里时直接回退到第一个（不对它再加偏移）；
/// 空列表返回 `None`。
pub fn target(pane_ids: &[u32], active_pane_id: u32, offset: i32) -> Option<u32> {
    let Some(current) = pane_ids.iter().position(|id| *id == active_pane_id) else {
        return pane_ids.first().copied();
    };
    let count = pane_ids.len() as i32;
    let index = ((current as i32 + offset) % count + count) % count;
    pane_ids.get(index as usize).copied()
}

/// 布局是否处于 tmux zoom（单叶但快照里有多个 pane）。
pub fn zoomed_pane_id(layout_pane_ids: &[u32], pane_ids: &[u32]) -> Option<u32> {
    crate::frontend::gpui::model::layout_projection::zoomed_pane_id(layout_pane_ids, pane_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `PaneNavigationTests.testNextAndPreviousStayInsideCurrentTab`。
    #[test]
    fn next_and_previous_stay_inside_the_current_tab() {
        let panes = [11, 12, 13];
        assert_eq!(target(&panes, 12, 1), Some(13));
        assert_eq!(target(&panes, 12, -1), Some(11));
    }

    /// 对照 `PaneNavigationTests.testNavigationWraps`。
    #[test]
    fn navigation_wraps_around_both_ends() {
        let panes = [21, 22];
        assert_eq!(target(&panes, 22, 1), Some(21));
        assert_eq!(target(&panes, 21, -1), Some(22));
    }

    /// 对照 `PaneNavigationTests.testMissingActivePaneFallsBackToFirstPane`。
    #[test]
    fn missing_active_pane_falls_back_to_the_first_pane() {
        assert_eq!(target(&[31, 32], 99, 1), Some(31));
        assert_eq!(target(&[], 0, 1), None);
    }

    /// 对照 `PaneNavigationTests.testZoomedLayoutNavigatesAcrossAllPanesInTheCurrentTab`。
    #[test]
    fn zoomed_layout_navigates_across_all_panes_in_the_current_tab() {
        let layout = [41];
        let panes = [41, 42, 43];
        assert_eq!(navigation_pane_ids(Some(&layout), &panes), panes.to_vec());
        assert_eq!(
            target(&navigation_pane_ids(Some(&layout), &panes), 41, 1),
            Some(42)
        );
        assert_eq!(zoomed_pane_id(&layout, &panes), Some(41));
    }

    /// 对照 `PaneNavigationTests.testNonZoomedLayoutKeepsItsGeometryOrder`。
    #[test]
    fn non_zoomed_layout_keeps_its_geometry_order() {
        let layout = [52, 51, 53];
        let panes = [51, 52, 53];
        assert_eq!(navigation_pane_ids(Some(&layout), &panes), layout.to_vec());
        assert_eq!(zoomed_pane_id(&layout, &panes), None);
    }

    /// 布局叶子按几何顺序递归展开，导航顺序跟着屏幕走。
    #[test]
    fn layout_leaves_flatten_in_geometry_order() {
        let layout = ClientLayout::Split {
            horizontal: true,
            ratio: 50,
            first: Box::new(ClientLayout::Leaf { pane_id: 7 }),
            second: Box::new(ClientLayout::Split {
                horizontal: false,
                ratio: 50,
                first: Box::new(ClientLayout::Leaf { pane_id: 8 }),
                second: Box::new(ClientLayout::Leaf { pane_id: 9 }),
            }),
        };
        assert_eq!(layout_leaf_ids(&layout), vec![7, 8, 9]);
    }

    /// 没有布局（过渡快照）时按快照顺序导航。
    #[test]
    fn missing_layout_falls_back_to_the_snapshot_order() {
        assert_eq!(navigation_pane_ids(None, &[5, 6]), vec![5, 6]);
        assert_eq!(navigation_pane_ids(Some(&[]), &[5, 6]), vec![5, 6]);
    }
}
