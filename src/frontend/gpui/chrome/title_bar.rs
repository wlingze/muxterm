//! 客户端装饰（CSD）标题栏：Linux（Wayland/GNOME）用。
//!
//! GNOME 的 Mutter 在 `xdg-decoration` 上永远回 client，服务端不画标题栏，
//! 窗口就既没有关闭按钮也没有最小化/最大化 —— 用户只能靠快捷键关窗。
//! `chrome/menu_bar.rs` 是菜单，这里补系统窗口控制。
//!
//! 只在 `window.window_decorations() == Decorations::Client` 时出现：
//! - X11 / KDE 走服务端装饰时 `Decorations::Server`，不画（避免两条标题栏）。
//! - Windows 现在用原生标题栏（`platform::titlebar_options` 传
//!   `appears_transparent: false`），也不画。
//!
//! 高度参与布局：侧栏 / 面板 / pane 网格都要扣掉它，
//! 统一走 [`GpuiApp::top_chrome_height`]。

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, Rgba, SharedString, Window};

use crate::frontend::gpui::state::{window_title_text, GpuiApp};
use crate::frontend::gpui::theme::TAB_BAR_FONT_SIZE;

/// 标题栏高度：与 GNOME 默认 headerbar（37px）接近，容得下 16px 按钮。
pub const TITLE_BAR_HEIGHT: f32 = 36.0;
/// 单个窗口控制按钮的宽度（命中区）。
const BUTTON_WIDTH: f32 = 46.0;
/// 关闭按钮的 `×` 字号。
const CLOSE_GLYPH_SIZE: f32 = 15.0;

impl GpuiApp {
    /// 窗口顶部所有自绘 chrome 的高度（CSD 标题栏 + 菜单条）。
    ///
    /// 布局里凡是「扣掉菜单条」的地方都应该用它，否则 CSD 平台会多算一条。
    pub(crate) fn top_chrome_height(&self) -> f32 {
        crate::frontend::gpui::chrome::menu_bar::menu_bar_height()
            + if self.client_title_bar {
                TITLE_BAR_HEIGHT
            } else {
                0.0
            }
    }

    /// 自绘标题栏；不需要时返回 `None`。
    pub(crate) fn render_title_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.client_title_bar {
            return None;
        }
        let theme = self.theme().clone();
        let title = SharedString::from(window_title_text(
            self.show_title_bar(),
            &self.visible_workspace_name(),
        ));

        Some(
            div()
                .id("title-bar")
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .h(px(TITLE_BAR_HEIGHT))
                .w_full()
                .bg(theme.chrome_bg)
                .border_b_1()
                .border_color(theme.border)
                // 按住空白处拖动窗口；双击最大化/还原。
                .on_mouse_down(MouseButton::Left, |_, window, _| {
                    window.start_window_move();
                })
                .on_click(|event, window, _| {
                    if event.click_count() == 2 {
                        window.zoom_window();
                    }
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_1()
                        .min_w(px(0.0))
                        .pl(px(12.0))
                        .text_size(px(TAB_BAR_FONT_SIZE + 1.0))
                        .text_color(theme.muted)
                        .child(title),
                )
                .child(window_control(
                    "title-bar-min",
                    cx,
                    theme.active_bg,
                    minimize_glyph(theme.fg),
                    |window, _| window.minimize_window(),
                ))
                .child(window_control(
                    "title-bar-max",
                    cx,
                    theme.active_bg,
                    maximize_glyph(theme.fg),
                    |window, _| window.zoom_window(),
                ))
                .child(window_control(
                    "title-bar-close",
                    cx,
                    theme.danger,
                    close_glyph(theme.fg),
                    |window, _| window.remove_window(),
                ))
                .into_any_element(),
        )
    }
}

/// 一个窗口控制按钮。按下就执行（`stop_propagation` 防止触发标题栏拖动）。
fn window_control(
    id: &'static str,
    cx: &mut Context<GpuiApp>,
    hover: Rgba,
    glyph: AnyElement,
    action: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector({
            let id = id.to_string();
            move || id.clone()
        })
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .w(px(BUTTON_WIDTH))
        .h_full()
        .hover(|style| style.bg(hover))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_, _, window, cx| {
                cx.stop_propagation();
                action(window, cx);
            }),
        )
        .child(glyph)
        .into_any_element()
}

/// 最小化：一条 10×1 的横线。
fn minimize_glyph(color: Rgba) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(10.0))
        .h(px(1.0))
        .bg(color)
        .into_any_element()
}

/// 最大化：9×9 空心方框。
fn maximize_glyph(color: Rgba) -> AnyElement {
    div()
        .w(px(9.0))
        .h(px(9.0))
        .border_1()
        .border_color(color)
        .into_any_element()
}

/// 关闭：`×`。系统等宽字体都有 U+00D7，不依赖图标字体。
fn close_glyph(color: Rgba) -> AnyElement {
    div()
        .text_size(px(CLOSE_GLYPH_SIZE))
        .line_height(px(CLOSE_GLYPH_SIZE + 2.0))
        .text_color(color)
        .child("×")
        .into_any_element()
}
