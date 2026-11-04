//! PaneSurface 绘制：CellGrid → 行 → 同色 run。
//!
//! 一个 pane 一个常驻 Surface；隐藏时只跳过绘制，不销毁、不 reset。

use gpui::{
    canvas, div, prelude::*, px, AnyElement, ElementInputHandler, MouseButton, ScrollDelta,
    ScrollWheelEvent, SharedString,
};

use crate::frontend::gpui::chrome::pane_grid::Rect;
use crate::frontend::gpui::state::{GpuiApp, WheelInput};
use crate::frontend::gpui::terminal::grid::{Cell, Color, FILLER};

impl GpuiApp {
    pub(crate) fn render_pane(
        &self,
        pane_id: u32,
        rect: Rect,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme().clone();
        let Some(workspace) = self.visible_workspace().map(str::to_string) else {
            return div().into_any_element();
        };
        let cell_width = theme.cell_width();
        let cell_height = theme.cell_height();
        let cols = (rect.max_width() / cell_width).floor().max(1.0) as usize;
        let rows = (rect.max_height() / cell_height).floor().max(1.0) as usize;
        let focused = self.focused_pane() == Some(pane_id);

        let mut row_elements: Vec<AnyElement> = Vec::new();
        match self.surface(&workspace, pane_id) {
            Some(surface) => {
                let grid = surface.grid();
                let draw_rows = rows.min(grid.rows());
                let draw_cols = cols.min(grid.cols());
                let cursor = grid.cursor();
                // 滚到历史里就不画光标：live 光标不属于历史行。
                let show_cursor = focused && grid.cursor_visible() && surface.scroll_offset() == 0;
                for row in 0..draw_rows {
                    let Some(cells) = surface.visible_row(row) else {
                        break;
                    };
                    row_elements.push(Self::render_row(
                        &theme,
                        cells,
                        row,
                        draw_cols,
                        cursor,
                        show_cursor,
                    ));
                }
            }
            None => {
                row_elements.push(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(SharedString::from(format!("pane {pane_id}")))
                        .into_any_element(),
                );
            }
        }

        // IME：组合文本画在光标处，输入处理器负责候选窗锚定与提交。
        let marked = self.marked_text().to_string();
        if focused && !marked.is_empty() {
            if let Some((row, col, _)) = self.cursor_cell() {
                row_elements.push(
                    div()
                        .absolute()
                        .left(px(col as f32 * cell_width))
                        .top(px(row as f32 * cell_height))
                        .h(px(cell_height))
                        .text_color(theme.accent)
                        .underline()
                        .child(SharedString::from(marked))
                        .into_any_element(),
                );
            }
        }
        let input_handle = self.focus_handle();
        let entity = cx.entity();
        let input_handler = div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .child(canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    window.handle_input(
                        &input_handle,
                        ElementInputHandler::new(bounds, entity.clone()),
                        cx,
                    );
                },
            ));

        let border = if focused { theme.accent } else { theme.border };
        div()
            .id(SharedString::from(format!("pane-{pane_id}")))
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.max_width()))
            .h(px(rect.max_height()))
            .overflow_hidden()
            .bg(theme.bg)
            .border_1()
            .border_color(border)
            .font_family(theme.font_family)
            .text_size(px(theme.font_size))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus_handle());
                    this.focus_pane(pane_id, cx);
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
                    // 像素 delta 走触控板路径（先攒整格），行 delta 直接当格数。
                    let (delta_y, precise) = match event.delta {
                        ScrollDelta::Pixels(delta) => (f32::from(delta.y), true),
                        ScrollDelta::Lines(delta) => (delta.y, false),
                    };
                    let col =
                        ((f32::from(event.position.x) - rect.x) / cell_width).max(0.0) as usize;
                    let row =
                        ((f32::from(event.position.y) - rect.y) / cell_height).max(0.0) as usize;
                    this.handle_wheel(
                        WheelInput {
                            pane_id,
                            delta_y,
                            precise,
                            shift: event.modifiers.shift,
                            col,
                            row,
                        },
                        cx,
                    );
                }),
            )
            .children(row_elements)
            .child(input_handler)
            .into_any_element()
    }

    fn render_row(
        theme: &crate::frontend::gpui::theme::Theme,
        cells: &[Cell],
        row: usize,
        cols: usize,
        cursor: (usize, usize),
        show_cursor: bool,
    ) -> AnyElement {
        let cell_height = theme.cell_height();

        // 跳过尾部空白：既省 run，也不改变可见结果。
        let mut last = cols.min(cells.len());
        while last > 0 {
            let cell = &cells[last - 1];
            if cell.ch != ' ' || cell.bg != Color::Default || cell.attrs.reverse {
                break;
            }
            last -= 1;
        }

        let mut runs: Vec<AnyElement> = Vec::new();
        let mut index = 0;
        while index < last {
            let (fg, bg) = Self::cell_colors(&cells[index], theme, row, index, cursor, show_cursor);
            let mut text = String::new();
            while index < last {
                let (next_fg, next_bg) =
                    Self::cell_colors(&cells[index], theme, row, index, cursor, show_cursor);
                if (next_fg, next_bg) != (fg, bg) {
                    break;
                }
                // 双宽字形的第二格是占位：字形本身已经占了两格列宽。
                if cells[index].ch != FILLER {
                    text.push(cells[index].ch);
                }
                index += 1;
            }
            runs.push(
                div()
                    .h(px(cell_height))
                    .bg(bg)
                    .text_color(fg)
                    .child(SharedString::from(text))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_row()
            .h(px(cell_height))
            .children(runs)
            .into_any_element()
    }

    fn cell_colors(
        cell: &Cell,
        theme: &crate::frontend::gpui::theme::Theme,
        row: usize,
        col: usize,
        cursor: (usize, usize),
        show_cursor: bool,
    ) -> (gpui::Rgba, gpui::Rgba) {
        let (fg, bg) = cell.resolved();
        let mut fg = theme.foreground(fg);
        let mut bg = theme.background(bg);
        if show_cursor && cursor == (row, col) {
            std::mem::swap(&mut fg, &mut bg);
        }
        (fg, bg)
    }
}
