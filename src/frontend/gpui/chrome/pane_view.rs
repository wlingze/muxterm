//! PaneSurface 绘制：CellGrid → 行 → 同色 run。
//!
//! 一个 pane 一个常驻 Surface；隐藏时只跳过绘制，不销毁、不 reset。
//!
//! 文字**不能**交给普通文本流排版：终端每一列都是一个固定的格子，而字形
//! 的自然 advance 由字体决定（Menlo 的 `M`、CJK 的回退字体、粗斜体变体
//! 各不一样）。一旦整行交给 flex 拼接，中文/全角字符后面的竖线就会逐格
//! 漂移，表格边框对不齐。所以这里分两层画：
//!
//! * 背景 / 光标 / debug selector 用宽度写死的 div（一列 = 一个格子宽）；
//! * 字形用一层 canvas 按 `列号 × 格子宽` 显式定位，只取 `shape_line` 的
//!   glyph id 与字体，位置完全不用它给的 advance。

use std::sync::Arc;

use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, ElementInputHandler, FontStyle, FontWeight,
    Hsla, MouseButton, ScrollDelta, ScrollWheelEvent, SharedString, StrikethroughStyle, TextRun,
    UnderlineStyle,
};

use crate::frontend::gpui::chrome::pane_grid::Rect;
use crate::frontend::gpui::state::{GpuiApp, WheelInput};
use crate::frontend::gpui::terminal::grid::{Cell, Color, FILLER};

/// 一段样式相同的文本（同一行的连续格子）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowRun {
    pub start: usize,
    pub end: usize,
    /// 与 `text` 逐字符对应的终端列号。
    ///
    /// 不能靠 `start + 字符序号` 推出来：双宽字形占两列、隐藏格会把字形
    /// 换成空格但**不**改变列宽，序号和列号不是一回事。
    pub cols: Vec<usize>,
    pub text: String,
    pub fg: gpui::Rgba,
    pub bg: gpui::Rgba,
    pub style: RunStyle,
}

/// 一行待绘制的字形计划（字体与位置在 paint 阶段解析）。
#[derive(Debug, Clone)]
struct RowText {
    row: usize,
    runs: Vec<RowRun>,
}

