//! 非 macOS 平台的自绘菜单条：状态机与纯几何。
//!
//! macOS 用原生 NSMenu（`platform/macos_menu.rs`），Linux / Windows 没有
//! 系统菜单栏（gpui 0.2.2 在 Linux 上是空实现、Windows 只写进跳转列表），
//! 所以在窗口顶部自绘一条：数据仍是 `model::menu` 的同一份 `Vec<Menu>`，
//! 动作仍走 `KeyAction` → `GpuiApp::run_action`，与原生菜单零分叉。
//!
//! 这里只放**可测的纯逻辑**：打开/关闭、上下移动、回车选中、快捷键提示
//! 文案。像素渲染在 `chrome/menu_bar.rs`。

use crate::frontend::gpui::model::menu::{Menu, MenuEntry, MenuKey};
use crate::frontend::gpui::model::shortcuts::KeyAction;
use crate::frontend::gpui::platform::desktop::{DesktopOs, PrimaryModifier};

/// 菜单条高度：与现有 24pt 单条栏同一尺度，避免出现两层不同高度的条。
pub const MENU_BAR_HEIGHT: f32 = 24.0;
/// 每个菜单标题左右各留的内边距。
pub const MENU_TITLE_PADDING: f32 = 8.0;
/// 下拉面板宽度：能放下 "Move Pane to New Tab" + 快捷键提示。
pub const MENU_PANEL_WIDTH: f32 = 260.0;
/// 单条菜单项高度。
pub const MENU_ITEM_HEIGHT: f32 = 22.0;
/// 分隔线高度。
pub const MENU_SEPARATOR_HEIGHT: f32 = 7.0;

/// 自绘菜单条的交互状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuBar {
    /// 当前展开的菜单下标（`None` = 收起）。
    pub open: Option<usize>,
    /// 展开菜单里高亮的条目下标（含分隔线的原始下标）。
    pub selected: usize,
}

impl MenuBar {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// 打开第 `index` 个菜单，高亮落到第一个可执行条目。
    pub fn open_menu(&mut self, index: usize, menus: &[Menu]) {
        self.open = Some(index);
        self.selected = menus.get(index).map(first_selectable).unwrap_or_default();
    }

    pub fn toggle(&mut self, index: usize, menus: &[Menu]) {
        if self.open == Some(index) {
            self.close();
        } else {
            self.open_menu(index, menus);
        }
    }

    pub fn close(&mut self) {
        self.open = None;
        self.selected = 0;
    }

    /// 鼠标悬停到展开菜单里的某一条：置灰项与分隔线不抢高亮。
    pub fn hover_item(&mut self, index: usize, menus: &[Menu]) {
        let Some(menu) = self.open.and_then(|open| menus.get(open)) else {
            return;
        };
        if is_selectable(menu, index) {
            self.selected = index;
        }
    }

    /// 键盘上下移动：跳过分隔线与置灰项。
    pub fn move_selection(&mut self, delta: i32, menus: &[Menu]) {
        let Some(menu) = self.open.and_then(|open| menus.get(open)) else {
            return;
        };
        let Some(next) = step_selectable(menu, self.selected, delta) else {
            return;
        };
        self.selected = next;
    }

    /// 左右移动：在菜单标题之间切换（展开状态下直接换菜单）。
    pub fn move_menu(&mut self, delta: i32, menus: &[Menu]) {
        if menus.is_empty() {
            return;
        }
        let current = self.open.unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(menus.len() as i32) as usize;
        self.open_menu(next, menus);
    }

    /// 当前高亮项对应的动作（置灰项返回 `None`）。
    pub fn selected_action(&self, menus: &[Menu]) -> Option<KeyAction> {
        let menu = self.open.and_then(|open| menus.get(open))?;
        match menu.entries.get(self.selected)? {
            MenuEntry::Item(item) if item.enabled => Some(item.action),
            _ => None,
        }
    }
}

fn is_selectable(menu: &Menu, index: usize) -> bool {
    matches!(
        menu.entries.get(index),
        Some(MenuEntry::Item(item)) if item.enabled
    )
}

fn first_selectable(menu: &Menu) -> usize {
    menu.entries
        .iter()
        .position(|entry| matches!(entry, MenuEntry::Item(item) if item.enabled))
        .unwrap_or(0)
}

fn step_selectable(menu: &Menu, from: usize, delta: i32) -> Option<usize> {
    let len = menu.entries.len();
    if len == 0 {
        return None;
    }
    let mut index = from as i32;
    for _ in 0..len {
        index = (index + delta).rem_euclid(len as i32);
        if is_selectable(menu, index as usize) {
            return Some(index as usize);
        }
    }
    None
}

/// 标题条的占用宽度（文本宽度按等宽字估算，渲染层用真实 shaping）。
pub fn title_width(label: &str, char_width: f32) -> f32 {
    (label.chars().count() as f32 * char_width) + MENU_TITLE_PADDING * 2.0
}

/// 下拉面板高度：条目与分隔线各自的高度之和 + 上下内边距。
pub fn panel_height(menu: &Menu) -> f32 {
    menu.entries
        .iter()
        .map(|entry| match entry {
            MenuEntry::Item(_) => MENU_ITEM_HEIGHT,
            MenuEntry::Separator => MENU_SEPARATOR_HEIGHT,
        })
        .sum::<f32>()
        + 4.0
}

