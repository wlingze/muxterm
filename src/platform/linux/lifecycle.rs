//! Pane / tab 生命周期纯决策（不依赖 GTK），便于单测防回归。

use crate::core::config::{OnLastPaneExit, OnProgramExitAbnormal};

/// 本地程序退出后对 pane 的处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneExitDecision {
    /// 保留 pane（异常 + Keep 策略）。
    Keep,
    /// 关闭 pane（可能已先 Notify）。
    Close,
}

/// 根据退出码与策略决定是否关 pane。
pub fn pane_exit_decision(code: i32, policy: OnProgramExitAbnormal) -> PaneExitDecision {
    if code != 0 && policy == OnProgramExitAbnormal::Keep {
        PaneExitDecision::Keep
    } else {
        PaneExitDecision::Close
    }
}

/// 异常退出时是否应在状态栏提示。
pub fn should_notify_abnormal_exit(code: i32, policy: OnProgramExitAbnormal) -> bool {
    code != 0
        && matches!(
            policy,
            OnProgramExitAbnormal::Notify | OnProgramExitAbnormal::Keep
        )
}

/// 所有 tab 关闭后的窗口级行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastTabsClosedAction {
    CloseWindow,
    KeepEmpty,
    NewShell,
}

pub fn last_tabs_closed_action(policy: OnLastPaneExit) -> LastTabsClosedAction {
    match policy {
        OnLastPaneExit::CloseWindow => LastTabsClosedAction::CloseWindow,
        OnLastPaneExit::KeepEmpty => LastTabsClosedAction::KeepEmpty,
        OnLastPaneExit::NewShell => LastTabsClosedAction::NewShell,
    }
}

/// 当前 tab 内 pane 循环切换；len≤1 时返回 None（无操作）。
pub fn next_pane_index(len: usize, idx: usize, forward: bool) -> Option<usize> {
    if len <= 1 {
        return None;
    }
    let idx = idx.min(len - 1);
    Some(if forward {
        (idx + 1) % len
    } else if idx == 0 {
        len - 1
    } else {
        idx - 1
    })
}

/// Alt+N 切 tab：`n==0` 为最后一个；否则 1-based 转 0-based（越界钳到 last）。
pub fn tab_index_for_shortcut(n: u32, total: usize) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let idx = if n == 0 {
        total - 1
    } else {
        (n as usize).min(total) - 1
    };
    Some(idx)
}

/// 空窗口（无 tab）是否应允许继续操作而不崩。
pub fn empty_window_is_safe(_n_tabs: usize) -> bool {
    // 空窗口是合法状态，调用方自行决定下一步（关窗 / 新建）
    true
}

/// 底部 input_bar 在正常 UI 中应始终隐藏（保留控件，仅 `set_visible(false)`）。
pub fn input_bar_should_be_visible() -> bool {
    false
}

