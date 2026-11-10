//! Pane `⋯` 菜单的浮层渲染。
//!
//! 对照 macOS `PaneTitleBarView.actionMenu`（NSMenu）：条目顺序、分隔线、
//! 「点外面即关闭」都照做；差别只是 gpui 里自己画，不用原生菜单。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::pane_grid::Rect;
use crate::frontend::gpui::model::pane_menu::{PaneMenuItem, MENU_ITEMS};
use crate::frontend::gpui::model::pane_title::TITLE_BAR_HEIGHT;
use crate::frontend::gpui::state::GpuiApp;

/// 菜单宽度与行高（NSMenu 的观感：略宽、紧凑）。
pub const MENU_WIDTH: f32 = 208.0;
pub const MENU_ROW_HEIGHT: f32 = 24.0;

impl GpuiApp {
    /// 渲染已打开的 pane 菜单；没打开返回 `None`。
    ///
    /// `area` 是终端内容区的矩形（与 `pane_grid::pane_rects` 同一坐标系）。
    pub(crate) fn render_pane_menu(
        &self,
        area: Rect,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let menu = *self.pane_menu.as_ref()?;
        let theme = self.theme().clone();
        let pane_id = menu.pane_id;
        // 菜单锚在这个 pane 的标题条右下角。
        let rect = crate::frontend::gpui::chrome::pane_grid::pane_rects(&self.layout()?, area)
            .into_iter()
            .find(|(id, _)| *id == pane_id)
            .map(|(_, rect)| rect)?;

        let height = MENU_ITEMS
            .iter()
            .map(|item| match item {
                PaneMenuItem::Separator => 1.0 + 8.0,
                PaneMenuItem::Action(_) => MENU_ROW_HEIGHT,
            })
            .sum::<f32>()
            + 8.0;
        let x = (rect.x + rect.max_width() - MENU_WIDTH - 4.0).max(0.0);
        let y = (rect.y + TITLE_BAR_HEIGHT + 2.0).min((area.h - height - 4.0).max(0.0));

        // 行下标 → 动作下标：选中态按动作算，分隔线不占号。
        let mut action_index = 0usize;
        let rows = MENU_ITEMS.iter().map(|item| match item {
            PaneMenuItem::Separator => div()
                .h(px(1.0))
                .mx(px(8.0))
                .my(px(4.0))
                .bg(theme.border)
                .into_any_element(),
            PaneMenuItem::Action(action) => {
                let action = *action;
                let is_selected = action_index == menu.selected;
                action_index += 1;
                let slug = action.label().to_lowercase().replace(' ', "-");
                div()
                    .id(SharedString::from(format!("pane-menu-{slug}")))
                    .debug_selector(move || format!("pane-menu-{slug}"))
                    .h(px(MENU_ROW_HEIGHT))
                    .w_full()
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_color(if is_selected { theme.accent } else { theme.fg })
                    .when(is_selected, |this| this.bg(theme.active_bg))
                    .hover(|style| style.bg(theme.active_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.run_pane_title_action(pane_id, action, window, cx);
                        }),
                    )
                    .child(SharedString::from(action.label()))
                    .into_any_element()
            }
        });

        let panel = div()
            .id("pane-menu")
            .debug_selector(|| "pane-menu".into())
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .py(px(4.0))
            .bg(theme.chrome_bg)
            .border_1()
            .border_color(theme.border)
            .rounded(px(6.0))
            .children(rows);

        // 点菜单外面 = 关闭（NSMenu 行为）。这一层只在菜单开着时存在，
        // 所以平时不会挡住终端的点击。
        Some(
            div()
                .id("pane-menu-layer")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_pane_menu(cx);
                    }),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}
