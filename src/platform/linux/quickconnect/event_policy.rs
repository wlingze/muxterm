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
            && tree_pane_ids
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                == pane_ids
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
    }
}

/// 当前 pane 全屏的纯布局策略（本地 shell 用；tmux 走 `resize-pane -Z`）。
pub enum PaneFullscreenPolicy {}

impl PaneFullscreenPolicy {
    pub fn resolved_fullscreen_id(
        fullscreen_pane_id: Option<u32>,
        pane_ids: &[u32],
    ) -> Option<u32> {
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
    pub const STATE_PANE_RESIZED: u32 = 9;
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
            3..=5 => tab_id == active_tab_id,
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

    /// 同一批里若有布局/尺寸变化，PaneOutput 必须等网格对齐后再喂。
    /// 否则 htop/codex 按新列数生成的 CUP 会画进旧宽度（折行、表头叠表）。
    pub fn defer_output(types: &[u32]) -> bool {
        types.iter().any(|&t| {
            StateEventPolicy::requires_layout_reload(t) || t == state_types::STATE_PANE_RESIZED
        })
    }

    /// 先处理非输出事件（布局、resize），再处理 `%output`。
    pub fn partition(types: &[u32]) -> (Vec<usize>, Vec<usize>) {
        let defer = Self::defer_output(types);
        let mut now = Vec::new();
        let mut later = Vec::new();
        for (i, t) in types.iter().enumerate() {
            if defer && *t == state_types::STATE_PANE_OUTPUT {
                later.push(i);
            } else {
                now.push(i);
            }
        }
        (now, later)
    }
}

/// 用 VTE 实际列数驱动 `refresh-client -C`，避免 root 像素/字宽算出
/// 比 widget 更宽的 client（htop CUP 画到 VTE 右缘之外再折行）。
pub enum ClientSizePolicy {}

impl ClientSizePolicy {
    /// `multi_pane` 为 true 时不能用 active pane 的 VTE 列数当 client 宽度：
    /// 那是整个 tmux window 的宽度，多 pane 下 active pane 只占一部分，
    /// 用它驱动 `refresh-client -C` 会让 tmux 把整个 client 缩到单 pane 宽
    /// （1820 白屏的 resize 反馈环）。单 pane 仍优先 VTE 实际列数（2310.log）。
    pub fn cols(
        vte_cols: i64,
        allocated: bool,
        root_w: u64,
        cell_w: i64,
        multi_pane: bool,
    ) -> Option<u16> {
        if !multi_pane && allocated && vte_cols >= 2 {
            return Some(vte_cols.clamp(2, u16::MAX as i64) as u16);
        }
        if cell_w <= 0 || root_w == 0 {
            return None;
        }
        Some((root_w / cell_w as u64).clamp(2, u16::MAX as u64) as u16)
    }

    pub fn rows(root_h: u64, cell_h: i64) -> Option<u16> {
        if cell_h <= 0 || root_h == 0 {
            return None;
        }
        Some((root_h / cell_h as u64).clamp(1, u16::MAX as u64) as u16)
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
        assert_eq!(
            PaneFullscreenPolicy::resolved_fullscreen_id(Some(2), &[1, 2]),
            Some(2)
        );
        assert_eq!(
            PaneFullscreenPolicy::resolved_fullscreen_id(Some(9), &[1, 2]),
            None
        );
        assert_eq!(
            PaneFullscreenPolicy::resolved_fullscreen_id(None, &[1, 2]),
            None
        );
    }

    #[test]
    fn background_tab_layout_events_ignored() {
        assert!(StateEventPolicy::should_reload_ui(3, 1, 1));
        assert!(!StateEventPolicy::should_reload_ui(3, 2, 1));
        assert!(StateEventPolicy::should_reload_ui(6, 2, 2));
    }

    #[test]
    fn structural_event_detection() {
        assert!(EventBatchPlan::has_structural_event(
            &[0, 3],
            StateEventPolicy::requires_layout_reload
        ));
        assert!(!EventBatchPlan::has_structural_event(
            &[0, 7, 8],
            StateEventPolicy::requires_layout_reload
        ));
    }

    #[test]
    fn htop_output_deferred_until_after_layout_and_resize() {
        use state_types::{STATE_LAYOUT_CHANGED, STATE_PANE_OUTPUT, STATE_PANE_RESIZED};
        // 2219/2144：%layout-change + PaneResized + htop %output 同一批。
        let types = [STATE_LAYOUT_CHANGED, STATE_PANE_OUTPUT, STATE_PANE_RESIZED];
        assert!(EventBatchPlan::defer_output(&types));
        let (now, later) = EventBatchPlan::partition(&types);
        assert_eq!(now, vec![0, 2], "布局和 resize 必须先于输出");
        assert_eq!(later, vec![1]);
        // 同一批里 output 写在 resize 前面时，仍要先 resize。
        let types = [STATE_PANE_OUTPUT, STATE_PANE_RESIZED];
        let (now, later) = EventBatchPlan::partition(&types);
        assert_eq!(now, vec![1]);
        assert_eq!(later, vec![0]);
        assert!(!EventBatchPlan::defer_output(&[STATE_PANE_OUTPUT]));
        let (now, later) = EventBatchPlan::partition(&[STATE_PANE_OUTPUT]);
        assert_eq!(now, vec![0]);
        assert!(later.is_empty());
    }

    #[test]
    fn changes_active_pane_detection() {
        assert!(StateEventPolicy::changes_active_pane(7));
        assert!(!StateEventPolicy::changes_active_pane(6));
        assert!(!StateEventPolicy::changes_active_pane(0));
    }

    #[test]
    fn client_size_policy_guards_invalid_inputs() {
        // allocated=false 时走像素推算；root/cell 无效返回 None
        assert_eq!(ClientSizePolicy::cols(80, false, 0, 10, false), None);
        assert_eq!(ClientSizePolicy::cols(80, false, 1280, 0, false), None);
        assert_eq!(ClientSizePolicy::rows(0, 10), None);
        assert_eq!(ClientSizePolicy::rows(100, 0), None);
        // allocated=true 但 VTE 未实际分配（<=1 列）时退回像素推算
        assert_eq!(ClientSizePolicy::cols(1, true, 1280, 10, false), Some(128));
        assert_eq!(ClientSizePolicy::cols(0, true, 0, 10, false), None);
    }

    #[test]
    fn client_cols_prefer_vte_over_pixel_division() {
        // 2310.log：root/字宽算出 128，VTE 已布局时用实际 120，避免 htop 折行。
        assert_eq!(
            ClientSizePolicy::cols(120, true, 1280, 10, false),
            Some(120)
        );
        // 多 pane 时 active pane 列数不能当 client 宽度（1820 白屏反馈环）。
        assert_eq!(ClientSizePolicy::cols(40, true, 1280, 10, true), Some(128));
        // 未实现前 VTE 默认 80，不能压过像素推算。
        assert_eq!(
            ClientSizePolicy::cols(80, false, 1280, 10, false),
            Some(128)
        );
        assert_eq!(ClientSizePolicy::rows(580, 10), Some(58));
        assert_eq!(ClientSizePolicy::rows(0, 10), None);
    }
}