/// 命令面板执行后是否应立刻把焦点还给 terminal。
///
/// 会打开二级对话框/QuickPick 的命令返回 false（由对话框关闭后再聚焦）。
pub fn palette_should_refocus_terminal(cmd: &str) -> bool {
    !matches!(
        cmd,
        "tmux_attach" | "tmux_new" | "ssh_connect" | "rename_pane" | "search_panes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对应：shell 异常退出 + Keep → 不关 pane。
    #[test]
    fn test_lifecycle_keep_on_abnormal() {
        assert_eq!(
            pane_exit_decision(1, OnProgramExitAbnormal::Keep),
            PaneExitDecision::Keep
        );
        assert!(should_notify_abnormal_exit(1, OnProgramExitAbnormal::Keep));
    }

    /// 对应：child-exited 后 Close / Notify 策略应关 pane。
    #[test]
    fn test_lifecycle_close_pane_on_exit() {
        assert_eq!(
            pane_exit_decision(0, OnProgramExitAbnormal::Notify),
            PaneExitDecision::Close
        );
        assert_eq!(
            pane_exit_decision(1, OnProgramExitAbnormal::Notify),
            PaneExitDecision::Close
        );
        assert_eq!(
            pane_exit_decision(1, OnProgramExitAbnormal::Close),
            PaneExitDecision::Close
        );
        assert!(should_notify_abnormal_exit(
            2,
            OnProgramExitAbnormal::Notify
        ));
        assert!(!should_notify_abnormal_exit(
            0,
            OnProgramExitAbnormal::Notify
        ));
    }

    /// 对应：最后 tab 退出 → close_window。
    #[test]
    fn test_lifecycle_last_tab_closes_window() {
        assert_eq!(
            last_tabs_closed_action(OnLastPaneExit::CloseWindow),
            LastTabsClosedAction::CloseWindow
        );
    }

    #[test]
    fn test_lifecycle_last_tab_keep_empty() {
        assert_eq!(
            last_tabs_closed_action(OnLastPaneExit::KeepEmpty),
            LastTabsClosedAction::KeepEmpty
        );
    }

    #[test]
    fn test_lifecycle_last_tab_new_shell_compat() {
        assert_eq!(
            last_tabs_closed_action(OnLastPaneExit::NewShell),
            LastTabsClosedAction::NewShell
        );
    }

    /// 对应：多 pane 循环切换。
    #[test]
    fn test_lifecycle_switch_pane_wrap() {
        assert_eq!(next_pane_index(3, 0, true), Some(1));
        assert_eq!(next_pane_index(3, 2, true), Some(0));
        assert_eq!(next_pane_index(3, 0, false), Some(2));
        assert_eq!(next_pane_index(3, 1, false), Some(0));
    }

    /// 对应：单 pane 切换无操作。
    #[test]
    fn test_lifecycle_switch_pane_single_noop() {
        assert_eq!(next_pane_index(1, 0, true), None);
        assert_eq!(next_pane_index(0, 0, true), None);
    }

    /// 对应：Alt+0 / Alt+1 切 tab 索引。
    #[test]
    fn test_lifecycle_tab_shortcut_index() {
        assert_eq!(tab_index_for_shortcut(1, 5), Some(0));
        assert_eq!(tab_index_for_shortcut(5, 5), Some(4));
        assert_eq!(tab_index_for_shortcut(0, 5), Some(4)); // last
        assert_eq!(tab_index_for_shortcut(9, 3), Some(2)); // clamp
        assert_eq!(tab_index_for_shortcut(1, 0), None);
    }

    /// 对应：空窗口状态不崩。
    #[test]
    fn test_lifecycle_empty_window_safe() {
        assert!(empty_window_is_safe(0));
        assert_eq!(tab_index_for_shortcut(1, 0), None);
        assert_eq!(next_pane_index(0, 0, true), None);
    }

    /// 对应：快速连续分割场景下索引仍合法（模拟 5 次）。
    #[test]
    fn test_lifecycle_rapid_split_indices_stable() {
        let mut n = 1usize;
        for _ in 0..5 {
            n += 1;
            assert!(next_pane_index(n, 0, true).is_some());
        }
        assert_eq!(n, 6);
    }

    /// 对应：input_bar 始终隐藏（不删除）。
    #[test]
    fn test_lifecycle_input_bar_always_hidden() {
        assert!(!input_bar_should_be_visible());
    }

    /// 对应：命令面板后焦点回 terminal；二级对话框除外。
    #[test]
    fn test_lifecycle_palette_refocus_rules() {
        assert!(palette_should_refocus_terminal("new_tab"));
        assert!(palette_should_refocus_terminal("new_pane"));
        assert!(palette_should_refocus_terminal("close_pane"));
        assert!(palette_should_refocus_terminal("ssh_disconnect"));
        assert!(!palette_should_refocus_terminal("ssh_connect"));
        assert!(!palette_should_refocus_terminal("tmux_attach"));
        assert!(!palette_should_refocus_terminal("tmux_new"));
        assert!(!palette_should_refocus_terminal("rename_pane"));
        assert!(!palette_should_refocus_terminal("search_panes"));
    }
}