/// 一个格子的字体变体（颜色之外要画出来的 SGR 属性）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RunStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl RunStyle {
    fn of(cell: &Cell) -> Self {
        Self {
            bold: cell.attrs.bold,
            italic: cell.attrs.italic,
            underline: cell.attrs.underline,
            strike: cell.attrs.strike,
        }
    }
}

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
        let (cell_width, cell_height) = self.cell();
        let cols = (rect.max_width() / cell_width).floor().max(1.0) as usize;
        let rows = (rect.max_height() / cell_height).floor().max(1.0) as usize;
        let focused = self.focused_pane() == Some(pane_id);

        let mut row_elements: Vec<AnyElement> = Vec::new();
        let mut row_texts: Vec<RowText> = Vec::new();
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
                    let runs = Self::row_runs(&theme, cells, row, draw_cols, cursor, show_cursor);
                    row_elements.push(Self::render_row(
                        &runs,
                        row,
                        cursor,
                        show_cursor,
                        cell_width,
                        cell_height,
                    ));
                    if runs.iter().any(|run| !run.text.is_empty()) {
                        row_texts.push(RowText { row, runs });
                    }
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

        // 字形的唯一绘制入口：按列号定位（见文件头）。
        let text_layer = {
            let rows = Arc::new(row_texts);
            let family = theme.font_family.clone();
            let font_size = theme.font_size;
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .child(canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _cx| {
                        paint_text(
                            bounds,
                            &rows,
                            &family,
                            font_size,
                            cell_width,
                            cell_height,
                            window,
                        );
                    },
                ))
                .into_any_element()
        };

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
            // gpui 默认 flex 方向是 row：不显式声明列，30 行会并排放并被压缩成 0 宽，
            // 于是整块终端看起来是空白。
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.bg)
            .border_1()
            .border_color(border)
            .font_family(theme.font_family)
            .text_size(px(theme.font_size))
            .line_height(px(cell_height))
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
            .child(text_layer)
            .child(input_handler)
            .into_any_element()
    }

    fn render_row(
        runs: &[RowRun],
        row: usize,
        cursor: (usize, usize),
        show_cursor: bool,
        cell_width: f32,
        cell_height: f32,
    ) -> AnyElement {
        let mut elements: Vec<AnyElement> = Vec::new();
        for run in runs {
            let holds_cursor =
                show_cursor && cursor.0 == row && (run.start..run.end).contains(&cursor.1);
            // 宽度必须写死成「列数 × 格宽」：交给内容撑宽就会被字形 advance 带走。
            let width = px((run.end - run.start) as f32 * cell_width);
            elements.push(
                div()
                    .h(px(cell_height))
                    .w(width)
                    .flex_none()
                    .bg(run.bg)
                    .when(holds_cursor, |this| {
                        this.debug_selector(|| "pane-cursor".into())
                    })
                    // 一个元素只能挂一个 debug selector：测试按行的属性分别断言。
                    .when(!holds_cursor && run.style.bold, |this| {
                        this.debug_selector(|| "pane-bold".into())
                    })
                    .when(
                        !holds_cursor && !run.style.bold && run.style.italic,
                        |this| this.debug_selector(|| "pane-italic".into()),
                    )
                    .when(
                        !holds_cursor
                            && !run.style.bold
                            && !run.style.italic
                            && run.style.underline,
                        |this| this.debug_selector(|| "pane-underline".into()),
                    )
                    .when(
                        !holds_cursor
                            && !run.style.bold
                            && !run.style.italic
                            && !run.style.underline
                            && run.style.strike,
                        |this| this.debug_selector(|| "pane-strike".into()),
                    )
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(cell_height))
            .w_full()
            .children(elements)
            .into_any_element()
    }

    /// 一行按「同样的颜色 + 同样的字体属性」切段。
    ///
    /// 纯函数：chore 绘制前就能单测粗体/斜体/隐藏格的切段与提亮结果。
    fn row_runs(
        theme: &crate::frontend::gpui::theme::Theme,
        cells: &[Cell],
        row: usize,
        cols: usize,
        cursor: (usize, usize),
        show_cursor: bool,
    ) -> Vec<RowRun> {
        let last = Self::last_visible_cell(cells, cols, row, cursor, show_cursor);
        let cursor_col = (show_cursor && cursor.0 == row).then_some(cursor.1);
        let mut runs: Vec<RowRun> = Vec::new();
        let mut index = 0;
        while index < last {
            let start = index;
            let style = RunStyle::of(&cells[index]);
            let (fg, bg) = Self::cell_colors(&cells[index], theme, row, index, cursor, show_cursor);
            let mut text = String::new();
            let mut cols: Vec<usize> = Vec::new();
            while index < last {
                // 光标格单独成段：反色后宽度必须正好一格，测试也按它定位。
                if Some(index) == cursor_col && index > start {
                    break;
                }
                let (next_fg, next_bg) =
                    Self::cell_colors(&cells[index], theme, row, index, cursor, show_cursor);
                if (next_fg, next_bg) != (fg, bg) || RunStyle::of(&cells[index]) != style {
                    break;
                }
                // SGR 8（隐藏）：保留格子宽度与背景，只把字形换成空格。
                if cells[index].attrs.hidden {
                    text.push(' ');
                    cols.push(index);
                } else if cells[index].ch != FILLER {
                    // 双宽字形的第二格是占位：字形本身已经占了两格列宽。
                    text.push(cells[index].ch);
                    cols.push(index);
                }
                index += 1;
            }
            runs.push(RowRun {
                start,
                end: index,
                cols,
                text,
                fg,
                bg,
                style,
            });
        }
        runs
    }

    /// 一行要画到第几格（不含）。
    ///
    /// 跳过尾部空白：既省 run，也不改变可见结果。**但光标格子必须保留**：
    /// shell 提示符后面通常是一个空格，把尾部空白剪掉等于把光标也剪掉了
    /// （用户看到的就是「没有光标」）。
    fn last_visible_cell(
        cells: &[Cell],
        cols: usize,
        row: usize,
        cursor: (usize, usize),
        show_cursor: bool,
    ) -> usize {
        let mut last = cols.min(cells.len());
        while last > 0 {
            let cell = &cells[last - 1];
            if cell.ch != ' ' || cell.bg != Color::Default || cell.attrs.reverse {
                break;
            }
            last -= 1;
        }
        if show_cursor && cursor.0 == row {
            last = last.max(cursor.1 + 1).min(cells.len());
        }
        last
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
        // 反显已经换过前景/背景：提亮必须作用在真正当前景的那个颜色上
        //（SwiftTerm 也是先 swap 再 mapColor(isFg:)）。
        let mut fg = theme.foreground_with_bold(fg, cell.attrs.bold);
        let mut bg = theme.background(bg);
        if cell.attrs.dim {
            fg = crate::frontend::gpui::theme::Theme::dimmed(fg, bg);
        }
        if show_cursor && cursor == (row, col) {
            std::mem::swap(&mut fg, &mut bg);
        }
        (fg, bg)
    }
}

