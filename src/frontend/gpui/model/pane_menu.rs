//! Pane 标题条的 `⋯` 菜单（纯数据 + 选择状态）。
//!
//! 对照 macOS `PaneTitleBarView.configureActionMenu()`：条目顺序、分隔线位置
//! 与 Swift 一致；菜单**绑定到具体 pane**，执行前先把它切成活动 pane
//! （Swift `showActionMenu` 里先 `onActivate?()`），否则 `run_action` 会作用
//! 在别处的焦点 pane 上 —— 这正是用户报的「点 pane 菜单却新建了 pane」。

use crate::frontend::gpui::model::pane_title::PaneTitleAction;

/// 菜单的一行：动作或分隔线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMenuItem {
    Action(PaneTitleAction),
    Separator,
}

/// 菜单条目顺序（Swift：两个分割 → 分隔 → 移动 → 分隔 → 全屏/循环 → 分隔 → 关闭）。
pub const MENU_ITEMS: [PaneMenuItem; 9] = [
    PaneMenuItem::Action(PaneTitleAction::SplitHorizontal),
    PaneMenuItem::Action(PaneTitleAction::SplitVertical),
    PaneMenuItem::Separator,
    PaneMenuItem::Action(PaneTitleAction::MoveToNewTab),
    PaneMenuItem::Separator,
    PaneMenuItem::Action(PaneTitleAction::Fullscreen),
    PaneMenuItem::Action(PaneTitleAction::CycleLayout),
    PaneMenuItem::Separator,
    PaneMenuItem::Action(PaneTitleAction::Close),
];

/// 已打开的 pane 菜单。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneMenu {
    pub pane_id: u32,
    /// 键盘选中的动作下标（见 `actions()`，与 `MENU_ITEMS` 的下标不是一回事）。
    pub selected: usize,
}

impl PaneMenu {
    pub fn new(pane_id: u32) -> Self {
        Self {
            pane_id,
            selected: 0,
        }
    }

    /// 可选项（跳过分隔线），顺序与菜单视觉顺序一致。
    pub fn actions() -> Vec<PaneTitleAction> {
        MENU_ITEMS
            .iter()
            .filter_map(|item| match item {
                PaneMenuItem::Action(action) => Some(*action),
                PaneMenuItem::Separator => None,
            })
            .collect()
    }

    pub fn len() -> usize {
        Self::actions().len()
    }

    /// 上下移动，越界夹住（菜单不环绕，与 macOS NSMenu 一致）。
    pub fn move_selection(&mut self, delta: i32) {
        let len = Self::len() as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, len - 1) as usize;
    }

    pub fn selected_action(&self) -> Option<PaneTitleAction> {
        Self::actions().get(self.selected).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `FlatChromeTests.testPaneTitleBarActions`：动作顺序与 Swift 相同。
    #[test]
    fn menu_order_matches_the_swift_action_menu() {
        assert_eq!(
            PaneMenu::actions(),
            vec![
                PaneTitleAction::SplitHorizontal,
                PaneTitleAction::SplitVertical,
                PaneTitleAction::MoveToNewTab,
                PaneTitleAction::Fullscreen,
                PaneTitleAction::CycleLayout,
                PaneTitleAction::Close,
            ]
        );
    }

    /// 分隔线不参与键盘选择，上下夹住不环绕。
    #[test]
    fn selection_skips_separators_and_stops_at_the_edges() {
        let mut menu = PaneMenu::new(7);
        assert_eq!(
            menu.selected_action(),
            Some(PaneTitleAction::SplitHorizontal)
        );
        menu.move_selection(-1);
        assert_eq!(menu.selected, 0, "到顶不再上移");
        menu.move_selection(100);
        assert_eq!(menu.selected, PaneMenu::len() - 1, "到底不再下移");
        assert_eq!(menu.selected_action(), Some(PaneTitleAction::Close));
    }
}
