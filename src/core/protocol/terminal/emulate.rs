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
        Self::default()
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
            Attr::Underline | Attr::DoubleUnderline | Attr::Undercurl
            | Attr::DottedUnderline | Attr::DashedUnderline => self.underline = true,
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

    /// 屏幕快照，去行尾空白。
    pub fn snapshot_trimmed(&self) -> Vec<String> {
        self.snapshot()
            .into_iter()
            .map(|s| s.trim_end().to_string())
            .collect()
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
        if self.cursor_col >= self.cols() {
            if !self.line_wrap {
                return;
            }
            self.linefeed();
            self.carriage_return();
        }
        let cell = self
            .grid
            .get_mut(self.cursor_row)
            .and_then(|r| r.get_mut(self.cursor_col));
        if let Some(cell) = cell {
            self.attr.apply_to(cell);
            cell.ch = c;
        }
        if self.cursor_col + 1 < self.cols() {
            self.cursor_col += 1;
        } else if self.line_wrap {
            self.cursor_col = self.cols() - 1;
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
            ClearMode::All | ClearMode::Below => {
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
            for i in c..(c + n).min(cols) {
                row[i] = Cell::blank();
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
        self.scroll_top = top.min(rows.saturating_sub(1));
        self.scroll_bottom = bottom.unwrap_or(rows.saturating_sub(1)).min(rows.saturating_sub(1));
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
