//! 独立输入框浮层：目标徽章、草稿页 tab、正文、状态行。
//!
//! 文本本身由 `state.rs` 的按键路径维护（gpui 0.2 没有现成的文本控件），
//! 这里只负责把 `ComposerDraftStore` 的当前页画出来。窗口尺寸跟 macOS 一样
//! 是竖长比例：宽度略小于半屏，高度接近八成屏高。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::model::composer::{self, ComposerDraftPage};
use crate::frontend::gpui::state::GpuiApp;

impl GpuiApp {
    /// 输入框浮层；没打开时返回 `None`。
    pub(crate) fn render_composer(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.composer_open {
            return None;
        }
        let theme = self.theme().clone();
        let page = self.composer_page()?.clone();
        let (width, height) = composer::window_layout::size(1440.0, 900.0);

        let badge = page
            .target
            .as_ref()
            .map(|target| target.badge().to_string())
            .unwrap_or_else(|| "—".to_string());
        let detail = page
            .target
            .as_ref()
            .map(|target| target.detail())
            .unwrap_or_default();

        let tabs: Vec<AnyElement> = self
            .composer_visible_pages()
            .into_iter()
            .enumerate()
            .map(|(index, draft)| {
                let active = draft.key == page.key;
                let title = draft.title();
                div()
                    .id(SharedString::from(format!("composer-tab-{index}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .bg(if active {
                        theme.active_bg
                    } else {
                        theme.chrome_bg
                    })
                    .text_color(if active { theme.fg } else { theme.muted })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.composer_select_visible(index, cx)),
                    )
                    .child(SharedString::from(title))
                    .into_any_element()
            })
            .collect();

        let status = self.composer_status.clone().unwrap_or_default();
        let body = if page.is_blank() {
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(SharedString::from("write a prompt, ⌘↩ sends"))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .text_sm()
                .text_color(theme.fg)
                .children(page.text.lines().map(|line| {
                    div()
                        .text_sm()
                        .child(SharedString::from(line.to_string()))
                        .into_any_element()
                }))
                .into_any_element()
        };

        Some(
            div()
                .id("composer-overlay")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .bg(gpui::rgba(0x00000080))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(width))
                        .h(px(height))
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.chrome_bg)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(theme.active_bg)
                                        .text_xs()
                                        .child(SharedString::from(badge)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted)
                                        .child(SharedString::from(detail)),
                                ),
                        )
                        .when(tabs.len() > 1, |element| {
                            element.child(div().flex().flex_row().gap_1().children(tabs))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .gap_1()
                                .overflow_hidden()
                                .child(body),
                        )
                        .when(!status.is_empty(), |element| {
                            element.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted)
                                    .child(SharedString::from(status)),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}

/// 画在浮层里的页面顺序（当前页在 visible 列表里的下标）。
pub fn active_visible_index(pages: &[ComposerDraftPage], active: &ComposerDraftPage) -> usize {
    pages
        .iter()
        .position(|page| page.key == active.key)
        .unwrap_or(0)
}
