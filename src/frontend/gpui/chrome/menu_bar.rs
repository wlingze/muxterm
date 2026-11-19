//! Linux / Windows 的自绘菜单条与下拉面板。
//!
//! macOS 用原生 NSMenu（`platform/macos_menu.rs`），gpui 0.2.2 在 Linux 上
//! `set_menus` 是空实现、Windows 只写进跳转列表，所以这两个平台必须自己画。
//! 数据来自同一份 `model::menu`，动作走 `KeyAction` → `run_action`。
//!
//! 布局：菜单条是窗口顶部一条 24pt 的行，只在非 macOS 出现；展开时下拉面板
//! 挂在标题下方，另外铺一层全窗点击层（点外面即关闭，与 NSMenu 同行为）。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::model::menu::MenuEntry;
use crate::frontend::gpui::model::menu_bar::{
    panel_height, title_width, MENU_BAR_HEIGHT, MENU_ITEM_HEIGHT, MENU_PANEL_WIDTH,
    MENU_SEPARATOR_HEIGHT, MENU_TITLE_PADDING,
};
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::TAB_BAR_FONT_SIZE;

/// 标题文本宽度估算用的字宽：11pt 等宽字约 6.6px。
/// 面板锚点只需要「跟手」，真实 shaping 由 gpui 负责。
const TITLE_CHAR_WIDTH: f32 = 6.6;

/// 菜单标题条的 debug selector 前缀（测试按它点击）。
fn title_selector(index: usize) -> String {
    format!("menu-bar-title-{index}")
}

/// 菜单项的 debug selector：按动作稳定命名，测试可以点具体命令。
fn item_selector(label: &str) -> String {
    let slug: String = label
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    format!("menu-bar-item-{slug}")
}

impl GpuiApp {
    /// 顶部菜单条。macOS 返回 `None`（原生菜单在系统菜单栏里）。
    pub(crate) fn render_menu_bar(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if crate::frontend::gpui::platform::uses_native_menu_bar() {
            return None;
        }
        let theme = self.theme().clone();
        let menus = self.menu_entries();
        let open = self.menu_bar_open();
        let titles = menus.iter().enumerate().map(|(index, menu)| {
            let label = SharedString::from(menu.title.clone());
            let is_open = open == Some(index);
            div()
                .id(SharedString::from(title_selector(index)))
                .debug_selector(move || title_selector(index))
                .flex_none()
                .h_full()
                .px(px(MENU_TITLE_PADDING))
                .flex()
                .items_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .text_size(px(TAB_BAR_FONT_SIZE))
                .text_color(if is_open { theme.accent } else { theme.fg })
                .when(is_open, |this| this.bg(theme.active_bg))
                .hover(|style| style.bg(theme.active_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        crate::frontend::gpui::state::GpuiApp::toggle_menu_bar(this, index, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    crate::frontend::gpui::state::GpuiApp::hover_menu_bar_title(this, index, cx);
                }))
                .child(label)
                .into_any_element()
        });

        Some(
            div()
                .id("menu-bar")
                .debug_selector(|| "menu-bar".into())
                .flex()
                .flex_row()
                .flex_none()
                .w_full()
                .h(px(MENU_BAR_HEIGHT))
                .px(px(6.0))
                .items_center()
                .gap(px(2.0))
                .bg(theme.chrome_bg)
                .border_b_1()
                .border_color(theme.border)
                .children(titles)
                .into_any_element(),
        )
    }

    /// 已展开菜单的下拉面板 + 点外面关闭的遮罩层。
    pub(crate) fn render_menu_panel(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let open = self.menu_bar_open()?;
        let theme = self.theme().clone();
        let menus = self.menu_entries();
        let menu = menus.get(open)?;
        let selected = self.menu_bar_state().selected;

        // 面板锚在对应标题下方：标题宽度按等宽字估算。
        let mut x = 6.0;
        for (index, other) in menus.iter().enumerate() {
            if index == open {
                break;
            }
            x += title_width(&other.title, TITLE_CHAR_WIDTH) + 2.0;
        }
        let height = panel_height(menu);

        let rows = menu
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| match entry {
                MenuEntry::Separator => div()
                    .h(px(MENU_SEPARATOR_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .child(div().h(px(1.0)).w_full().mx(px(8.0)).bg(theme.border))
                    .into_any_element(),
                MenuEntry::Item(item) => {
                    let action = item.action;
                    let enabled = item.enabled;
                    let hint =
                        crate::frontend::gpui::model::menu_bar::shortcut_hint(item.key.as_ref());
                    let label = SharedString::from(format!(
                        "{}{}",
                        if item.checked { "✓ " } else { "" },
                        item.label
                    ));
                    let selector = item_selector(&item.label);
                    let is_selected = selected == index && enabled;
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .h(px(MENU_ITEM_HEIGHT))
                        .w_full()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .rounded(px(4.0))
                        .text_size(px(TAB_BAR_FONT_SIZE))
                        .text_color(if !enabled {
                            theme.muted
                        } else if is_selected {
                            theme.accent
                        } else {
                            theme.fg
                        })
                        .when(is_selected, |this| this.bg(theme.active_bg))
                        .when(enabled, |this| {
                            this.cursor_pointer()
                            .hover(|style| style.bg(theme.active_bg))
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                crate::frontend::gpui::state::GpuiApp::hover_menu_bar_item(
                                    this, index, cx,
                                );
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    crate::frontend::gpui::state::GpuiApp::activate_menu_bar_item(
                                        this, action, window, cx,
                                    );
                                }),
                            )
                        })
                        .child(label)
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .justify_end()
                                .text_color(theme.muted)
                                .child(SharedString::from(hint.unwrap_or_default())),
                        )
                        .into_any_element()
                }
            });

        let panel = div()
            .id("menu-bar-panel")
            .debug_selector(|| "menu-bar-panel".into())
            .absolute()
            .left(px(x))
            .top(px(MENU_BAR_HEIGHT))
            .w(px(MENU_PANEL_WIDTH))
            .max_h(px(height))
            .flex()
            .flex_col()
            .py(px(4.0))
            .bg(theme.chrome_bg)
            .border_1()
            .border_color(theme.border)
            .rounded(px(6.0))
            .children(rows);

        Some(
            div()
                .id("menu-bar-layer")
                .debug_selector(|| "menu-bar-layer".into())
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_menu_bar(cx);
                    }),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}

/// 菜单条是否占据布局高度（非 macOS + 始终显示）。
pub fn menu_bar_height() -> f32 {
    if crate::frontend::gpui::model::menu_bar::visible() {
        MENU_BAR_HEIGHT
    } else {
        0.0
    }
}
