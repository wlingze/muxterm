//! 无头（headless）终端状态模型。
//!
//! 用 `vte` 的 `Processor` + `Handler` 把原始字节流解析成屏幕状态，
//! 不依赖任何 GUI。它让 L2 终端 payload 层可以在纯单元测试里验证
//! ANSI / alternate screen / 光标移动 / 颜色 / OSC / DCS 等行为。
//!
//! 设计要点：
//! - `feed(&[u8])`：把 pane 输出的原始字节喂给解析器，更新屏幕状态。
//! - 维护一个字符网格（rows × cols）、光标行列、当前行属性、alternate
//!   screen 标志、标题等。
//! - 不处理完整渲染细节（例如真正绘制颜色），只做「状态层面」的验证：
//!   屏幕快照、光标位置、模式标志。这样测试可断言终端状态，而不只是文本。

use vte::ansi::{
    Attr, ClearMode, Color, Handler, LineClearMode, NamedPrivateMode, PrivateMode, Processor,
};

/// 一个屏幕单元格：字符 + 前景/背景色 + 样式位。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub reverse: bool,
    pub strike: bool,
    pub hidden: bool,
}

impl Cell {
    fn blank() -> Self {
        Self {
            ch: ' ',
            ..Default::default()
        }
    }
}

/// 当前生效的文本属性（SGR）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct AttrState {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    underline: bool,
    reverse: bool,
    strike: bool,
    hidden: bool,
}

impl AttrState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn apply(&mut self, attr: &Attr) {
        match attr {
            Attr::Reset => self.reset(),
            Attr::Bold => self.bold = true,
            Attr::Dim => self.dim = true,
            Attr::Underline
            | Attr::DoubleUnderline
            | Attr::Undercurl
            | Attr::DottedUnderline
            | Attr::DashedUnderline => self.underline = true,
            Attr::Reverse => self.reverse = true,
            Attr::Strike => self.strike = true,
            Attr::Hidden => self.hidden = true,
            Attr::CancelBold => self.bold = false,
            Attr::CancelBoldDim => {
                self.bold = false;
                self.dim = false;
            }
            Attr::CancelItalic => {}
            Attr::CancelUnderline => self.underline = false,
            Attr::CancelBlink => {}
            Attr::CancelReverse => self.reverse = false,
            Attr::CancelHidden => self.hidden = false,
            Attr::CancelStrike => self.strike = false,
            Attr::Italic | Attr::BlinkSlow | Attr::BlinkFast => {}
            Attr::Foreground(c) => self.fg = Some(*c),
            Attr::Background(c) => self.bg = Some(*c),
            Attr::UnderlineColor(_) => {}
        }
    }

    fn apply_to(&self, cell: &mut Cell) {
        cell.fg = self.fg;
        cell.bg = self.bg;
        cell.bold = self.bold;
        cell.dim = self.dim;
        cell.underline = self.underline;
        cell.reverse = self.reverse;
        cell.strike = self.strike;
        cell.hidden = self.hidden;
    }
}

