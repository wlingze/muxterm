//! 终端右键菜单的浮层渲染。
//!
//! 与 pane `⋯` 菜单同样的做法：gpui 自己画，点菜单外面即关闭。区别是
//! 位置跟鼠标（窗口坐标），而不是锚在 pane 标题条上。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::model::selection_menu::{SelectionMenuAction, MENU_ACTIONS};
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

/// 菜单宽度与行高（与 pane 菜单同一观感）。
pub const SELECTION_MENU_WIDTH: f32 = 176.0;
pub const SELECTION_MENU_ROW: f32 = 26.0;
const MENU_PAD: f32 = 4.0;

impl GpuiApp {
    pub(crate) fn render_selection_menu(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let menu = self.selection_menu?;
        let theme = self.theme().clone();
        let has_selection = self.has_terminal_selection();
        let height = MENU_ACTIONS.len() as f32 * SELECTION_MENU_ROW + MENU_PAD * 2.0;
        // 贴边时往回收，菜单不能跑出窗口。
        let x = menu
            .x
            .min(self.window_width - SELECTION_MENU_WIDTH - MENU_PAD)
            .max(MENU_PAD);
        let y = menu
            .y
            .min(self.window_height - height - MENU_PAD)
            .max(MENU_PAD);

        let rows = MENU_ACTIONS
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let action = *action;
                let enabled = menu.enabled(action, has_selection);
                let selected = index == menu.selected && enabled;
                render_item(&theme, action, selected, enabled, cx)
            })
            .collect::<Vec<_>>();

        let panel = div()
            .id("selection-menu")
            .debug_selector(|| "selection-menu".to_string())
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(SELECTION_MENU_WIDTH))
            .flex()
            .flex_col()
            .py(px(MENU_PAD))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.chrome_bg)
            .shadow_lg()
            .children(rows);

        // 点菜单外面 = 关闭（NSMenu 行为）。
        Some(
            div()
                .id("selection-menu-layer")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_selection_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_selection_menu(cx);
                    }),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}

fn render_item(
    theme: &Theme,
    action: SelectionMenuAction,
    selected: bool,
    enabled: bool,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let slug = action.label().to_lowercase().replace(' ', "-");
    div()
        .id(SharedString::from(format!("selection-menu-{slug}")))
        .debug_selector(move || format!("selection-menu-{slug}"))
        .h(px(SELECTION_MENU_ROW))
        .w_full()
        .px(px(10.0))
        .flex()
        .items_center()
        .justify_between()
        .rounded(px(4.0))
        .cursor(if enabled {
            gpui::CursorStyle::PointingHand
        } else {
            gpui::CursorStyle::Arrow
        })
        .text_color(if !enabled {
            theme.muted
        } else if selected {
            theme.accent
        } else {
            theme.fg
        })
        .when(selected, |this| this.bg(theme.active_bg))
        .when(enabled, |this| {
            this.hover(|style| style.bg(theme.active_bg))
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.run_selection_menu_action(action, window, cx);
            }),
        )
        .child(SharedString::from(action.label()))
        .child(
            div()
                .text_size(px(10.5))
                .text_color(theme.muted)
                .child(SharedString::from(action.shortcut())),
        )
        .into_any_element()
}
