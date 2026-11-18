//! 终端右键菜单（复制 / 粘贴 / 全选）。
//!
//! 用户要求「鼠标右键应该可以复制」：右键不再转发给应用，而是在指针位置
//! 弹出一个小菜单，避免用户先拖选、再去够 `Edit → Copy` 菜单或记快捷键。

/// 菜单的一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMenuAction {
    Copy,
    Paste,
    SelectAll,
}

impl SelectionMenuAction {
    pub fn label(self) -> &'static str {
        match self {
            SelectionMenuAction::Copy => "Copy",
            SelectionMenuAction::Paste => "Paste",
            SelectionMenuAction::SelectAll => "Select All",
        }
    }

    /// 行尾的快捷键提示（与内置默认键一致；只做展示）。
    pub fn shortcut(self) -> &'static str {
        match self {
            SelectionMenuAction::Copy => "⌘C",
            SelectionMenuAction::Paste => "⌘V",
            SelectionMenuAction::SelectAll => "⌘A",
        }
    }
}

pub const MENU_ACTIONS: [SelectionMenuAction; 3] = [
    SelectionMenuAction::Copy,
    SelectionMenuAction::Paste,
    SelectionMenuAction::SelectAll,
];

/// 已打开的右键菜单：窗口坐标 + 键盘选中行。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionMenu {
    pub x: f32,
    pub y: f32,
    pub pane_id: u32,
    pub selected: usize,
}

impl SelectionMenu {
    pub fn new(x: f32, y: f32, pane_id: u32) -> Self {
        Self {
            x,
            y,
            pane_id,
            selected: 0,
        }
    }

    pub fn len() -> usize {
        MENU_ACTIONS.len()
    }

    /// 上下移动，越界夹住（与 macOS NSMenu 一致，不环绕）。
    pub fn move_selection(&mut self, delta: i32) {
        let last = Self::len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn selected_action(&self) -> SelectionMenuAction {
        MENU_ACTIONS[self.selected.min(MENU_ACTIONS.len() - 1)]
    }

    /// Copy 在没有选区时置灰（粘贴、全选始终可用）。
    pub fn enabled(self, action: SelectionMenuAction, has_selection: bool) -> bool {
        !matches!(action, SelectionMenuAction::Copy) || has_selection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_clamps_at_both_ends() {
        let mut menu = SelectionMenu::new(10.0, 20.0, 7);
        assert_eq!(menu.selected_action(), SelectionMenuAction::Copy);
        menu.move_selection(1);
        assert_eq!(menu.selected_action(), SelectionMenuAction::Paste);
        menu.move_selection(-5);
        assert_eq!(menu.selected_action(), SelectionMenuAction::Copy);
        for _ in 0..10 {
            menu.move_selection(1);
        }
        assert_eq!(menu.selected_action(), SelectionMenuAction::SelectAll);
    }

    #[test]
    fn copy_is_greyed_without_a_selection() {
        let menu = SelectionMenu::new(0.0, 0.0, 1);
        assert!(!menu.enabled(SelectionMenuAction::Copy, false));
        assert!(menu.enabled(SelectionMenuAction::Copy, true));
        assert!(menu.enabled(SelectionMenuAction::Paste, false));
        assert!(menu.enabled(SelectionMenuAction::SelectAll, false));
    }
}
