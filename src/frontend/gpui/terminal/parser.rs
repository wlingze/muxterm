//! vte::Perform 实现：把 Core 给的原始字节写进 [`Grid`]。
//!
//! 输入永远是 `PaneOutput` 字节流，不是 Index 的 ANSI dump（`SURFACE.md` §3 定律 2、9）。

use vte::{Params, Perform};

use super::grid::{char_width, Cell, FILLER};

use super::grid::Grid;
use crate::frontend::gpui::model::scroll;

/// 一个 pane 的常驻 Surface：解析器 + 网格。
///
/// 解析器与网格分开存放，避免 `advance` 期间重复借用 `self`。
pub struct Surface {
    parser: vte::Parser,
    grid: Grid,
    /// 视口上移的历史行数：0 表示贴着 live 屏幕。
    ///
    /// 只影响绘制，不改网格：抢 live 屏幕会把 Cursor/htop 的 alternate
    /// screen 状态清掉（`model::scroll::should_replace_live_screen` 恒 false）。
    scroll_offset: u32,
}

impl Surface {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            grid: Grid::new(cols, rows),
            scroll_offset: 0,
        }
    }

    /// 当前视口上移了多少历史行。
    pub fn scroll_offset(&self) -> u32 {
        self.scroll_offset
    }

    /// 能上移的上限：现有历史行数。
    pub fn max_scroll_offset(&self) -> u32 {
        u32::try_from(self.grid.scrollback_len()).unwrap_or(u32::MAX)
    }

    /// 按行滚动视口（正数往历史上滚）。返回是否真的移动了。
    pub fn scroll_by(&mut self, lines: i32) -> bool {
        let next = scroll::next_offset(self.scroll_offset, lines, self.max_scroll_offset());
        if next == self.scroll_offset {
            return false;
        }
        self.scroll_offset = next;
        true
    }

    /// 追加 Core 回填的历史行（`PaneHistory`，纯文本、SGR 已剥离）。
    ///
    /// 历史行比现有所有 scrollback 都老，所以插在头部；视口偏移不用改，
    /// `visible_row` 的内容保持不动（偏移是相对 live 屏幕的）。
    /// 返回真正推入的行数。
    pub fn feed_history(&mut self, data: &[u8]) -> usize {
        if data.is_empty() {
            return 0;
        }
        let cols = self.grid.cols();
        let text = String::from_utf8_lossy(data);
        let mut rows = Vec::new();
        for line in text.split('\n') {
            // `capture-pane` 的行不带 `\r`，历史也可能从 CRLF 文本来。
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut row = vec![
                Cell {
                    ch: ' ',
                    ..Cell::default()
                };
                cols
            ];
            let mut col = 0;
            for ch in line.chars() {
                let width = char_width(ch);
                // 组合标记/零宽字符没有独立格子：历史行按纯文本处理，直接跳过。
                if width == 0 {
                    continue;
                }
                if col + width > cols {
                    break;
                }
                row[col] = Cell {
                    ch,
                    ..Cell::default()
                };
                if width == 2 {
                    row[col + 1] = Cell {
                        ch: FILLER,
                        ..Cell::default()
                    };
                }
                col += width;
            }
            rows.push(row);
        }
        let added = rows.len();
        let dropped = self.grid.prepend_history(rows);
        if dropped > 0 {
            // 上限裁掉了最老的行：视口跟着往上补，内容不动。
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(u32::try_from(dropped).unwrap_or(u32::MAX));
        }
        added
    }

    /// 回到 live 屏幕底部。
    pub fn scroll_to_bottom(&mut self) -> bool {
        if self.scroll_offset == 0 {
            return false;
        }
        self.scroll_offset = 0;
        true
    }

    /// 视口里第 `row` 行（0 基，按网格行数算）。
    ///
    /// 上移之后，前 `min(offset, rows)` 行来自 scrollback，其余是 live 屏幕。
    pub fn visible_row(&self, row: usize) -> Option<&[Cell]> {
        let rows = self.grid.rows();
        if row >= rows {
            return None;
        }
        let offset = usize::try_from(self.scroll_offset).unwrap_or(usize::MAX);
        let covered = offset.min(rows);
        if row < covered {
            // scrollback 的 0 是最老的一行；贴着 live 屏幕的是 `len - 1`。
            let index = self.grid.scrollback_len() + row - offset;
            return self.grid.scrollback_row(index);
        }
        Some(self.grid.row(row - covered))
    }

    /// 视口里第 `(row, col)` 格所在链接的列范围（首尾都含）。
    ///
    /// OSC 8 链接按 `Cell::link` 的 id 向两侧扩张；行内 http(s) 文本交给
    /// `model::links::url_span_at`。Command+悬停只画这条范围。
    pub fn url_span_at(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        let cells = self.visible_row(row)?;
        if let Some(cell) = cells.get(col) {
            if cell.link != 0 {
                let mut start = col;
                while start > 0 && cells[start - 1].link == cell.link {
                    start -= 1;
                }
                let mut end = col;
                while end + 1 < cells.len() && cells[end + 1].link == cell.link {
                    end += 1;
                }
                return Some((start, end));
            }
        }
        let line: Vec<char> = cells
            .iter()
            .map(|cell| if cell.ch == FILLER { ' ' } else { cell.ch })
            .collect();
        crate::frontend::gpui::model::links::url_span_at(&line, col)
    }

    /// 视口里第 `(row, col)` 格上的链接：OSC 8 超链接优先，其次行内
    /// `http(s)://` 文本。返回值直接交给系统浏览器。
    pub fn url_at(&self, row: usize, col: usize) -> Option<String> {
        let cells = self.visible_row(row)?;
        if let Some(cell) = cells.get(col) {
            if let Some(link) = self.grid.link_of(cell) {
                return Some(link.to_string());
            }
        }
        let line: Vec<char> = cells
            .iter()
            .map(|cell| if cell.ch == FILLER { ' ' } else { cell.ch })
            .collect();
        crate::frontend::gpui::model::links::url_at(&line, col)
    }

    /// feed 原始字节。禁止用 `reset` 追帧。
    pub fn feed(&mut self, bytes: &[u8]) {
        // vte 0.13 的 advance 只吃单字节；解析状态保存在 parser 里，逐字节推进。
        for byte in bytes {
            self.parser.advance(&mut self.grid, *byte);
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }

    /// 权威 baseline：只有新 Surface 或明确的 full frame 才允许重置后整体 feed。
    pub fn feed_full(&mut self, bytes: &[u8]) {
        let cols = self.grid.cols();
        let rows = self.grid.rows();
        self.grid = Grid::new(cols, rows);
        self.feed(bytes);
    }
}