/// 无头终端状态。
pub struct TerminalState {
    grid: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    attr: AttrState,
    /// 是否处于 alternate screen（`CSI ? 1049 h`）。
    pub alternate_screen: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    /// 是否自动换行。
    pub line_wrap: bool,
    /// 是否显示光标。
    pub show_cursor: bool,
    /// 窗口标题（OSC 0/2）。
    pub title: Option<String>,
    processor: Processor,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl TerminalState {
    /// 创建指定行列数的状态模型。
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            grid: vec![vec![Cell::blank(); cols]; rows],
            cursor_row: 0,
            cursor_col: 0,
            attr: AttrState::default(),
            alternate_screen: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            line_wrap: true,
            show_cursor: true,
            title: None,
            processor: Processor::default(),
        }
    }

    pub fn cols(&self) -> usize {
        self.grid.first().map(|r| r.len()).unwrap_or(0)
    }

    pub fn rows(&self) -> usize {
        self.grid.len()
    }

    /// 当前光标行（0 基）。
    pub fn cursor_row(&self) -> usize {
        self.cursor_row
    }

    /// 当前光标列（0 基）。
    pub fn cursor_col(&self) -> usize {
        self.cursor_col
    }

    /// 把原始字节喂给解析器。
    pub fn feed(&mut self, bytes: &[u8]) {
        // 把 processor 临时取出来，避免与 `self`（作为 Handler）同时可变借用。
        let mut processor = std::mem::take(&mut self.processor);
        for &b in bytes {
            processor.advance(self, b);
        }
        self.processor = processor;
    }

    /// 屏幕快照：每行一个字符串（行尾空白保留）。
    pub fn snapshot(&self) -> Vec<String> {
        self.grid
            .iter()
            .map(|row| row.iter().map(|c| c.ch).collect())
            .collect()
    }

    /// 屏幕快照，去行尾空白与 NUL（空单元格用 `\0` 表示），并去掉末尾空行。
    pub fn snapshot_trimmed(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .snapshot()
            .into_iter()
            .map(|s| s.trim_end_matches([' ', '\0']).to_string())
            .collect();
        // 去掉末尾空行（保留中间空行，便于滚动区域断言）
        while rows.last().map(|r| r.is_empty()).unwrap_or(false) {
            rows.pop();
        }
        rows
    }

    /// 取某一行（0 基）的字符。
    pub fn line(&self, row: usize) -> String {
        self.grid
            .get(row)
            .map(|r| r.iter().map(|c| c.ch).collect())
            .unwrap_or_default()
    }

    /// 取某单元格。
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.grid.get(row).and_then(|r| r.get(col))
    }

    fn put_char(&mut self, c: char) {
        let cell = self
            .grid
            .get_mut(self.cursor_row)
            .and_then(|r| r.get_mut(self.cursor_col));
        if let Some(cell) = cell {
            self.attr.apply_to(cell);
            cell.ch = c;
        }
        // 在最后一列之后自动换行（或停在末列）
        if self.cursor_col + 1 < self.cols() {
            self.cursor_col += 1;
        } else if self.line_wrap {
            self.linefeed();
            self.carriage_return();
        }
    }

    fn linefeed(&mut self) {
        if self.cursor_row < self.scroll_bottom {
            self.cursor_row += 1;
        } else {
            let top = self.scroll_top;
            if top < self.rows() {
                self.grid.remove(top);
                self.grid.push(vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    fn scroll_up_n(&mut self, n: usize) {
        let span = self.scroll_bottom.saturating_sub(self.scroll_top) + 1;
        let n = n.min(span);
        for _ in 0..n {
            if self.scroll_top < self.rows() {
                self.grid.remove(self.scroll_top);
                self.grid
                    .insert(self.scroll_bottom, vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn scroll_down_n(&mut self, n: usize) {
        let span = self.scroll_bottom.saturating_sub(self.scroll_top) + 1;
        let n = n.min(span);
        for _ in 0..n {
            if self.scroll_top < self.rows() {
                self.grid.remove(self.scroll_bottom);
                self.grid
                    .insert(self.scroll_top, vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        match mode {
            ClearMode::All => {
                for row in self.grid.iter_mut() {
                    for cell in row.iter_mut() {
                        *cell = Cell::blank();
                    }
                }
            }
            ClearMode::Below => {
                if let Some(row) = self.grid.get_mut(r) {
                    for cell in row.iter_mut().skip(c) {
                        *cell = Cell::blank();
                    }
                }
                for row in self.grid.iter_mut().skip(r + 1) {
                    for cell in row.iter_mut() {
                        *cell = Cell::blank();
                    }
                }
            }
            ClearMode::Above => {
                for row in self.grid.iter_mut().take(r) {
                    for cell in row.iter_mut() {
                        *cell = Cell::blank();
                    }
                }
                if let Some(row) = self.grid.get_mut(r) {
                    for cell in row.iter_mut().take(c + 1) {
                        *cell = Cell::blank();
                    }
                }
            }
            ClearMode::Saved => {
                for row in self.grid.iter_mut() {
                    for cell in row.iter_mut() {
                        *cell = Cell::blank();
                    }
                }
            }
        }
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        let row = self.grid.get_mut(r);
        match mode {
            LineClearMode::Right => {
                if let Some(row) = row {
                    for cell in row.iter_mut().skip(c) {
                        *cell = Cell::blank();
                    }
                }
            }
            LineClearMode::Left => {
                if let Some(row) = row {
                    for cell in row.iter_mut().take(c + 1) {
                        *cell = Cell::blank();
                    }
                }
            }
            LineClearMode::All => {
                if let Some(row) = row {
                    for cell in row.iter_mut() {
                        *cell = Cell::blank();
                    }
                }
            }
        }
    }

    fn erase_chars(&mut self, n: usize) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        let cols = self.cols();
        if let Some(row) = self.grid.get_mut(r) {
            for cell in row.iter_mut().skip(c).take(n.min(cols.saturating_sub(c))) {
                *cell = Cell::blank();
            }
        }
    }

    fn insert_blank(&mut self, n: usize) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        let cols = self.cols();
        if let Some(row) = self.grid.get_mut(r) {
            for _ in 0..n.min(cols.saturating_sub(c)) {
                row.pop();
                row.insert(c, Cell::blank());
            }
        }
    }

    fn delete_chars(&mut self, n: usize) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        let cols = self.cols();
        if let Some(row) = self.grid.get_mut(r) {
            for _ in 0..n.min(cols.saturating_sub(c)) {
                row.remove(c);
                row.push(Cell::blank());
            }
        }
    }

    fn insert_blank_lines(&mut self, n: usize) {
        let n = n.min(self.rows());
        for _ in 0..n {
            if self.grid.len() > self.scroll_bottom {
                self.grid.remove(self.scroll_bottom);
            }
            self.grid
                .insert(self.scroll_top, vec![Cell::blank(); self.cols()]);
        }
    }

    fn delete_lines(&mut self, n: usize) {
        let n = n.min(self.rows());
        for _ in 0..n {
            if self.scroll_top < self.rows() {
                self.grid.remove(self.scroll_top);
                self.grid.push(vec![Cell::blank(); self.cols()]);
            }
        }
    }
}

impl Handler for TerminalState {
    fn input(&mut self, c: char) {
        self.put_char(c);
    }

    fn goto(&mut self, line: i32, col: usize) {
        self.cursor_row = line.clamp(0, self.rows() as i32 - 1) as usize;
        self.cursor_col = col.min(self.cols().saturating_sub(1));
    }

    fn goto_line(&mut self, line: i32) {
        self.cursor_row = line.clamp(0, self.rows() as i32 - 1) as usize;
    }

    fn goto_col(&mut self, col: usize) {
        self.cursor_col = col.min(self.cols().saturating_sub(1));
    }

    fn insert_blank(&mut self, n: usize) {
        self.insert_blank(n);
    }

    fn move_up(&mut self, n: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(n.min(self.cursor_row));
    }

    fn move_down(&mut self, n: usize) {
        self.cursor_row = (self.cursor_row + n).min(self.rows() - 1);
    }

    fn move_forward(&mut self, n: usize) {
        self.cursor_col = (self.cursor_col + n).min(self.cols() - 1);
    }

    fn move_backward(&mut self, n: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(n.min(self.cursor_col));
    }

    fn move_down_and_cr(&mut self, n: usize) {
        self.move_down(n);
        self.cursor_col = 0;
    }

    fn move_up_and_cr(&mut self, n: usize) {
        self.move_up(n);
        self.cursor_col = 0;
    }

    fn carriage_return(&mut self) {
        self.carriage_return();
    }

    fn linefeed(&mut self) {
        self.linefeed();
    }

    fn newline(&mut self) {
        self.linefeed();
        self.carriage_return();
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    fn scroll_up(&mut self, n: usize) {
        self.scroll_up_n(n);
    }

    fn scroll_down(&mut self, n: usize) {
        self.scroll_down_n(n);
    }

    fn insert_blank_lines(&mut self, n: usize) {
        self.insert_blank_lines(n);
    }

    fn delete_lines(&mut self, n: usize) {
        self.delete_lines(n);
    }

    fn erase_chars(&mut self, n: usize) {
        self.erase_chars(n);
    }

    fn delete_chars(&mut self, n: usize) {
        self.delete_chars(n);
    }

    fn save_cursor_position(&mut self) {}

    fn restore_cursor_position(&mut self) {}

    fn clear_line(&mut self, mode: LineClearMode) {
        self.clear_line(mode);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        self.clear_screen(mode);
    }

    fn reset_state(&mut self) {
        self.attr.reset();
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.line_wrap = true;
        self.show_cursor = true;
    }

    fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down_n(1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.attr.apply(&attr);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        match mode {
            PrivateMode::Named(n) => match n {
                NamedPrivateMode::SwapScreenAndSetRestoreCursor => self.alternate_screen = true,
                NamedPrivateMode::LineWrap => self.line_wrap = true,
                NamedPrivateMode::ShowCursor => self.show_cursor = true,
                _ => {}
            },
            PrivateMode::Unknown(_) => {}
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        match mode {
            PrivateMode::Named(n) => match n {
                NamedPrivateMode::SwapScreenAndSetRestoreCursor => self.alternate_screen = false,
                NamedPrivateMode::LineWrap => self.line_wrap = false,
                NamedPrivateMode::ShowCursor => self.show_cursor = false,
                _ => {}
            },
            PrivateMode::Unknown(_) => {}
        }
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let rows = self.rows();
        // tmux/ANSI 用 1 基行号，转成 0 基
        let top = top.saturating_sub(1);
        self.scroll_top = top.min(rows.saturating_sub(1));
        self.scroll_bottom = bottom
            .map(|b| b.saturating_sub(1))
            .unwrap_or(rows.saturating_sub(1))
            .min(rows.saturating_sub(1));
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
            self.scroll_bottom = rows.saturating_sub(1);
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vte::ansi::{Color::*, NamedColor, Rgb};

    /// 便捷：喂字节后返回去尾空白的屏幕快照。
    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    #[test]
    fn plain_text_wraps_and_scrolls() {
        let mut t = TerminalState::new(5, 3);
        t.feed(b"hello");
        assert_eq!(snap(&t), vec!["hello"]);
        // 超出列宽自动换行
        t.feed(b"world");
        assert_eq!(snap(&t), vec!["hello", "world"]);
    }

    #[test]
    fn cr_lf_handling() {
        let mut t = TerminalState::new(10, 4);
        t.feed(b"abc\r\nXYZ");
        // CR 回行首，LF 换行，XYZ 打到第 2 行
        assert_eq!(snap(&t), vec!["abc", "XYZ"]);
    }

    #[test]
    fn clear_screen_above_and_below() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"aaabbbcccddd"); // 10 列放 10 字符，dd 换到第 2 行
        assert_eq!(snap(&t), vec!["aaabbbcccd", "dd"]);
        t.feed(b"\x1b[2;2H"); // 光标到第 2 行第 2 列
        t.feed(b"\x1b[J"); // 清光标以下（第 2 行光标右侧 + 第 3 行起）
        assert_eq!(snap(&t), vec!["aaabbbcccd", "d"]);
        t.feed(b"\x1b[2J"); // 清全屏
        assert_eq!(snap(&t), Vec::<String>::new());
    }

    #[test]
    fn clear_line_right() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"abcdef\x1b[1;3H\x1b[K"); // 光标到 (1,2) 列，清行尾
        assert_eq!(snap(&t), vec!["ab"]);
    }

    #[test]
    fn cursor_movement_and_show() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;2H"); // goto (0,1)
        assert_eq!((t.cursor_row(), t.cursor_col()), (0, 1));
        t.feed(b"X");
        assert_eq!(snap(&t), vec!["aXcdef"]);
        t.feed(b"\x1b[?25l"); // hide cursor（私有模式）
        assert!(!t.show_cursor);
        t.feed(b"\x1b[?25h"); // show cursor
        assert!(t.show_cursor);
    }

    #[test]
    fn insert_delete_chars_and_lines() {
        let mut t = TerminalState::new(8, 5);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;3H"); // col 2
        t.feed(b"\x1b[2@"); // insert 2 blanks at cursor
        assert_eq!(snap(&t), vec!["ab  cdef"]);
        t.feed(b"\x1b[2P"); // delete 2 chars at cursor
        assert_eq!(snap(&t), vec!["abcdef"]);
    }

    #[test]
    fn colors_16_256_truecolor() {
        let mut t = TerminalState::new(20, 3);
        t.feed(b"\x1b[31mred\x1b[0m");
        let cell = t.cell(0, 0).unwrap();
        assert_eq!(cell.fg, Some(Named(NamedColor::Red)));
        // 256 色：38;5;196
        let mut t2 = TerminalState::new(20, 3);
        t2.feed(b"\x1b[38;5;196mX");
        let c2 = t2.cell(0, 0).unwrap();
        assert_eq!(c2.fg, Some(Indexed(196)));
        // true color：38;2;r;g;b
        let mut t3 = TerminalState::new(20, 3);
        t3.feed(b"\x1b[38;2;255;0;0mX");
        let c3 = t3.cell(0, 0).unwrap();
        assert_eq!(c3.fg, Some(Spec(Rgb { r: 255, g: 0, b: 0 })));
    }

    #[test]
    fn styles_bold_dim_underline_reverse_strike() {
        let mut t = TerminalState::new(20, 3);
        t.feed(b"\x1b[1mb\x1b[2md\x1b[4mu\x1b[7mr\x1b[9ms\x1b[0m");
        let c0 = t.cell(0, 0).unwrap();
        assert!(c0.bold);
        let c1 = t.cell(0, 1).unwrap();
        assert!(c1.dim);
        let c2 = t.cell(0, 2).unwrap();
        assert!(c2.underline);
        let c3 = t.cell(0, 3).unwrap();
        assert!(c3.reverse);
        let c4 = t.cell(0, 4).unwrap();
        assert!(c4.strike);
        // 重置后新字符无样式
        t.feed(b"\x1b[0mZ");
        let c5 = t.cell(0, 5).unwrap();
        assert!(!c5.bold && !c5.underline);
    }

    #[test]
    fn alternate_screen_enter_exit() {
        let mut t = TerminalState::new(20, 5);
        t.feed(b"\x1b[?1049h");
        assert!(t.alternate_screen);
        t.feed(b"fullscreen");
        t.feed(b"\x1b[?1049l");
        assert!(!t.alternate_screen);
    }

    #[test]
    fn scroll_region_and_reverse_index() {
        let mut t = TerminalState::new(10, 5);
        // 用 CRLF（真实终端程序的标准行尾）
        t.feed(b"aaaa\r\nbbbb\r\ncccc\r\ndddd");
        assert_eq!(snap(&t), vec!["aaaa", "bbbb", "cccc", "dddd"]);
        // 滚动区域行 2..4（1 基）
        t.feed(b"\x1b[2;4r");
        // 光标移到滚动区顶（第 2 行）
        t.feed(b"\x1b[2;1H");
        // 反向换行在滚动区顶部：区域内内容下移一行，最底部一行滚出
        t.feed(b"\x1bM");
        assert_eq!(snap(&t), vec!["aaaa", "", "bbbb", "cccc"]);
    }

    #[test]
    fn utf8_chinese_emoji_cjk() {
        let mut t = TerminalState::new(40, 5);
        t.feed("中文😀宽".as_bytes());
        assert_eq!(snap(&t), vec!["中文😀宽"]);
    }

    #[test]
    fn incomplete_utf8_across_feed_calls() {
        let mut t = TerminalState::new(40, 5);
        // "中" = E4 B8 AD，分两次喂
        let bytes = "中".as_bytes();
        t.feed(&bytes[..1]);
        t.feed(&bytes[1..]);
        assert_eq!(snap(&t), vec!["中"]);
    }

    #[test]
    fn title_osc0_and_osc2() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b]2;my title\x07");
        assert_eq!(t.title.as_deref(), Some("my title"));
        let mut t2 = TerminalState::new(40, 5);
        t2.feed(b"\x1b]0;other title\x1b\\");
        assert_eq!(t2.title.as_deref(), Some("other title"));
    }

    #[test]
    fn unknown_osc_does_not_corrupt_state() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b]999;arbitrary\x07");
        // 不认识的 OSC 之后，正常文本仍能打印
        t.feed(b"ok");
        assert_eq!(snap(&t), vec!["ok"]);
    }

    #[test]
    fn dcs_passthrough_does_not_break_control_parsing() {
        let mut t = TerminalState::new(40, 5);
        // DCS passthrough：ESC P ... ESC \，内部字节不当作控制消息
        t.feed(b"\x1bPtmux;\x1b\x1b]1337;SetUserVar=X=\x07\x1b\\");
        t.feed(b"after");
        assert_eq!(snap(&t), vec!["after"]);
    }

    #[test]
    fn backspace_and_tab() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"abc\x08"); // backspace 光标左移，不删字符
        assert_eq!(t.cursor_col(), 2);
        t.feed(b"X"); // 在 col2 覆盖原 'c'
        assert_eq!(snap(&t), vec!["abX"]);
    }

    #[test]
    fn newline_and_reverse_linefeed() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"a\x1bD"); // ESC D = linefeed
        assert_eq!(t.cursor_row(), 1);
        t.feed(b"\x1bM"); // reverse index
        assert_eq!(t.cursor_row(), 0);
    }

    #[test]
    fn resize_via_reinit_is_reasonable() {
        // 状态模型目前不支持运行时 resize（VTE 侧也没有稳定 API），
        // 这里验证初始化尺寸正确。
        let t = TerminalState::new(120, 40);
        assert_eq!(t.cols(), 120);
        assert_eq!(t.rows(), 40);
    }

    #[test]
    fn crlf_from_real_tmux_output() {
        // 真实 tmux -CC 里 \r\n 是常见的行尾
        let mut t = TerminalState::new(80, 10);
        t.feed(b"line1\r\nline2\r\n");
        assert_eq!(snap(&t), vec!["line1", "line2"]);
    }
}

