//! GUI 布局前的纯一致性检查与状态事件渲染策略。
//!
//! 后端切 tab / split 是异步的；过渡快照里 layout 与 panes 可能来自不同
//! 版本，不能把旧 tab 的叶子拿来渲染新 tab。后台 tab 的 layout/pane 事件
//! 不应触发当前 tab 重建（否则前台 htop 会被反复重绘）。

/// 布局树叶子与 panes 快照的一致性检查。
pub enum PaneLayoutProjection {}

impl PaneLayoutProjection {
    pub fn accepts(tree_pane_ids: &[u32], pane_ids: &[u32]) -> bool {
        tree_pane_ids.len() == pane_ids.len()
            && tree_pane_ids.iter().copied().collect::<std::collections::HashSet<_>>()
                == pane_ids.iter().copied().collect::<std::collections::HashSet<_>>()
    }
}

/// 当前 pane 全屏的纯布局策略（本地 shell 用；tmux 走 `resize-pane -Z`）。
pub enum PaneFullscreenPolicy {}

impl PaneFullscreenPolicy {
    pub fn resolved_fullscreen_id(fullscreen_pane_id: Option<u32>, pane_ids: &[u32]) -> Option<u32> {
        fullscreen_pane_id.filter(|id| pane_ids.contains(id))
    }
}

/// 状态事件常量（与 core ffi types 对齐）。
pub mod state_types {
    pub const STATE_PANE_OUTPUT: u32 = 0;
    pub const STATE_TAB_ADDED: u32 = 1;
    pub const STATE_TAB_CLOSED: u32 = 2;
    pub const STATE_LAYOUT_CHANGED: u32 = 3;
    pub const STATE_PANE_ADDED: u32 = 4;
    pub const STATE_PANE_CLOSED: u32 = 5;
    pub const STATE_ACTIVE_TAB_CHANGED: u32 = 6;
    pub const STATE_ACTIVE_PANE_CHANGED: u32 = 7;
}

/// FFI 状态事件的渲染策略。
pub enum StateEventPolicy {}

impl StateEventPolicy {
    /// 该事件类型是否要求重新加载布局。
    pub fn requires_layout_reload(type_: u32) -> bool {
        matches!(type_, 1..=6)
    }

    /// 是否需要重建当前 UI 布局：只有当前 tab 的结构事件才触发。
    pub fn should_reload_ui(type_: u32, tab_id: u32, active_tab_id: u32) -> bool {
        match type_ {
            // tab add/close、active tab changed：总是重建
            1 | 2 | 6 => true,
            // layout / pane add / pane close：只看当前 tab
            3 | 4 | 5 => tab_id == active_tab_id,
            _ => false,
        }
    }

    pub fn changes_active_pane(type_: u32) -> bool {
        type_ == 7
    }
}

/// 一批事件里是否存在结构/布局类事件。
pub enum EventBatchPlan {}

impl EventBatchPlan {
    pub fn has_structural_event(
        types: &[u32],
        requires_layout_reload: impl Fn(u32) -> bool,
    ) -> bool {
        types.iter().any(|t| requires_layout_reload(*t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_accepts_matching_sets() {
        assert!(PaneLayoutProjection::accepts(&[1, 2], &[2, 1]));
        assert!(!PaneLayoutProjection::accepts(&[1], &[1, 2]));
        assert!(!PaneLayoutProjection::accepts(&[1, 2], &[1, 3]));
    }

    #[test]
    fn fullscreen_id_resolves_only_existing() {
        assert_eq!(PaneFullscreenPolicy::resolved_fullscreen_id(Some(2), &[1, 2]), Some(2));
        assert_eq!(PaneFullscreenPolicy::resolved_fullscreen_id(Some(9), &[1, 2]), None);
        assert_eq!(PaneFullscreenPolicy::resolved_fullscreen_id(None, &[1, 2]), None);
    }

    #[test]
    fn background_tab_layout_events_ignored() {
        assert!(StateEventPolicy::should_reload_ui(3, 1, 1));
        assert!(!StateEventPolicy::should_reload_ui(3, 2, 1));
        assert!(StateEventPolicy::should_reload_ui(6, 2, 2));
    }

    #[test]
    fn structural_event_detection() {
        assert!(EventBatchPlan::has_structural_event(&[0, 3], StateEventPolicy::requires_layout_reload));
        assert!(!EventBatchPlan::has_structural_event(&[0, 7, 8], StateEventPolicy::requires_layout_reload));
    }
}
