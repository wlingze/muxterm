//! Linux / Windows 自绘菜单条的状态操作。
//!
//! 与 macOS 原生菜单共用同一份 `model::menu` 数据与 `KeyAction` 执行入口，
//! 差别只在「怎么触发」：原生菜单由 AppKit 发 `ShortcutCommand`，这里由
//! 窗口内的点击与键盘直接调 `run_action`。

use gpui::{Context, KeyDownEvent, Window};

use crate::frontend::gpui::model::menu::Menu;
use crate::frontend::gpui::model::menu_bar::MenuBar;
use crate::frontend::gpui::model::shortcuts::KeyAction;

impl crate::frontend::gpui::state::GpuiApp {
    /// 菜单的运行时状态（勾选 / 置灰）：原生菜单与自绘菜单条共用。
    pub fn menu_state(&self) -> crate::frontend::gpui::model::menu::MenuState {
        use crate::frontend::gpui::model::menu::MenuState;
        MenuState {
            tab_bar_on_top: self.tab_bar_position()
                == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top,
            has_selection: self.selection.is_some(),
            has_pane: self.focused_pane.is_some(),
        }
    }

    /// 当前菜单（本地化后）。自绘菜单条、测试与 Info 浮层都读这一份。
    pub fn menu_entries(&self) -> Vec<Menu> {
        crate::frontend::gpui::model::menu::localized_menu_with(self.menu_state())
    }

    /// 菜单条是否展开。
    pub fn menu_bar_open(&self) -> Option<usize> {
        self.menu_bar.open
    }

    /// 点击菜单标题：同一条再点一次收起，换成另一条直接切换。
    pub fn toggle_menu_bar(&mut self, index: usize, cx: &mut Context<Self>) {
        let menus = self.menu_entries();
        self.menu_bar.toggle(index, &menus);
        cx.notify();
    }

    /// 鼠标移到某条菜单标题上：已经展开时跟随切换（原生菜单条的行为）。
    pub fn hover_menu_bar_title(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.menu_bar.is_open() && self.menu_bar.open != Some(index) {
            let menus = self.menu_entries();
            self.menu_bar.open_menu(index, &menus);
            cx.notify();
        }
    }

    /// 鼠标移到展开菜单的某一条上。
    pub fn hover_menu_bar_item(&mut self, index: usize, cx: &mut Context<Self>) {
        let menus = self.menu_entries();
        let before = self.menu_bar.selected;
        self.menu_bar.hover_item(index, &menus);
        if self.menu_bar.selected != before {
            cx.notify();
        }
    }

    pub fn close_menu_bar(&mut self, cx: &mut Context<Self>) {
        if self.menu_bar.is_open() {
            self.menu_bar.close();
            cx.notify();
        }
    }

    /// 点击菜单项：收起菜单再执行（动作可能打开浮层 / 切窗口）。
    pub fn activate_menu_bar_item(
        &mut self,
        action: KeyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_bar.close();
        self.run_action(action, window, cx);
        cx.notify();
    }

    /// 菜单条打开时的键盘处理。返回 `true` 表示这一下已消费。
    ///
    /// 只接管导航键；其它按键先收起菜单再继续走正常路由，用户可以直接
    /// 接着打字（菜单不是模态窗口）。
    pub(super) fn menu_bar_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.menu_bar.is_open() {
            return false;
        }
        let keystroke = crate::frontend::gpui::platform::normalize_keystroke(&event.keystroke);
        let menus = self.menu_entries();
        match keystroke.key.as_str() {
            "escape" => {
                self.menu_bar.close();
                cx.notify();
                true
            }
            "down" => {
                self.menu_bar.move_selection(1, &menus);
                cx.notify();
                true
            }
            "up" => {
                self.menu_bar.move_selection(-1, &menus);
                cx.notify();
                true
            }
            "right" => {
                self.menu_bar.move_menu(1, &menus);
                cx.notify();
                true
            }
            "left" => {
                self.menu_bar.move_menu(-1, &menus);
                cx.notify();
                true
            }
            "enter" | "space" => {
                if let Some(action) = self.menu_bar.selected_action(&menus) {
                    self.menu_bar.close();
                    self.run_action(action, window, cx);
                    cx.notify();
                }
                true
            }
            _ => {
                // 其它键：收起菜单，继续原有路由（不吞键）。
                self.menu_bar.close();
                cx.notify();
                false
            }
        }
    }

    /// 给测试用：当前高亮项的动作。
    pub fn menu_bar_selected_action(&self) -> Option<KeyAction> {
        self.menu_bar.selected_action(&self.menu_entries())
    }

    /// 给测试用：菜单条状态（打开的下标 + 高亮项）。
    pub fn menu_bar_state(&self) -> &MenuBar {
        &self.menu_bar
    }
}