#[cfg(test)]
mod tui_redraw_tests {
    use super::*;

    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    /// TUI 局部重绘：移动光标覆盖旧内容，而不是追加新行。
    #[test]
    fn tui_cursor_overwrite_local_region() {
        let mut t = TerminalState::new(20, 5);
        // 第一行打印 "status: idle"
        t.feed(b"status: idle");
        // 光标回到行首，覆盖同一行 -> 应替换旧内容而非换行
        t.feed(b"\x1b[1;1H"); // goto (0,0)
        t.feed(b"status: busy");
        assert_eq!(snap(&t), vec!["status: busy"]);
        // 光标列应停在覆盖文本末尾（"status: busy" = 12 字符）
        assert_eq!(t.cursor_col(), 12);
    }

    /// TUI 局部重绘：在中间某行覆盖一个子区域。
    #[test]
    fn tui_partial_line_overwrite() {
        let mut t = TerminalState::new(30, 5);
        t.feed(b"line0\r\nline1\r\nline2\r\nline3\r\nline4");
        // 光标到第 3 行第 0 列，覆盖整行内容
        t.feed(b"\x1b[3;1H");
        t.feed(b"REPLACED");
        assert_eq!(
            snap(&t),
            vec!["line0", "line1", "REPLACED", "line3", "line4"]
        );
    }

