//! 重命名输入浮层。
//!
//! gpui 0.2 没有现成的 TextInput，文本编辑沿用面板查询那条路径：按键在
//! `GpuiApp::handle_key` 里维护 `PromptState.value`，这里只负责画。
//! 对照 macOS `MainWindow.promptForName`（一个小输入框 + 回车确认）。
use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::info::INFO_WIDTH;
use crate::frontend::gpui::state::GpuiApp;

pub const PROMPT_WIDTH: f32 = INFO_WIDTH * 0.75;
pub const PROMPT_HEIGHT: f32 = 132.0;

impl GpuiApp {
    pub(crate) fn render_prompt(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let prompt = self.prompt()?;
        let theme = self.theme().clone();
        let title = prompt.title.clone();
        let value = prompt.value.clone();
        let error = prompt.error.clone();
        Some(
            div()
                .id("prompt-overlay")
                .debug_selector(|| "prompt-overlay".into())
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000033))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|app, _, _, cx| app.cancel_prompt(cx)),
                )
                .child(
                    div()
                        .id("prompt-panel")
                        .debug_selector(|| "prompt-panel".into())
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .w(px(PROMPT_WIDTH))
                        .h(px(PROMPT_HEIGHT))
                        .p(px(14.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.chrome_bg)
                        .text_size(px(13.0))
                        .text_color(theme.label())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(SharedString::from(title)),
                        )
                        .child(
                            div()
                                .id("prompt-input")
                                .debug_selector(|| "prompt-input".into())
                                .flex()
                                .items_center()
                                .h(px(28.0))
                                .px(px(8.0))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(if error.is_some() {
                                    theme.danger
                                } else {
                                    theme.border
                                })
                                .bg(theme.active_bg)
                                .child(SharedString::from(value))
                                .child(div().text_color(theme.accent).child("▏")),
                        )
                        .when_some(error, |this, message| {
                            this.child(
                                div()
                                    .debug_selector(|| "prompt-error".into())
                                    .text_size(px(11.0))
                                    .text_color(theme.danger)
                                    .child(SharedString::from(message)),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted)
                                .child("Enter to apply · Esc to cancel"),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// 输入框里显示的文本：值 + 光标。
pub fn input_text(value: &str) -> String {
    format!("{value}▏")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_text_keeps_a_trailing_caret_even_when_empty() {
        assert_eq!(input_text("dev"), "dev▏");
        assert_eq!(input_text(""), "▏");
    }
}
