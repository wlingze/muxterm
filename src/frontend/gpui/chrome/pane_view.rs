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
    Hsla, MouseButton, MouseMoveEvent, Pixels, ScrollDelta, ScrollWheelEvent, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle,
};

use crate::frontend::gpui::chrome::pane_grid::Rect;
use crate::frontend::gpui::state::{GpuiApp, WheelInput};
use crate::frontend::gpui::terminal::grid::{Cell, Color, FILLER};
use crate::frontend::gpui::terminal::mouse::{Button, MouseEventKind, MouseModifiers};

/// 选区背景混色比例（强调色叠在格子背景上的 alpha）。
pub const SELECTION_ALPHA: f32 = 0.3;

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
    /// 内容区在窗口里的左上原点（侧栏宽度 / 顶部 bar 高度）。
    /// 鼠标事件是窗口坐标，换算格子必须减掉它。
    pub(crate) fn render_pane(
        &self,
        pane_id: u32,
        rect: Rect,
        origin: (f32, f32),
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
        // 未聚焦 pane 压暗（对齐 iTerm2 `SplitPaneDimmingAmount`）。
        let visible_panes = self.visible_pane_count();
        let dimmed = crate::frontend::gpui::model::pane_dimming::should_dim(focused, visible_panes);
        let selection = self.selection_for(pane_id);

        let mut row_elements: Vec<AnyElement> = Vec::new();
        let mut row_texts: Vec<RowText> = Vec::new();
        if let Some(surface) = self.surface(&workspace, pane_id) {
            {
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
                    let runs = Self::row_runs(
                        &theme,
                        cells,
                        row,
                        draw_cols,
                        cursor,
                        show_cursor,
                        dimmed,
                        selection,
                    );
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
        }
        // 还没有 Surface（首帧）：保持空白底色，别往终端里塞
        // 「pane N」这种调试文字，用户看到会以为渲染坏了。

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
                // 终端内容盒的选择器：切焦点时它的几何必须一动不动。
                .debug_selector(move || format!("pane-text-{pane_id}"))
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
                    // 每个 pane 共用一个 focus handle；若所有 pane 都注册，
                    // 平台只保留最后绘制的那个，候选窗会锚到错误的 pane，
                    // 中文输入法看起来就是「候选窗跑掉 / 位置不对」。
                    if focused {
                        window.handle_input(
                            &input_handle,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                    }
                },
            ));

        // 多 pane 时浮一条标题条（不占终端格子，见 `model::pane_title`）。
        // 默认**隐藏**，只有鼠标停在 pane 顶部 22pt 或这个 pane 的菜单开着时
        // 才出现 —— 对照 Swift `updateTitleHover` / `showActionMenu`。
        let title_bar = if self.shows_pane_title(pane_id) {
            let title = crate::frontend::gpui::model::pane_title::display_title(
                &self.pane_title(pane_id).unwrap_or_default(),
                pane_id,
            );
            Some(
                div()
                    .id(SharedString::from(format!("pane-title-{pane_id}")))
                    .debug_selector(|| format!("pane-title-{pane_id}"))
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(
                        crate::frontend::gpui::model::pane_title::TITLE_BAR_HEIGHT,
                    ))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(7.0))
                    .bg(theme.chrome_bg)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(11.0))
                    .when(focused, |this| this.text_color(theme.accent))
                    .when(!focused, |this| this.text_color(theme.muted))
                    .child(div().overflow_hidden().child(SharedString::from(title)))
                    .child(
                        div()
                            .id(SharedString::from(format!("pane-actions-{pane_id}")))
                            .debug_selector(|| format!("pane-actions-{pane_id}"))
                            .cursor_pointer()
                            .text_color(theme.muted)
                            .px(px(4.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    // `⋯` 打开**这个 pane** 的动作菜单；以前这里直接
                                    // 切成水平分割，看起来就是「点菜单却新建了 pane」。
                                    this.open_pane_menu(pane_id, cx);
                                }),
                            )
                            .child(crate::frontend::gpui::chrome::icons::icon_element(
                                crate::frontend::gpui::chrome::icons::IconSpec::with_size(
                                    crate::frontend::gpui::chrome::icons::Icon::Ellipsis,
                                    theme.muted,
                                    12.0,
                                ),
                                12.0,
                            )),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        // 对照 Swift `PaneHostView`：只有活动 pane 画 1pt 强调色边框，
        // 非活动 pane 完全没有边框（相邻 pane 之间只剩 6pt 拖拽带）。
        // 之前每个 pane 都描一条灰边，看起来像格子表格，比 Swift 脏。
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
            .bg(if dimmed {
                crate::frontend::gpui::model::pane_dimming::dim(
                    theme.bg,
                    crate::frontend::gpui::model::pane_dimming::INACTIVE_PANE_DIM,
                )
            } else {
                theme.bg
            })
            .when(dimmed, |this| this.debug_selector(|| "pane-dimmed".into()))
            // 焦点只改光影：强调色描边画在覆盖层里，**不能**给终端容器加
            // `border_1` —— 边框会挤小内容盒（终端内容右移/下移 1pt、列数
            // 跟着变），切 pane 时看起来就是「pane 跳了一下」。
            .font_family(theme.font_family)
            .text_size(px(theme.font_size))
            .line_height(px(cell_height))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    pane_mouse_down(
                        this,
                        window,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Left,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    pane_mouse_down(
                        this,
                        window,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Right,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    pane_mouse_down(
                        this,
                        window,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Middle,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _window, cx| {
                    pane_mouse_up(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Left,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _window, cx| {
                    pane_mouse_up(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Right,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _window, cx| {
                    pane_mouse_up(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Middle,
                        event,
                        cx,
                    );
                }),
            )
            // 拖出 pane 再松手：`on_mouse_up` 只在指针还在本元素上时触发，
            // 少了这一条应用会永远以为鼠标还按着。
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _window, cx| {
                    pane_mouse_up(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Left,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _window, cx| {
                    pane_mouse_up(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Right,
                        event,
                        cx,
                    );
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _window, cx| {
                    pane_mouse_up(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        MouseButton::Middle,
                        event,
                        cx,
                    );
                }),
            )
            // 悬停只观察、不拦截：标题条默认隐藏，鼠标移到顶部 22pt 才出现。
            // 不能用一条透明 div 做“感应条”，那会吞掉终端首行的点击。
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    pane_mouse_move(
                        this,
                        pane_id,
                        PaneGeometry {
                            rect,
                            origin,
                            cell: (cell_width, cell_height),
                        },
                        event,
                        cx,
                    );
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.clear_pane_hover(pane_id, cx);
                    this.forget_mouse_motion(pane_id);
                }
            }))
            .on_scroll_wheel(
                cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
                    // 像素 delta 走触控板路径（先攒整格），行 delta 直接当格数。
                    let (delta_y, precise) = match event.delta {
                        ScrollDelta::Pixels(delta) => (f32::from(delta.y), true),
                        ScrollDelta::Lines(delta) => (delta.y, false),
                    };
                    // 坐标必须减掉窗口内容原点（侧栏 / 顶部 bar）和这一块
                    // pane 自己相对内容区的位置，否则右半边会把侧栏宽度算进列号。
                    let (col, row) =
                        pane_cell_at(rect, origin, cell_width, cell_height, event.position);
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
            .children(title_bar)
            .child(input_handler)
            .when(focused, |this| {
                this.child(
                    div()
                        .debug_selector(|| "pane-focus-ring".into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h_full()
                        .border_1()
                        .border_color(theme.accent),
                )
            })
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
    #[allow(clippy::too_many_arguments)]
    fn row_runs(
        theme: &crate::frontend::gpui::theme::Theme,
        cells: &[Cell],
        row: usize,
        cols: usize,
        cursor: (usize, usize),
        show_cursor: bool,
        dimmed: bool,
        selection: Option<crate::frontend::gpui::model::selection::Selection>,
    ) -> Vec<RowRun> {
        let selection_end = selection.and_then(|selection| {
            let (start, end) = selection.normalized();
            if row < start.row || row > end.row {
                return None;
            }
            // 中间行整行都有选区，不能只看 end.col。
            Some(if row < end.row {
                cols.saturating_sub(1)
            } else {
                end.col
            })
        });
        let last = Self::last_visible_cell(cells, cols, row, cursor, show_cursor, selection_end);
        let cursor_col = (show_cursor && cursor.0 == row).then_some(cursor.1);
        let mut runs: Vec<RowRun> = Vec::new();
        let mut index = 0;
        while index < last {
            let start = index;
            let style = RunStyle::of(&cells[index]);
            let (fg, bg) = Self::cell_colors(
                &cells[index],
                theme,
                row,
                index,
                cursor,
                show_cursor,
                dimmed,
                selection.is_some_and(|selection| selection.contains(row, index)),
            );
            let mut text = String::new();
            let mut cols: Vec<usize> = Vec::new();
            while index < last {
                // 光标格单独成段：反色后宽度必须正好一格，测试也按它定位。
                if Some(index) == cursor_col && index > start {
                    break;
                }
                let (next_fg, next_bg) = Self::cell_colors(
                    &cells[index],
                    theme,
                    row,
                    index,
                    cursor,
                    show_cursor,
                    dimmed,
                    selection.is_some_and(|selection| selection.contains(row, index)),
                );
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
        selection_end: Option<usize>,
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
        // 选区里的尾随空格也要画（否则选中一片空白看不到高亮）。
        if let Some(end) = selection_end {
            last = last.max(end + 1).min(cells.len());
        }
        last
    }

    #[allow(clippy::too_many_arguments)]
    fn cell_colors(
        cell: &Cell,
        theme: &crate::frontend::gpui::theme::Theme,
        row: usize,
        col: usize,
        cursor: (usize, usize),
        show_cursor: bool,
        dimmed: bool,
        selected: bool,
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
            // iTerm2 / Ghostty 的块光标：整格填主题 cursor 色，字形用原背景色。
            // 只做 fg/bg 交换的话，彩色格子上光标会被前景色染成一块同色，
            // 看起来像「没画光标」。
            let cell_bg = bg;
            fg = cell_bg;
            bg = theme.cursor;
        }
        if selected {
            // iTerm2 / Ghostty 的默认观感：前景不动，背景按强调色 30% 叠一层。
            bg = blend(bg, theme.accent, SELECTION_ALPHA);
        }
        if dimmed {
            use crate::frontend::gpui::model::pane_dimming;
            let amount = pane_dimming::INACTIVE_PANE_DIM;
            fg = pane_dimming::dim(fg, amount);
            bg = pane_dimming::dim(bg, amount);
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
        // 参照 gpui 自己的 LineLayout::paint：整行共用 paint layer，避免每个
        // 字形都独立插入 BoundsTree（真机采样热点：Bounds::intersects）。
        let row_bounds = gpui::Bounds::new(
            gpui::point(bounds.origin.x, row_top),
            gpui::size(bounds.size.width, cell),
        );
        window.paint_layer(row_bounds, |window| {
            for run in &row.runs {
                if run.text.is_empty() {
                    continue;
                }
                // 先画制表符 / 方块：它们不用字体字形，按格子几何对齐。
                paint_box_glyphs(bounds, row, run, cell_width, cell_height, window);
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

                let char_starts: Vec<usize> =
                    run.text.char_indices().map(|(index, _)| index).collect();
                let chars: Vec<char> = run.text.chars().collect();
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
                        // 制表 / 方块字符已由 `paint_box_glyphs` 画完，字体字形会
                        // 多画一层（错位、断线、方块锯齿都来自这里）。
                        if chars.get(char_index).is_some_and(|ch| {
                            crate::frontend::gpui::model::box_drawing::is_custom(*ch)
                        }) {
                            continue;
                        }
                        let origin = point(
                            bounds.origin.x + px(*column as f32 * cell_width),
                            row_top + baseline,
                        );
                        if glyph.is_emoji {
                            let _ = window.paint_emoji(origin, shaped_run.font_id, glyph.id, size);
                        } else {
                            let _ = window.paint_glyph(
                                origin,
                                shaped_run.font_id,
                                glyph.id,
                                size,
                                color,
                            );
                        }
                    }
                }

                let left = bounds.origin.x + px(run.start as f32 * cell_width);
                let width = px((run.end - run.start) as f32 * cell_width);
                let (underline_y, strike_y) = decoration_offsets(baseline, ascent, descent);
                if run.style.underline {
                    window.paint_underline(
                        point(left, row_top + underline_y),
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
                        point(left, row_top + strike_y),
                        width,
                        &StrikethroughStyle {
                            thickness: px(1.0),
                            color: Some(color),
                        },
                    );
                }
            }
        });
    }
}

/// 窗口坐标 → 视口内的格子坐标（越界夹到网格内）。
fn pane_cell_at(
    rect: Rect,
    origin: (f32, f32),
    cell_width: f32,
    cell_height: f32,
    position: gpui::Point<gpui::Pixels>,
) -> (usize, usize) {
    let x = f32::from(position.x) - origin.0 - rect.x;
    let y = f32::from(position.y) - origin.1 - rect.y;
    let col = (x / cell_width).floor().max(0.0) as usize;
    let row = (y / cell_height).floor().max(0.0) as usize;
    (col, row)
}

/// 一块 pane 的几何：内容区原点 + pane 自身位置 + 格子尺寸。
#[derive(Clone, Copy)]
struct PaneGeometry {
    rect: Rect,
    origin: (f32, f32),
    cell: (f32, f32),
}

impl PaneGeometry {
    fn cell_at(self, position: gpui::Point<gpui::Pixels>) -> (usize, usize) {
        pane_cell_at(self.rect, self.origin, self.cell.0, self.cell.1, position)
    }
}

/// gpui 的按键 → xterm 键号；导航键不参与终端鼠标上报。
fn mouse_button(button: MouseButton) -> Option<Button> {
    match button {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Middle => Some(Button::Middle),
        MouseButton::Right => Some(Button::Right),
        MouseButton::Navigate(_) => None,
    }
}

/// gpui 的修饰键 → xterm 的 4/8/16 位（Command 不转发，留给本机快捷键）。
fn mouse_modifiers(modifiers: &gpui::Modifiers) -> MouseModifiers {
    MouseModifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
}

/// 鼠标按下：先聚焦，再看应用是否接管（Shift 绕过）。
fn pane_mouse_down(
    app: &mut GpuiApp,
    window: &mut gpui::Window,
    pane_id: u32,
    geometry: PaneGeometry,
    button: MouseButton,
    event: &gpui::MouseDownEvent,
    cx: &mut gpui::Context<GpuiApp>,
) {
    window.focus(&app.focus_handle());
    app.focus_pane(pane_id, cx);
    // 点终端任意处都关掉 pane 菜单（NSMenu 点外面即消）。
    app.close_pane_menu(cx);
    let (col, row) = geometry.cell_at(event.position);
    // Command+点击 = 打开光标下的链接（macOS 惯例，对照 Swift
    // `requestOpenLink`）。放在鼠标上报之前：vim / tmux 开着鼠标时链接
    // 仍然要能点开。
    if button == MouseButton::Left && event.modifiers.platform {
        if let Some(url) = app.pane_url_at(pane_id, row, col) {
            cx.open_url(&url);
            return;
        }
    }
    let Some(protocol_button) = mouse_button(button) else {
        return;
    };
    let reported = app.report_mouse(
        pane_id,
        MouseEventKind::Press(protocol_button),
        col,
        row,
        mouse_modifiers(&event.modifiers),
    );
    if reported {
        app.mouse_press = Some((pane_id, protocol_button));
        return;
    }
    // 应用不要鼠标（或按住 Shift）：左键回到本地选字。
    // 不记 `mouse_press`：没上报的按下不能补一个释放给应用。
    if protocol_button == Button::Left {
        app.begin_selection(pane_id, row, col, event.click_count, cx);
    }
}

/// 鼠标释放：按键在哪块 pane 按下，就由哪块把释放送给应用。
fn pane_mouse_up(
    app: &mut GpuiApp,
    pane_id: u32,
    geometry: PaneGeometry,
    button: MouseButton,
    event: &gpui::MouseUpEvent,
    cx: &mut gpui::Context<GpuiApp>,
) {
    let Some(protocol_button) = mouse_button(button) else {
        return;
    };
    let (col, row) = geometry.cell_at(event.position);
    if app.mouse_press == Some((pane_id, protocol_button)) {
        app.report_mouse(
            pane_id,
            MouseEventKind::Release(protocol_button),
            col,
            row,
            mouse_modifiers(&event.modifiers),
        );
        app.mouse_press = None;
    }
    if protocol_button == Button::Left {
        // 松手后选区留着（可以继续 Cmd-C）；只结束拖拽状态。
        app.finish_drag(cx);
    }
}

/// 鼠标移动：标题条 hover、应用拖动上报（1002）、无按键悬停（1003）、
/// 以及应用没接管时的本地拖选。
fn pane_mouse_move(
    app: &mut GpuiApp,
    pane_id: u32,
    geometry: PaneGeometry,
    event: &MouseMoveEvent,
    cx: &mut gpui::Context<GpuiApp>,
) {
    // 标题条只看这一块 pane 自己的顶部，必须减掉 `rect.y`。
    let top = f32::from(event.position.y) - geometry.origin.1 - geometry.rect.y;
    app.note_pane_hover(
        pane_id,
        top <= crate::frontend::gpui::model::pane_title::TITLE_BAR_HEIGHT,
        cx,
    );
    let (col, row) = geometry.cell_at(event.position);
    let modifiers = mouse_modifiers(&event.modifiers);
    if let Some(button) = event.pressed_button.and_then(mouse_button) {
        // 应用拖动优先；应用没接管才继续本地选区。
        if !app.report_mouse(pane_id, MouseEventKind::Drag(button), col, row, modifiers)
            && button == Button::Left
        {
            app.extend_selection(pane_id, row, col, cx);
        }
    } else {
        // 无按键移动 = 1003 悬停；应用不接就什么都不做（标题条已经更新）。
        app.report_mouse(pane_id, MouseEventKind::Move, col, row, modifiers);
    }
}

/// 制表符 / 方块元素：按格子几何画矩形（iTerm2 `iTermBoxDrawingRenderer` 同思路）。
///
/// 位置完全由「列号 × 格宽」决定，端点贴格边，所以相邻格一定接得上；
/// 中文 / 双宽字符夹在表格里也不会把边框推歪。
fn paint_box_glyphs(
    bounds: gpui::Bounds<gpui::Pixels>,
    row: &RowText,
    run: &RowRun,
    cell_width: f32,
    cell_height: f32,
    window: &mut gpui::Window,
) {
    use crate::frontend::gpui::model::box_drawing::{shapes, Cell};
    let cell = Cell {
        width: cell_width,
        height: cell_height,
    };
    let row_top = bounds.origin.y + px(row.row as f32 * cell_height);
    for (text_index, ch) in run.text.chars().enumerate() {
        let Some(rects) = shapes(ch, cell) else {
            continue;
        };
        let Some(column) = run.cols.get(text_index) else {
            continue;
        };
        let cell_left = bounds.origin.x + px(*column as f32 * cell_width);
        for rect in rects {
            let color = if rect.alpha >= 1.0 {
                run.fg
            } else {
                // `░▒▓`：前景按透明度叠在格子背景上。
                blend(run.bg, run.fg, rect.alpha)
            };
            window.paint_quad(gpui::fill(
                gpui::Bounds::new(
                    gpui::point(cell_left + px(rect.x), row_top + px(rect.y)),
                    gpui::size(px(rect.w), px(rect.h)),
                ),
                color,
            ));
        }
    }
}

/// 线性混色：`t=0` 取 `from`，`t=1` 取 `to`。
fn blend(from: gpui::Rgba, to: gpui::Rgba, t: f32) -> gpui::Rgba {
    let t = t.clamp(0.0, 1.0);
    gpui::Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: 1.0,
    }
}

/// 下划线与删除线的纵坐标（相对行顶）。
///
/// `TextSystem::descent()` 返回的是**负值**（基线以下为负）。gpui 自己那段
/// `baseline + descent * 0.618` 用的是 `LineLayout` 里已经翻正的 descent，直接
/// 照搬到我们这里会把下划线画到基线**上方**、横穿字形（用户看到的就是这个）。
/// 所以这里显式取绝对值：下划线在基线下，删除线在基线稍上。
fn decoration_offsets(baseline: Pixels, ascent: Pixels, descent: Pixels) -> (Pixels, Pixels) {
    let below = -descent;
    let underline = baseline + below * 0.618;
    let strike = (ascent * 0.5 + baseline) * 0.5;
    (underline, strike)
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
        assert_eq!(
            GpuiApp::last_visible_cell(&line, 80, 0, (0, 5), false, None),
            1
        );
        assert_eq!(
            GpuiApp::last_visible_cell(&line, 80, 0, (0, 1), true, None),
            2,
            "光标在最后的空格上时这一格必须留着"
        );
        // 光标在别的行时不影响该行的裁剪。
        assert_eq!(
            GpuiApp::last_visible_cell(&line, 80, 3, (0, 1), true, None),
            1
        );
    }

    /// 整行空白且没有光标：不画任何 run（省一次布局）。
    #[test]
    fn blank_rows_stay_empty_without_a_cursor() {
        let blank = cells("    ");
        assert_eq!(
            GpuiApp::last_visible_cell(&blank, 80, 0, (0, 0), false, None),
            0
        );
        assert_eq!(
            GpuiApp::last_visible_cell(&blank, 80, 0, (0, 0), true, None),
            1
        );
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
            false,
            None,
        )
    }

    /// 光标格：整格用主题 cursor 色，字形颜色退回格子背景色。
    ///
    /// 对照 iTerm2 `iTermCursorRenderer`：光标是实心块，不是「颜色互换」；
    /// 互换会让彩色格子的光标和文字同色，看起来像没画。
    #[test]
    fn the_cursor_cell_uses_the_theme_cursor_color() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let cell = Cell::default();
        let (fg, bg) = GpuiApp::cell_colors(&cell, &theme, 0, 3, (0, 3), true, false, false);
        assert_eq!(bg, theme.cursor, "光标格背景 = 主题 cursor 色");
        assert_eq!(fg, theme.bg, "字形用格子背景色，才看得见");
    }

    /// 非光标格不受影响。
    #[test]
    fn ordinary_cells_keep_their_own_colors() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let cell = Cell::default();
        let (fg, bg) = GpuiApp::cell_colors(&cell, &theme, 0, 3, (0, 4), true, false, false);
        assert_eq!(bg, theme.bg);
        assert_eq!(fg, theme.fg);
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
    fn the_cursor_block_keeps_the_run_style() {
        let theme = crate::frontend::gpui::theme::Theme::default();
        let line = [styled(
            'c',
            Attributes {
                bold: true,
                ..Attributes::default()
            },
        )];
        let runs = GpuiApp::row_runs(&theme, &line, 0, 1, (0, 0), true, false, None);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].fg, theme.bg, "字号用格子背景色");
        assert_eq!(runs[0].bg, theme.cursor, "整格填 cursor 色");
        assert!(runs[0].style.bold, "反色不能丢掉粗体");
    }

    /// 跨行选区的**中间行**整行高亮：run 必须延伸到最后一列。
    ///
    /// 回归：曾经只用 `end.col` 算可见范围，跨行选区中间行只画到末行的列号，
    /// 看起来像高亮被截断。
    #[test]
    fn middle_rows_of_a_selection_cover_every_column() {
        use crate::frontend::gpui::model::selection::{CellPos, Kind, Selection};
        let theme = crate::frontend::gpui::theme::Theme::default();
        let cells = vec![Cell::default(); 12];
        let selection = Selection::new(CellPos::new(0, 8), CellPos::new(2, 2), Kind::Simple);
        let runs = GpuiApp::row_runs(&theme, &cells, 1, 12, (0, 0), false, false, Some(selection));
        // `end` 是开区间：12 表示画到第 11 列。
        assert_eq!(runs.last().map(|run| run.end), Some(12), "中间行整行都要画");
    }

    /// 选区必须真的改变格子背景（不是只记状态不画）。
    #[test]
    fn selected_cells_get_a_highlight_background() {
        use crate::frontend::gpui::model::selection::{CellPos, Kind, Selection};
        let theme = crate::frontend::gpui::theme::Theme::default();
        let cell = Cell::default();
        let plain = GpuiApp::cell_colors(&cell, &theme, 0, 3, (9, 9), false, false, false);
        let selected = GpuiApp::cell_colors(&cell, &theme, 0, 3, (9, 9), false, false, true);
        assert_eq!(plain.0, selected.0, "前景不变");
        assert_ne!(plain.1, selected.1, "背景要叠强调色");

        let selection = Selection::new(CellPos::new(1, 2), CellPos::new(1, 8), Kind::Simple);
        let cells = vec![Cell::default(); 12];
        let runs = GpuiApp::row_runs(&theme, &cells, 1, 12, (0, 0), false, false, Some(selection));
        assert!(
            runs.iter().any(|run| run.bg == selected.1),
            "选区覆盖的 run 用高亮背景"
        );
    }
}

#[cfg(test)]
mod decoration_tests {
    use super::decoration_offsets;
    use gpui::px;

    /// 下划线必须在基线**下方**、删除线在基线**上方**。
    ///
    /// 回归：`TextSystem::descent()` 是负值，直接 `baseline + descent * 0.618`
    /// 会把下划线画到字形中间（截图里的「下划线穿过文字」）。
    #[test]
    fn underline_sits_below_the_baseline_and_strike_above() {
        let baseline = px(15.0);
        let ascent = px(13.7);
        // gpui 的 descent 是负值。
        let descent = px(-3.9);
        let (underline, strike) = decoration_offsets(baseline, ascent, descent);
        assert!(
            underline > baseline,
            "下划线要在基线下：underline={underline:?} baseline={baseline:?}"
        );
        assert!(
            underline < baseline + (-descent),
            "下划线不能掉出格子：underline={underline:?}"
        );
        assert!(
            strike < baseline && strike > baseline - ascent,
            "删除线要在字形中部：strike={strike:?}"
        );
    }
}