/// 按终端列坐标逐字形绘制一整屏。
///
/// 只借用 `shape_line` 的 glyph id / font id：位置由 `列号 × 格子宽` 决定。
/// 这样中文回退字体、粗斜体、emoji 都不会改变列对齐。
fn paint_text(
    bounds: gpui::Bounds<gpui::Pixels>,
    rows: &[RowText],
    family: &SharedString,
    font_size: f32,
    cell_width: f32,
    cell_height: f32,
    window: &mut gpui::Window,
) {
    let text_system = window.text_system().clone();
    let size = px(font_size);
    // 基线只按主字体算一次：回退字体（CJK）自己的 ascent 不同，
    // 逐 run 取会让中英文基线错开。
    let base_id = text_system.resolve_font(&gpui::font(family.clone()));
    let ascent = text_system.ascent(base_id, size);
    // gpui 的 descent 是负值：行内内容高度 = ascent + |descent|。
    let descent = text_system.descent(base_id, size);
    let cell = px(cell_height);
    let top = ((cell - (ascent - descent)) / 2.0).max(px(0.0));
    let baseline = top + ascent;

    for row in rows {
        let row_top = bounds.origin.y + px(row.row as f32 * cell_height);
        for run in &row.runs {
            if run.text.is_empty() {
                continue;
            }
            let mut font = gpui::font(family.clone());
            if run.style.bold {
                font.weight = FontWeight::BOLD;
            }
            if run.style.italic {
                font.style = FontStyle::Italic;
            }
            let color = Hsla::from(run.fg);
            let shaped = text_system.shape_line(
                run.text.clone().into(),
                size,
                &[TextRun {
                    len: run.text.len(),
                    font,
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );

            let char_starts: Vec<usize> = run.text.char_indices().map(|(index, _)| index).collect();
            let mut char_index = 0usize;
            for shaped_run in &shaped.runs {
                for glyph in &shaped_run.glyphs {
                    while char_index + 1 < char_starts.len()
                        && char_starts[char_index + 1] <= glyph.index
                    {
                        char_index += 1;
                    }
                    let Some(column) = run.cols.get(char_index) else {
                        continue;
                    };
                    let origin = point(
                        bounds.origin.x + px(*column as f32 * cell_width),
                        row_top + baseline,
                    );
                    if glyph.is_emoji {
                        let _ = window.paint_emoji(origin, shaped_run.font_id, glyph.id, size);
                    } else {
                        let _ =
                            window.paint_glyph(origin, shaped_run.font_id, glyph.id, size, color);
                    }
                }
            }

            let left = bounds.origin.x + px(run.start as f32 * cell_width);
            let width = px((run.end - run.start) as f32 * cell_width);
            if run.style.underline {
                window.paint_underline(
                    point(left, row_top + baseline + descent * 0.618),
                    width,
                    &UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(color),
                        wavy: false,
                    },
                );
            }
            if run.style.strike {
                window.paint_strikethrough(
                    point(left, row_top + (ascent * 0.5 + baseline) * 0.5),
                    width,
                    &StrikethroughStyle {
                        thickness: px(1.0),
                        color: Some(color),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::terminal::grid::Attributes;

    fn cells(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|ch| Cell {
                ch,
                ..Cell::default()
            })
            .collect()
    }

    /// 回归：光标落在一个尾部空白格子上时，尾部空白裁剪不能把它一起裁掉。
    #[test]
    fn trailing_blank_trim_keeps_the_cursor_cell() {
        let line = cells("❯ ");
        assert_eq!(GpuiApp::last_visible_cell(&line, 80, 0, (0, 5), false), 1);
        assert_eq!(
            GpuiApp::last_visible_cell(&line, 80, 0, (0, 1), true),
            2,
            "光标在最后的空格上时这一格必须留着"
        );
        // 光标在别的行时不影响该行的裁剪。
        assert_eq!(GpuiApp::last_visible_cell(&line, 80, 3, (0, 1), true), 1);
    }

    /// 整行空白且没有光标：不画任何 run（省一次布局）。
    #[test]
    fn blank_rows_stay_empty_without_a_cursor() {
        let blank = cells("    ");
        assert_eq!(GpuiApp::last_visible_cell(&blank, 80, 0, (0, 0), false), 0);
        assert_eq!(GpuiApp::last_visible_cell(&blank, 80, 0, (0, 0), true), 1);
    }

    /// 带一个 SGR 属性的格子。
    fn styled(ch: char, attrs: Attributes) -> Cell {
        Cell {
            ch,
            attrs,
            ..Cell::default()
        }
    }

    fn runs(cells: &[Cell]) -> Vec<RowRun> {
        GpuiApp::row_runs(
            &crate::frontend::gpui::theme::Theme::default(),
            cells,
            0,
            cells.len(),
            (usize::MAX, usize::MAX),
            false,
        )
    }

    /// 粗体必须切开 run，并按 SwiftTerm 的规则把 ANSI 0–6 提亮到 8–14。
    #[test]
    fn bold_splits_the_run_and_brightens_the_ansi_color() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let bold = Attributes {
            bold: true,
            ..Attributes::default()
        };
        let line = [
            Cell {
                fg: Color::Indexed(1),
                ..styled('a', Attributes::default())
            },
            Cell {
                fg: Color::Indexed(1),
                ..styled('b', bold)
            },
            Cell {
                fg: Color::Indexed(1),
                ..styled('c', bold)
            },
        ];
        let runs = runs(&line);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(runs[0].text, "a");
        assert_eq!(runs[0].fg, theme.indexed(1));
        assert!(!runs[0].style.bold);
        assert_eq!(runs[1].text, "bc", "同样的属性合成一段");
        assert_eq!(runs[1].style.bold, true);
        assert_eq!(runs[1].fg, theme.indexed(9), "粗体把 ANSI 1 提亮成 9");
    }

    /// 斜体 / 下划线 / 删除线各自进入 run 的样式，不能被颜色合并吞掉。
    #[test]
    fn italic_underline_and_strike_reach_the_run_style() {
        let line = [
            styled(
                'i',
                Attributes {
                    italic: true,
                    ..Attributes::default()
                },
            ),
            styled(
                'u',
                Attributes {
                    underline: true,
                    ..Attributes::default()
                },
            ),
            styled(
                's',
                Attributes {
                    strike: true,
                    ..Attributes::default()
                },
            ),
        ];
        let runs = runs(&line);
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert!(runs[0].style.italic);
        assert!(runs[1].style.underline);
        assert!(runs[2].style.strike);
    }

    /// SGR 8（隐藏）：格子宽度与背景保留，字形换成空格。
    #[test]
    fn hidden_cells_keep_width_and_background_but_drop_the_glyph() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let line = [
            Cell {
                bg: Color::Indexed(4),
                ..styled(
                    'x',
                    Attributes {
                        hidden: true,
                        ..Attributes::default()
                    },
                )
            },
            Cell {
                bg: Color::Indexed(4),
                ..styled(
                    'y',
                    Attributes {
                        hidden: true,
                        ..Attributes::default()
                    },
                )
            },
        ];
        let runs = runs(&line);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "  ");
        assert_eq!(runs[0].bg, theme.indexed(4));
    }

    /// dim：前景往背景混合一半（SwiftTerm `dimmedColor(towards:)`）。
    #[test]
    fn dim_blends_the_foreground_towards_the_background() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let line = [Cell {
            fg: Color::Indexed(1),
            ..styled(
                'd',
                Attributes {
                    dim: true,
                    ..Attributes::default()
                },
            )
        }];
        let runs = runs(&line);
        assert_eq!(
            runs[0].fg,
            crate::frontend::gpui::theme::Theme::dimmed(theme.indexed(1), theme.bg)
        );
    }

    /// 反显 + 粗体：提亮作用在交换后的前景（SwiftTerm 先 swap 再 mapColor）。
    #[test]
    fn reverse_bold_brightens_the_swapped_foreground() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let line = [Cell {
            fg: Color::Default,
            bg: Color::Indexed(2),
            ..styled(
                'r',
                Attributes {
                    bold: true,
                    reverse: true,
                    ..Attributes::default()
                },
            )
        }];
        let runs = runs(&line);
        assert_eq!(
            runs[0].fg,
            theme.indexed(10),
            "反显后前景是 ANSI 2，粗体提到 10"
        );
        assert_eq!(runs[0].bg, theme.bg, "反显后背景是默认前景色");
    }

    /// 光标格反转颜色，但字体属性仍然保留。
    #[test]
    fn the_cursor_inverts_colors_and_keeps_the_style() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let line = [styled(
            'c',
            Attributes {
                bold: true,
                ..Attributes::default()
            },
        )];
        let runs = GpuiApp::row_runs(&theme, &line, 0, 1, (0, 0), true);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].fg, theme.bg, "光标格前景变背景色");
        assert_eq!(runs[0].bg, theme.fg, "光标格背景变前景色");
        assert!(runs[0].style.bold, "反色不能丢掉粗体");
    }
}