    /// TUI 清屏后重绘：ESC[2J 清屏，然后从 (1,1) 开始画。
    #[test]
    fn tui_clear_then_redraw_from_top() {
        let mut t = TerminalState::new(20, 5);
        t.feed(b"stale content that should be gone\r\nmore stale");
        t.feed(b"\x1b[2J"); // clear all
        assert_eq!(snap(&t), Vec::<String>::new(), "清屏后应为空");
        t.feed(b"\x1b[1;1H"); // 光标回左上
        t.feed(b"fresh frame");
        assert_eq!(snap(&t), vec!["fresh frame"]);
    }

    /// 光标显隐 + 位置，验证 TUI 状态（不只是最终文本）。
    #[test]
    fn tui_cursor_position_and_visibility() {
        let mut t = TerminalState::new(20, 5);
        assert!(t.show_cursor);
        t.feed(b"\x1b[?25l"); // hide
        assert!(!t.show_cursor);
        t.feed(b"\x1b[?25h"); // show
        assert!(t.show_cursor);
        // 光标移到 (2,5) 即第 3 行第 6 列
        t.feed(b"\x1b[3;6H");
        assert_eq!((t.cursor_row(), t.cursor_col()), (2, 5));
    }

    /// 滚动区域：在区域内打印，触发上滚时只滚动区域内的行。
    #[test]
    fn tui_scroll_region_isolated_scroll() {
        let mut t = TerminalState::new(10, 5);
        // 设滚动区域第 2..4 行（1 基）
        t.feed(b"\x1b[2;4r");
        t.feed(b"\x1b[2;1H");
        // 打印 3 行到区域内，最后一行应滚动掉区域顶
        t.feed(b"1\r\n2\r\n3");
        // 区域外第 1 行保持空
        assert_eq!(snap(&t), vec!["", "1", "2", "3"]);
    }
}
