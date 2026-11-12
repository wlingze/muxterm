//! 查询框内容渲染：选区高亮 + 可见光标。
//!
//! 命令面板（`Cmd-Shift-P`）与统一面板（`Cmd-P`）共用。以前只画一串文本：
//! 没有光标、看不出光标在哪、`Cmd-A` 全选也看不出来——输入体验很差。
//!
//! 光标位置必须和真实编辑器一致：它落在**两个字符之间**，所以渲染顺序是
//! `前面文字 → 光标 → 选区 → 后面文字`（光标在选区尾部时把光标放到选区之后）。
//! 空输入时光标在最前面，占位文案跟在光标右边 —— 用户报障截图里光标跑到
//! 占位文案末尾去了，看着像「已经输入了一段内容」。

use gpui::{div, prelude::*, px, AnyElement, SharedString};

use crate::frontend::gpui::model::text_input::TextInput;
use crate::frontend::gpui::theme::Theme;

/// 选区底色：强调色调淡，文字仍可读。
fn selection_bg(theme: &Theme) -> gpui::Rgba {
    let accent = theme.accent;
    gpui::Rgba {
        r: accent.r * 0.28 + theme.chrome_bg.r * 0.72,
        g: accent.g * 0.28 + theme.chrome_bg.g * 0.72,
        b: accent.b * 0.28 + theme.chrome_bg.b * 0.72,
        a: 1.0,
    }
}

/// 光标宽度与高度（细竖条，跟文本基线同高）。
const CARET_WIDTH: f32 = 1.5;
const CARET_HEIGHT: f32 = 15.0;

/// 画一行可编辑文本：前面文字 + 光标 + 选区 + 后面文字。
///
/// 没有文字时先画光标、再画占位文案（`muted`）：光标在行首，用户一眼就能
/// 看出「这是空输入框、可以打字」。
pub(crate) fn input_line(input: &TextInput, placeholder: &str, theme: &Theme) -> AnyElement {
    let text_color = if input.is_empty() {
        theme.muted
    } else {
        theme.label()
    };
    let caret = || {
        div()
            .debug_selector(|| "input-caret".to_string())
            .flex_none()
            .w(px(CARET_WIDTH))
            .h(px(CARET_HEIGHT))
            .bg(theme.accent)
            .into_any_element()
    };

    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .text_color(text_color);

    if input.is_empty() {
        // 光标在行首，占位文案跟在它后面 —— 不是反过来。
        row = row.child(caret());
        if !placeholder.is_empty() {
            row = row.child(
                div()
                    .debug_selector(|| "input-placeholder".to_string())
                    .child(SharedString::from(placeholder.to_string())),
            );
        }
        return row.into_any_element();
    }

    let (before, selected, after) = input.segments();
    let caret_after_selection = input
        .selection()
        .is_some_and(|_| !input.cursor_at_start_of_selection());
    if !before.is_empty() {
        row = row.child(SharedString::from(before));
    }
    if !caret_after_selection {
        row = row.child(caret());
    }
    if !selected.is_empty() {
        row = row.child(
            div()
                .debug_selector(|| "input-selection".to_string())
                .bg(selection_bg(theme))
                .child(SharedString::from(selected)),
        );
    }
    if caret_after_selection {
        row = row.child(caret());
    }
    if !after.is_empty() {
        row = row.child(SharedString::from(after));
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 光标与选区各有一个可断言的标记，测试才知道用户能不能看见它们。
    #[test]
    fn caret_and_selection_have_debug_selectors() {
        let theme = Theme::default();
        let mut input = TextInput::from_text("ab");
        input.select_all();
        assert!(input.selection().is_some(), "全选后必须有选区");
        // 渲染本身在 gpui 树里断言（见 gpui_chrome::palette_shows_a_caret）。
        let _ = input_line(&input, "", &theme);
    }
}
