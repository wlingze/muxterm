//! vte::Perform 实现：把 Core 给的原始字节写进 [`Grid`]。
//!
//! 输入永远是 `PaneOutput` 字节流，不是 Index 的 ANSI dump（`SURFACE.md` §3 定律 2、9）。

use vte::{Params, Perform};

use super::grid::Grid;

/// 一个 pane 的常驻 Surface：解析器 + 网格。
///
/// 解析器与网格分开存放，避免 `advance` 期间重复借用 `self`。
pub struct Surface {
    parser: vte::Parser,
    grid: Grid,
}

impl Surface {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            grid: Grid::new(cols, rows),
        }
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
        if params[0] == b"0" || params[0] == b"2" {
            let title = String::from_utf8_lossy(params[1]).into_owned();
            self.set_title(title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(cols: usize, rows: usize) -> Surface {
        Surface::new(cols, rows)
    }

    fn text(grid: &Grid, row: usize) -> String {
        grid.row(row).iter().map(|cell| cell.ch).collect()
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
        assert_eq!(surface.grid().cell(0, 0).unwrap().ch, '你');
        assert_eq!(surface.grid().cell(0, 1).unwrap().ch, '好');
    }

    #[test]
    fn title_from_osc() {
        let mut surface = surface(10, 2);
        surface.feed(b"\x1b]0;build\x07");
        assert_eq!(surface.grid().title(), "build");
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