/// 菜单条目右侧的快捷键提示。
///
/// macOS 用连写字形（`⌘⇧T`），Linux / Windows 用 `Alt+Shift+T`；
/// 命令键取当前配置的 `[shortcuts] primary_key`。
pub fn shortcut_label(key: &MenuKey, os: DesktopOs, primary: PrimaryModifier) -> String {
    // 和弦的排布（含「命令键 = Alt 时 option 去重」）统一由
    // `platform::chord_label_for` 决定，菜单提示与设置页不会出现两套写法。
    crate::frontend::gpui::platform::chord_label_for(
        os,
        primary,
        key.command,
        key.shift,
        key.option,
        key.control,
        &key.key,
    )
}

/// 已本地化菜单的快捷键提示（渲染层直接用）。
pub fn shortcut_hint(key: Option<&MenuKey>) -> Option<String> {
    let key = key?;
    Some(shortcut_label(
        key,
        super::super::platform::os(),
        super::super::platform::primary_modifier(),
    ))
}

/// 平台是否应该画这条菜单条。
pub fn visible() -> bool {
    !super::super::platform::uses_native_menu_bar()
}

// `format_shortcut` 是 platform 层的通用格式函数；这里只在测试里直接验证一致性。
#[cfg(test)]
fn _uses_platform_format() -> String {
    crate::frontend::gpui::platform::format_shortcut(&[], "X")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::menu::main_menu_with;

    fn menus() -> Vec<Menu> {
        main_menu_with(crate::frontend::gpui::model::menu::MenuState::default())
    }

    #[test]
    fn menu_bar_is_hidden_on_macos() {
        assert_eq!(visible(), !cfg!(target_os = "macos"));
    }

    #[test]
    fn toggle_opens_then_closes_the_same_menu() {
        let menus = menus();
        let mut bar = MenuBar::default();
        bar.toggle(1, &menus);
        assert_eq!(bar.open, Some(1));
        assert!(bar.is_open());
        // 打开时高亮第一个可执行项（跳过分隔线）。
        let first = bar.selected;
        assert!(is_selectable(&menus[1], first));
        bar.toggle(1, &menus);
        assert_eq!(bar.open, None);
    }

    #[test]
    fn selection_moves_over_separators_and_disabled_items() {
        let mut menus = menus();
        // 关掉一个中间项，确认键盘上下会跳过它。
        if let MenuEntry::Item(item) = &mut menus[1].entries[1] {
            item.enabled = false;
        }
        let mut bar = MenuBar::default();
        bar.open_menu(1, &menus);
        let start = bar.selected;
        bar.move_selection(1, &menus);
        assert_ne!(bar.selected, start, "下移必须换到下一个可执行项");
        assert!(is_selectable(&menus[1], bar.selected));
        // 绕一圈回到起点：不能卡在置灰项上。
        for _ in 0..menu_items(&menus[1]) {
            bar.move_selection(1, &menus);
            assert!(is_selectable(&menus[1], bar.selected));
        }
    }

    fn menu_items(menu: &Menu) -> usize {
        menu.entries.len() + 1
    }

    #[test]
    fn selected_action_returns_the_highlighted_item() {
        let menus = menus();
        let mut bar = MenuBar::default();
        bar.open_menu(0, &menus);
        let action = bar.selected_action(&menus);
        assert_eq!(action, Some(KeyAction::About));
    }

    #[test]
    fn horizontal_moves_wrap_between_menus() {
        let menus = menus();
        let mut bar = MenuBar::default();
        bar.open_menu(0, &menus);
        bar.move_menu(-1, &menus);
        assert_eq!(bar.open, Some(menus.len() - 1));
        bar.move_menu(1, &menus);
        assert_eq!(bar.open, Some(0));
    }

    #[test]
    fn shortcut_hints_follow_the_platform_style() {
        let key = MenuKey {
            key: "t".into(),
            command: true,
            shift: true,
            option: false,
            control: false,
        };
        let label = shortcut_label(&key, DesktopOs::Linux, PrimaryModifier::Alt);
        assert_eq!(label, "Alt+Shift+T");
        // Apple 的修饰键顺序是 ⌃⌥⇧⌘（Shift 在 Command 前面）。
        let mac = shortcut_label(&key, DesktopOs::MacOs, PrimaryModifier::Command);
        assert_eq!(mac, "⇧⌘T");
    }

    #[test]
    fn option_and_primary_alt_do_not_repeat() {
        let key = MenuKey {
            key: "i".into(),
            command: true,
            shift: false,
            option: true,
            control: false,
        };
        assert_eq!(
            shortcut_label(&key, DesktopOs::Linux, PrimaryModifier::Alt),
            "Alt+I"
        );
    }

    #[test]
    fn panel_height_counts_items_and_separators() {
        let menus = menus();
        let height = panel_height(&menus[0]);
        let items = menus[0]
            .entries
            .iter()
            .filter(|entry| matches!(entry, MenuEntry::Item(_)))
            .count() as f32;
        let separators = menus[0].entries.len() as f32 - items;
        assert!(
            (height - (items * MENU_ITEM_HEIGHT + separators * MENU_SEPARATOR_HEIGHT + 4.0)).abs()
                < 0.01
        );
    }

    #[test]
    fn every_menu_title_has_a_label_and_width() {
        for menu in menus() {
            assert!(!menu.title.is_empty());
            assert!(title_width(&menu.title, 7.0) > MENU_TITLE_PADDING);
        }
    }
}
