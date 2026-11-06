//! Cmd-Shift-P 命令面板的渲染：居中浮层 + 搜索行 + 命令列表。
//!
//! 行内容来自 `palette_view`（纯投影）。尺寸与 macOS `CommandPaletteController`
//! 一致：600×360、默认选中第一行、Esc 关闭、上下键与 Ctrl-N/P 移动。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::palette_view::{self, PaletteItem};
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

const PALETTE_WIDTH: f32 = 600.0;
const PALETTE_HEIGHT: f32 = 360.0;
const MAX_VISIBLE_ROWS: usize = 11;
const ROW_HEIGHT: f32 = 26.0;

impl GpuiApp {
    /// 命令面板浮层；没打开时返回空元素。
    pub(crate) fn render_palette(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.palette.open {
            return None;
        }
        let theme = self.theme().clone();
        let items = self.palette_items();
        let visible = palette_view::filter(&items, &self.palette.query);
        let selected = self.palette.selected;

        let rows: Vec<AnyElement> = visible
            .iter()
            .take(MAX_VISIBLE_ROWS)
            .enumerate()
            .map(|(index, item)| palette_row(item, index == selected, &theme, cx))
            .collect();

        let empty = if rows.is_empty() {
            div()
                .px(px(12.0))
                .py(px(8.0))
                .text_size(px(12.0))
                .text_color(theme.muted)
                .child(SharedString::from("No matching commands"))
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // 查询为空时显示占位文案，与 `NSSearchField.placeholderString` 一致。
        let query_text = if self.palette.query.is_empty() {
            "Type a command or search..."
        } else {
            self.palette.query.as_str()
        };
        let query_color = if self.palette.query.is_empty() {
            theme.muted
        } else {
            theme.label()
        };

        Some(
            div()
                .id("palette-overlay")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .debug_selector(|| "palette-overlay".to_string())
                .bg(gpui::rgba(0x00000033))
                .flex()
                .items_start()
                .justify_center()
                .pt(px(96.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_palette(cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .debug_selector(|| "palette-panel".to_string())
                        .w(px(PALETTE_WIDTH))
                        .h(px(PALETTE_HEIGHT))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.chrome_bg)
                        .shadow_lg()
                        .overflow_hidden()
                        // 浮层内部点击不关闭。
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .h(px(36.0))
                                .px(px(12.0))
                                .border_b_1()
                                .border_color(theme.border)
                                .text_size(px(13.0))
                                .text_color(query_color)
                                .child(SharedString::from(query_text.to_string())),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .overflow_hidden()
                                .children(rows)
                                .child(empty),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn palette_row(
    item: &PaletteItem,
    selected: bool,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let action = item.action;
    // macOS 选中行：强调色底 + 白字。目录里没有灰掉的占位项。
    let (bg, title_color, detail_color) = if selected {
        (theme.accent, gpui::rgb(0xffffff), gpui::rgba(0xffffffbf))
    } else {
        (theme.chrome_bg, theme.label(), theme.muted)
    };

    div()
        .id(SharedString::from(format!("palette-row-{}", item.title)))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .h(px(ROW_HEIGHT))
        .px(px(12.0))
        .bg(bg)
        .text_size(px(12.0))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| this.run_palette_action(action, window, cx)),
        )
        .child(
            div()
                .flex_none()
                .w(px(200.0))
                .overflow_hidden()
                .text_color(title_color)
                .child(SharedString::from(item.title.clone())),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_color(detail_color)
                .child(SharedString::from(item.detail.clone())),
        )
        .into_any_element()
}