impl Perform for Grid {
    fn print(&mut self, c: char) {
        Grid::print(self, c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {}
            0x08 => self.backspace(),
            0x09 => self.horizontal_tab(),
            0x0a..=0x0c => self.line_feed(),
            0x0d => self.carriage_return(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let values: Vec<u16> = params.iter().flatten().copied().collect();
        let first = values.first().copied().unwrap_or(0);
        let private = intermediates.first() == Some(&b'?');
        let arg = |index: usize| values.get(index).copied().unwrap_or(0).max(1) as usize;

        match action {
            'A' => self.move_cursor(-(arg(0) as isize), 0),
            'B' | 'e' => self.move_cursor(arg(0) as isize, 0),
            'C' | 'a' => self.move_cursor(0, arg(0) as isize),
            'D' => self.move_cursor(0, -(arg(0) as isize)),
            'E' => {
                self.move_cursor(arg(0) as isize, 0);
                self.carriage_return();
            }
            'F' => {
                self.move_cursor(-(arg(0) as isize), 0);
                self.carriage_return();
            }
            'G' | '`' => self.set_cursor(self.cursor().0, arg(0) - 1),
            'd' => self.set_cursor(arg(0) - 1, self.cursor().1),
            'H' | 'f' => self.set_cursor(arg(0) - 1, arg(1) - 1),
            'J' => self.erase_display(first),
            'K' => self.erase_line(first),
            'L' => self.insert_lines(arg(0)),
            'M' => self.delete_lines(arg(0)),
            'P' => self.delete_chars(arg(0)),
            '@' => self.insert_chars(arg(0)),
            'X' => self.erase_chars(arg(0)),
            'S' => self.scroll_up(arg(0)),
            'T' => self.scroll_down(arg(0)),
            'r' => self.set_scroll_region(
                first.saturating_sub(1) as usize,
                values.get(1).copied().unwrap_or(0).saturating_sub(1) as usize,
            ),
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'm' => self.apply_sgr(&values),
            'h' if private => {
                for value in &values {
                    self.set_private_mode(*value, true);
                }
            }
            'l' if private => {
                for value in &values {
                    self.set_private_mode(*value, false);
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.line_feed(),
            b'E' => {
                self.carriage_return();
                self.line_feed();
            }
            b'M' => self.reverse_index(),
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() < 2 {
            return;
        }
        if params[0] == b"8" {
            // `OSC 8 ; params ; URI ST`：URI 为空表示结束当前链接。
            // Command+点击按格子去找这条 URI（`Surface::url_at`）。
            let uri = params.get(2).copied().unwrap_or_default();
            let uri = String::from_utf8_lossy(uri).into_owned();
            self.set_link(if uri.is_empty() { None } else { Some(&uri) });
            return;
        }
        if params[0] == b"0" || params[0] == b"2" {
            let title = String::from_utf8_lossy(params[1]).into_owned();
            self.set_title(title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::terminal::mouse::MouseMode;

    fn surface(cols: usize, rows: usize) -> Surface {
        Surface::new(cols, rows)
    }

    fn text(grid: &Grid, row: usize) -> String {
        grid.row(row).iter().map(|cell| cell.ch).collect()
    }

    /// 视口一行去掉尾部空白（宽字符的 FILLER 也一并去掉）。
    fn line(cells: Option<&[Cell]>) -> String {
        cells
            .unwrap_or(&[])
            .iter()
            .map(|cell| if cell.ch == FILLER { ' ' } else { cell.ch })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn plain_text_and_crlf() {
        let mut surface = surface(10, 3);
        surface.feed(b"hello\r\nworld");
        assert_eq!(text(surface.grid(), 0), "hello     ");
        assert_eq!(text(surface.grid(), 1), "world     ");
    }

    #[test]
    fn cursor_position_and_erase() {
        let mut surface = surface(10, 3);
        surface.feed(b"abc\x1b[2J\x1b[1;1Hxyz");
        assert_eq!(text(surface.grid(), 0), "xyz       ");
    }

    #[test]
    fn sgr_foreground_applies_to_cells() {
        let mut surface = surface(10, 2);
        surface.feed(b"\x1b[31mred\x1b[0m plain");
        let grid = surface.grid();
        assert_eq!(
            grid.cell(0, 0).unwrap().fg,
            super::super::grid::Color::Indexed(1)
        );
        assert_eq!(
            grid.cell(0, 3).unwrap().fg,
            super::super::grid::Color::Default
        );
    }

    #[test]
    fn truecolor_and_256_colors() {
        let mut surface = surface(10, 2);
        surface.feed(b"\x1b[38;2;1;2;3mA\x1b[48;5;200mB");
        let grid = surface.grid();
        assert_eq!(
            grid.cell(0, 0).unwrap().fg,
            super::super::grid::Color::Rgb(1, 2, 3)
        );
        assert_eq!(
            grid.cell(0, 1).unwrap().bg,
            super::super::grid::Color::Indexed(200)
        );
    }

    #[test]
    fn utf8_text_decodes() {
        let mut surface = surface(10, 2);
        surface.feed("你好".as_bytes());
        // CJK 占两格：第二个格子是占位，真正的字在第三格。
        assert_eq!(surface.grid().cell(0, 0).unwrap().ch, '你');
        assert_eq!(
            surface.grid().cell(0, 1).unwrap().ch,
            crate::frontend::gpui::terminal::grid::FILLER
        );
        assert_eq!(surface.grid().cell(0, 2).unwrap().ch, '好');
    }

    /// 历史为空时滚不动，视口也不会变成负数。
    #[test]
    fn scrollback_viewport_starts_at_the_live_screen() {
        let mut surface = surface(10, 3);
        assert_eq!(surface.scroll_offset(), 0);
        assert_eq!(surface.max_scroll_offset(), 0);
        assert!(!surface.scroll_by(5), "没有历史就不该动");
        assert!(!surface.scroll_to_bottom(), "already at the bottom");
    }

    /// 往历史上滚时，视口顶行来自 scrollback；回到 0 就恢复 live 屏幕。
    #[test]
    fn scrolling_up_shows_history_rows_above_the_live_screen() {
        let mut surface = surface(4, 2);
        surface.feed(b"a\r\nb\r\nc\r\nd");
        // 2 行屏：a/b 已经被顶进历史，屏幕上是 c/d。
        assert_eq!(surface.max_scroll_offset(), 2);
        assert!(surface.scroll_by(1));
        assert_eq!(surface.scroll_offset(), 1);
        let top = surface.visible_row(0).unwrap();
        let top: String = top.iter().map(|cell| cell.ch).collect();
        assert_eq!(top.trim_end(), "b");
        let bottom: String = surface
            .visible_row(1)
            .unwrap()
            .iter()
            .map(|cell| cell.ch)
            .collect();
        assert_eq!(bottom.trim_end(), "c");

        assert!(surface.scroll_to_bottom());
        let live: String = surface
            .visible_row(0)
            .unwrap()
            .iter()
            .map(|cell| cell.ch)
            .collect();
        assert_eq!(live.trim_end(), "c");
    }

    /// 偏移量超过一屏时，整个视口都落在历史里。
    #[test]
    fn deep_offset_fills_the_whole_viewport_from_history() {
        let mut surface = surface(4, 2);
        surface.feed(b"a\r\nb\r\nc\r\nd\r\ne\r\nf");
        assert_eq!(surface.max_scroll_offset(), 4);
        assert!(surface.scroll_by(4));
        assert_eq!(surface.scroll_offset(), 4);
        let rows: Vec<String> = (0..2)
            .map(|row| {
                surface
                    .visible_row(row)
                    .unwrap()
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        assert_eq!(rows, vec!["a".to_string(), "b".to_string()]);
        // 超出网格行数没有内容。
        assert!(surface.visible_row(2).is_none());
    }

    /// 偏移量被夹在历史上限，不会因为惯性滚动而越界。
    #[test]
    fn fed_history_lands_above_the_live_screen() {
        let mut surface = Surface::new(10, 2);
        surface.feed(b"live\r\nonly");
        assert_eq!(surface.feed_history(b"older-1\nolder-2"), 2);
        assert_eq!(surface.grid().scrollback_len(), 2);
        // 视口还在 live 屏幕：历史不抢焦点。
        assert_eq!(surface.scroll_offset(), 0);
        assert_eq!(line(surface.visible_row(0)), "live");
        // 上滚两行看到历史。
        assert!(surface.scroll_by(2));
        assert_eq!(line(surface.visible_row(0)), "older-1");
        assert_eq!(line(surface.visible_row(1)), "older-2");
    }

    #[test]
    fn fed_history_pads_to_the_grid_width_and_marks_wide_cells() {
        let mut surface = Surface::new(6, 1);
        surface.feed_history("宽字".as_bytes());
        let row = surface.grid().scrollback_row(0).expect("历史行");
        assert_eq!(row.len(), 6);
        assert_eq!(row[0].ch, '宽');
        assert_eq!(row[1].ch, FILLER);
        assert_eq!(row[2].ch, '字');
        assert_eq!(row[3].ch, FILLER);
        // 尾部补齐空格，render_row 的尾空白裁剪才有意义。
        assert_eq!(row[4].ch, ' ');
        assert_eq!(row[5].ch, ' ');
    }

    #[test]
    fn history_feed_keeps_the_viewport_on_the_same_row() {
        let mut surface = Surface::new(8, 2);
        surface.feed(b"a\r\nb\r\nc\r\nd");
        // 把视口顶到最老的历史行。
        assert!(surface.scroll_by(10));
        let scrolled_top = line(surface.visible_row(0));
        // 又回填一批更老的历史：视口内容必须不动。
        surface.feed_history(b"z-1\nz-2");
        assert_eq!(line(surface.visible_row(0)), scrolled_top);
        // 回到 live 仍然贴着最新输出。
        assert!(surface.scroll_to_bottom());
        assert_eq!(line(surface.visible_row(1)), "d");
    }

    #[test]
    fn scroll_offset_is_clamped_to_available_history() {
        let mut surface = surface(4, 2);
        surface.feed(b"a\r\nb\r\nc\r\nd");
        assert!(surface.scroll_by(99));
        assert_eq!(surface.scroll_offset(), 2);
        assert!(!surface.scroll_by(1), "已经到顶");
        assert!(surface.scroll_by(-99));
        assert_eq!(surface.scroll_offset(), 0);
    }

    /// DECSET 9/1000/1002/1003 决定上报时机，1006 只决定编码格式。
    #[test]
    fn mouse_modes_follow_private_mode_sequences() {
        let mut surface = surface(10, 2);
        assert_eq!(surface.grid().mouse_mode(), MouseMode::Off);
        assert!(!surface.grid().sgr_mouse());

        surface.feed(b"\x1b[?1000h");
        assert_eq!(surface.grid().mouse_mode(), MouseMode::Normal);
        surface.feed(b"\x1b[?1006h");
        assert!(surface.grid().sgr_mouse());

        surface.feed(b"\x1b[?1002h");
        assert_eq!(surface.grid().mouse_mode(), MouseMode::Button);
        // 关掉一个当前没在用的模式：不改变正在生效的那个。
        surface.feed(b"\x1b[?1000l");
        assert_eq!(surface.grid().mouse_mode(), MouseMode::Button);
        surface.feed(b"\x1b[?1002l");
        assert_eq!(surface.grid().mouse_mode(), MouseMode::Off);
        surface.feed(b"\x1b[?1006l");
        assert!(!surface.grid().sgr_mouse());
    }

    #[test]
    fn title_from_osc() {
        let mut surface = surface(10, 2);
        surface.feed(b"\x1b]0;build\x07");
        assert_eq!(surface.grid().title(), "build");
    }

    /// OSC 8：链接跟着**格子**走，结束序列之后的字符不再属于它。
    #[test]
    fn osc8_hyperlinks_belong_to_cells() {
        let mut surface = surface(20, 2);
        surface.feed(b"\x1b]8;;https://example.com/x/\x1b\\click\x1b]8;;\x1b\\tail");
        assert_eq!(
            surface.url_at(0, 0).as_deref(),
            Some("https://example.com/x/")
        );
        assert_eq!(
            surface.url_at(0, 4).as_deref(),
            Some("https://example.com/x/"),
            "链接覆盖它的每一个字符格"
        );
        assert_eq!(surface.url_at(0, 6), None, "结束序列后的字符没有链接");
        assert_eq!(
            surface.url_span_at(0, 2),
            Some((0, 4)),
            "OSC 8 覆盖整段 `click`"
        );
        assert_eq!(surface.url_span_at(0, 6), None);
    }

    /// 没有 OSC 8 时按行内文本找：Command+点击纯文本 URL 也要能开。
    #[test]
    fn plain_url_text_is_detected_for_clicks() {
        let mut surface = surface(40, 2);
        surface.feed(b"see https://example.com/a now");
        assert_eq!(
            surface.url_at(0, 6).as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(surface.url_at(0, 2), None, "URL 前面的字不算");
        assert_eq!(
            surface.url_span_at(0, 6),
            Some((4, 24)),
            "行内 URL 的列范围"
        );
        assert_eq!(surface.url_at(5, 0), None, "空行/越界不能恐慌");
    }

    #[test]
    fn alt_screen_restores_main_buffer() {
        let mut surface = surface(10, 3);
        surface.feed(b"main");
        surface.feed(b"\x1b[?1049h");
        surface.feed(b"alt");
        assert_eq!(text(surface.grid(), 0), "alt       ");
        surface.feed(b"\x1b[?1049l");
        assert_eq!(text(surface.grid(), 0), "main      ");
    }

    #[test]
    fn scroll_region_keeps_status_line() {
        let mut surface = surface(6, 4);
        surface.feed(b"top\r\nmid\r\nbot");
        surface.feed(b"\x1b[2;3r");
        surface.feed(b"\x1b[3;1H\n");
        // 保留第 1 行与第 4 行，只有滚动区内的内容上移。
        assert_eq!(text(surface.grid(), 0), "top   ");
    }

    #[test]
    fn insert_and_delete_line() {
        let mut surface = surface(6, 3);
        surface.feed(b"a\r\nb\r\nc");
        surface.feed(b"\x1b[1;1H\x1b[L");
        assert_eq!(text(surface.grid(), 0), "      ");
        assert_eq!(text(surface.grid(), 1), "a     ");
    }

    #[test]
    fn tab_moves_to_next_stop() {
        let mut surface = surface(20, 2);
        surface.feed(b"a\tb");
        assert_eq!(surface.grid().cell(0, 8).unwrap().ch, 'b');
    }

    #[test]
    fn bracketed_paste_mode_is_tracked() {
        let mut surface = surface(10, 2);
        assert!(!surface.grid().bracketed_paste());
        surface.feed(b"\x1b[?2004h");
        assert!(surface.grid().bracketed_paste());
        surface.feed(b"\x1b[?2004l");
        assert!(!surface.grid().bracketed_paste());
    }
}
