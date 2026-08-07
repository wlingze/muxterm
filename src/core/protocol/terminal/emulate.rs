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
    Attr, CharsetIndex, ClearMode, Color, CursorShape, Handler, KeyboardModes, NamedColor,
    KeyboardModesApplyBehavior, LineClearMode, ModifyOtherKeys, NamedPrivateMode, PrivateMode,
    Processor, Rgb, StandardCharset,
};

/// 一个屏幕单元格：字符 + 前景/背景色 + 样式位 + hyperlink URI。
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
    /// OSC 8 hyperlink URI（若有）。
    pub link: Option<String>,
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
    link: Option<String>,
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
            Attr::Foreground(Color::Named(NamedColor::Foreground)) => self.fg = None,
            Attr::Background(Color::Named(NamedColor::Background)) => self.bg = None,
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
        cell.link = self.link.clone();
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
    /// bracketed paste 模式（DECSET 2004）。
    pub bracketed_paste: bool,
    /// mouse reporting 模式（1000/1002/1003/1005/1006）。
    pub mouse_reporting: bool,
    /// focus in/out 上报模式（1004）。
    pub focus_reporting: bool,
    /// 窗口标题（OSC 0/2）。
    pub title: Option<String>,
    /// 标题栈（OSC 22 push / 23 pop）。
    pub title_stack: Vec<String>,
    /// 光标形状（DECSCUSR / OSC 50）。
    pub cursor_shape: CursorShape,
    /// 光标闪烁（DECSCUSR）。
    pub cursor_blinking: bool,
    /// kitty keyboard protocol 模式（CSI > u / CSI = u）。
    pub keyboard_mode: KeyboardModes,
    /// XTMODKEYS modifyOtherKeys 状态（CSI > 4 m）。
    pub modify_other_keys: ModifyOtherKeys,
    /// 调色板索引（OSC 4 / 10-12 可重定义，ANSI 8 + bright 8 = 16）。
    pub palette: [Rgb; 16],
    /// G0-G3 字符集指定（DEC charset designation）。
    pub charsets: [StandardCharset; 4],
    /// scrollback：从屏幕顶部滚出的行（字符串），有上限。
    pub scrollback: Vec<String>,
    /// 当前激活字符集（SI/SO 切换）。
    pub active_charset: CharsetIndex,
    processor: Processor,
}

/// 标准 16 色终端调色板（ANSI + bright）。
fn default_palette() -> [Rgb; 16] {
    [
        Rgb { r: 0, g: 0, b: 0 },   // 0 black
        Rgb { r: 205, g: 0, b: 0 }, // 1 red
        Rgb { r: 0, g: 205, b: 0 }, // 2 green
        Rgb {
            r: 205,
            g: 205,
            b: 0,
        }, // 3 yellow
        Rgb { r: 0, g: 0, b: 205 }, // 4 blue
        Rgb {
            r: 205,
            g: 0,
            b: 205,
        }, // 5 magenta
        Rgb {
            r: 0,
            g: 205,
            b: 205,
        }, // 6 cyan
        Rgb {
            r: 229,
            g: 229,
            b: 229,
        }, // 7 white
        Rgb {
            r: 127,
            g: 127,
            b: 127,
        }, // 8 bright black
        Rgb { r: 255, g: 0, b: 0 }, // 9 bright red
        Rgb { r: 0, g: 255, b: 0 }, // 10 bright green
        Rgb {
            r: 255,
            g: 255,
            b: 0,
        }, // 11 bright yellow
        Rgb { r: 0, g: 0, b: 255 }, // 12 bright blue
        Rgb {
            r: 255,
            g: 0,
            b: 255,
        }, // 13 bright magenta
        Rgb {
            r: 0,
            g: 255,
            b: 255,
        }, // 14 bright cyan
        Rgb {
            r: 255,
            g: 255,
            b: 255,
        }, // 15 bright white
    ]
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
            title_stack: Vec::new(),
            cursor_shape: CursorShape::Block,
            cursor_blinking: false,
            keyboard_mode: KeyboardModes::default(),
            modify_other_keys: ModifyOtherKeys::Reset,
            bracketed_paste: false,
            mouse_reporting: false,
            focus_reporting: false,
            palette: default_palette(),
            charsets: [StandardCharset::default(); 4],
            active_charset: CharsetIndex::G0,
            scrollback: Vec::new(),
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
        // 组合字符（零宽）附着到前一个字符，不占格。
        if is_combining(c) {
            let col = self.cursor_col.saturating_sub(1);
            if let Some(cell) = self
                .grid
                .get_mut(self.cursor_row)
                .and_then(|r| r.get_mut(col))
            {
                cell.ch = c;
            }
            return;
        }
        // 先按当前激活字符集映射（如 DEC line-drawing），再写单元格。
        let idx = self.active_charset as usize;
        let mapped = self.charsets.get(idx).copied().unwrap_or_default().map(c);
        let cell = self
            .grid
            .get_mut(self.cursor_row)
            .and_then(|r| r.get_mut(self.cursor_col));
        if let Some(cell) = cell {
            self.attr.apply_to(cell);
            cell.ch = mapped;
        }
        // 宽字符（CJK）占 2 格：右侧留一个空格占位。
        let advance = if is_wide(mapped) { 2 } else { 1 };
        for _ in 0..advance {
            if self.cursor_col + 1 < self.cols() {
                self.cursor_col += 1;
            } else if self.line_wrap {
                self.linefeed();
                self.carriage_return();
            } else {
                break;
            }
        }
    }

    fn linefeed(&mut self) {
        if self.cursor_row < self.scroll_bottom {
            self.cursor_row += 1;
        } else {
            let top = self.scroll_top;
            if top == 0 && self.rows() > 0 {
                // 整屏上滚：滚出顶行的内容进入 scrollback
                if let Some(evicted) = self.grid.first() {
                    let s: String = evicted.iter().map(|c| c.ch).collect();
                    // 去掉行尾空白，保持 scrollback 可读
                    self.push_scrollback(s.trim_end().to_string());
                }
            }
            if top < self.rows() {
                self.grid.remove(top);
                self.grid.push(vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    /// 把一行推入 scrollback（有上限）。
    fn push_scrollback(&mut self, line: String) {
        if self.scrollback.len() >= SCROLLBACK_MAX_LINES {
            self.scrollback.remove(0);
        }
        self.scrollback.push(line);
    }

    /// scrollback 行数。
    pub fn scrollback_lines(&self) -> usize {
        self.scrollback.len()
    }

    /// 取第 idx 行 scrollback（0 = 最早）。
    pub fn scrollback_line(&self, idx: usize) -> Option<&str> {
        self.scrollback.get(idx).map(|s| s.as_str())
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

/// scrollback 最大保留行数（有上限，避免无界增长）。
pub const SCROLLBACK_MAX_LINES: usize = 1000;

/// 是否零宽组合字符（附着到前一个字符）。
fn is_combining(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}' // combining diacritical marks
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// 是否 CJK 宽字符（显示占 2 格）。
fn is_wide(c: char) -> bool {
    matches!(
        c,
        '\u{1100}'..='\u{115F}'
            | '\u{2E80}'..='\u{303E}'
            | '\u{3041}'..='\u{33FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{A000}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE30}'..='\u{FE4F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}'
            | '\u{20000}'..='\u{2FFFD}'
            | '\u{30000}'..='\u{3FFFD}'
    )
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
                NamedPrivateMode::BracketedPaste => self.bracketed_paste = true,
                NamedPrivateMode::ReportMouseClicks
                | NamedPrivateMode::ReportCellMouseMotion
                | NamedPrivateMode::ReportAllMouseMotion
                | NamedPrivateMode::Utf8Mouse
                | NamedPrivateMode::SgrMouse => self.mouse_reporting = true,
                NamedPrivateMode::ReportFocusInOut => self.focus_reporting = true,
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
                NamedPrivateMode::BracketedPaste => self.bracketed_paste = false,
                NamedPrivateMode::ReportMouseClicks
                | NamedPrivateMode::ReportCellMouseMotion
                | NamedPrivateMode::ReportAllMouseMotion
                | NamedPrivateMode::Utf8Mouse
                | NamedPrivateMode::SgrMouse => self.mouse_reporting = false,
                NamedPrivateMode::ReportFocusInOut => self.focus_reporting = false,
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

    /// OSC 22：把当前标题压栈。
    fn push_title(&mut self) {
        if let Some(t) = &self.title {
            self.title_stack.push(t.clone());
        }
    }

    /// OSC 23：从标题栈弹出一个标题（若栈空则忽略）。
    fn pop_title(&mut self) {
        if let Some(t) = self.title_stack.pop() {
            self.title = Some(t);
        }
    }

    /// DECSCUSR / OSC 50：设置光标形状。
    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.cursor_shape = shape;
    }

    /// DECSCUSR（CSI Ps SP q）：设置光标形状 + 闪烁。
    fn set_cursor_style(&mut self, style: Option<vte::ansi::CursorStyle>) {
        match style {
            Some(s) => {
                self.cursor_shape = s.shape;
                self.cursor_blinking = s.blinking;
            }
            None => {
                self.cursor_shape = CursorShape::Block;
                self.cursor_blinking = false;
            }
        }
    }

    /// kitty keyboard protocol（CSI ? u / = u）：按行为合并/替换模式位。
    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        self.keyboard_mode = match behavior {
            KeyboardModesApplyBehavior::Replace => mode,
            KeyboardModesApplyBehavior::Union => self.keyboard_mode | mode,
            KeyboardModesApplyBehavior::Difference => self.keyboard_mode & !mode,
        };
    }

    /// SI / SO：切换激活的 G0 / G1 字符集。
    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.active_charset = index;
    }

    /// ESC ( / ) / * / + <charset>：指定 G0-G3 字符集。
    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        let i = index as usize;
        if i < self.charsets.len() {
            self.charsets[i] = charset;
        }
    }

    /// XTMODKEYS（CSI > 4 m）：modifyOtherKeys 状态。
    fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys) {
        self.modify_other_keys = mode;
    }

    /// OSC 8 hyperlink：记录当前生效的 link（OSC 8 ; ; URI ST），
    /// 之后的字符单元格都会带上该 link；`None` 清除。
    fn set_hyperlink(&mut self, link: Option<vte::ansi::Hyperlink>) {
        self.attr.link = link.map(|h| h.uri);
    }

    /// OSC 4 / 10-12：重定义调色板索引颜色。
    fn set_color(&mut self, index: usize, color: Rgb) {
        if index < self.palette.len() {
            self.palette[index] = color;
        }
    }

    /// OSC 104 / 110-112：重置调色板索引颜色为默认值。
    fn reset_color(&mut self, index: usize) {
        if index < self.palette.len() {
            self.palette[index] = default_palette()[index];
        }
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
        // CJK 宽字符各占 2 格（右侧占位），因此快照里宽字符之间有空格占位。
        assert_eq!(snap(&t), vec!["中 文 😀宽"]);
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

#[cfg(test)]
mod hyperlink_tests {
    use super::*;

    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    /// OSC 8 hyperlink：链接 URI 应附着在之后的单元格上。
    #[test]
    fn osc8_hyperlink_attaches_to_cells() {
        let mut t = TerminalState::new(40, 5);
        // OSC 8 ; ; https://example.com ST  clickable  OSC 8 ; ; ST
        t.feed(b"\x1b]8;;https://example.com\x1b\\");
        t.feed(b"clickable");
        t.feed(b"\x1b]8;;\x1b\\"); // 结束 hyperlink
        t.feed(b" plain");

        assert_eq!(snap(&t), vec!["clickable plain"]);
        // clickable 各字符带 link
        for col in 0..9 {
            let c = t.cell(0, col).unwrap();
            assert_eq!(
                c.link.as_deref(),
                Some("https://example.com"),
                "col {col} 应有 link"
            );
        }
        // 结束后的字符无 link
        let plain = t.cell(0, 10).unwrap();
        assert!(plain.link.is_none(), "结束 hyperlink 后字符不应有 link");
    }

    /// OSC 8 hyperlink 在清屏/覆盖后仍正确（link 是单元格属性，随格走）。
    #[test]
    fn osc8_hyperlink_cleared_on_blank() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b]8;;https://example.com\x1b\\abc");
        // 清行：应把 link 也清掉（blank 单元格无 link）
        t.feed(b"\x1b[1;1H\x1b[K");
        assert_eq!(snap(&t), Vec::<String>::new());
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;

    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    /// 调色板默认值 + OSC 4 重定义 + OSC 104 重置。
    #[test]
    fn osc4_palette_override_and_reset() {
        let mut t = TerminalState::new(40, 5);
        // 默认红色
        assert_eq!(t.palette[1], Rgb { r: 205, g: 0, b: 0 });

        // OSC 4 ; 1 ; rgb:ff/00/00 ST  （xterm 颜色格式）
        t.feed(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
        assert_eq!(t.palette[1], Rgb { r: 255, g: 0, b: 0 });

        // OSC 104 ; 1 ST  重置
        t.feed(b"\x1b]104;1\x1b\\");
        assert_eq!(t.palette[1], Rgb { r: 205, g: 0, b: 0 });
    }

    /// 重定义调色板后，用该索引着色的字符应反映新颜色（经 Indexed 颜色引用）。
    #[test]
    fn palette_override_affects_indexed_foreground() {
        let mut t = TerminalState::new(40, 5);
        // 重定义红色为紫色
        t.feed(b"\x1b]4;1;rgb:ff/00/ff\x1b\\");
        // 用 38;5;1 着色字符 X
        t.feed(b"\x1b[38;5;1mX");
        let c = t.cell(0, 0).unwrap();
        assert_eq!(c.fg, Some(Color::Indexed(1)));
        // 通过 palette 解析，Indexed(1) 现在应是新颜色
        assert_eq!(
            t.palette[1],
            Rgb {
                r: 255,
                g: 0,
                b: 255
            }
        );
    }

    /// OSC 4 对 16 色范围外索引的修改应安全忽略（不越界、不 panic）。
    #[test]
    fn osc4_out_of_range_ignored() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b]4;999;rgb:ff/00/00\x1b\\"); // 越界
        t.feed(b"ok");
        assert_eq!(snap(&t), vec!["ok"]);
        assert_eq!(t.palette.len(), 16, "调色板应保持 16 项");
    }
}

#[cfg(test)]
mod input_mode_tests {
    use super::*;

    /// bracketed paste（DECSET 2004）开启/关闭。
    #[test]
    fn bracketed_paste_mode() {
        let mut t = TerminalState::new(40, 5);
        assert!(!t.bracketed_paste);
        t.feed(b"\x1b[?2004h");
        assert!(t.bracketed_paste, "CSI ? 2004 h 应开启 bracketed paste");
        t.feed(b"\x1b[?2004l");
        assert!(!t.bracketed_paste, "CSI ? 2004 l 应关闭");
    }

    /// mouse reporting（1000/1002/1003/1006）开启/关闭。
    #[test]
    fn mouse_reporting_mode() {
        let mut t = TerminalState::new(40, 5);
        assert!(!t.mouse_reporting);
        t.feed(b"\x1b[?1000h");
        assert!(t.mouse_reporting, "CSI ? 1000 h 应开启 mouse reporting");
        t.feed(b"\x1b[?1000l");
        assert!(!t.mouse_reporting, "CSI ? 1000 l 应关闭");
        // SGR mouse (1006)
        t.feed(b"\x1b[?1006h");
        assert!(t.mouse_reporting);
    }

    /// focus in/out（1004）开启/关闭。
    #[test]
    fn focus_reporting_mode() {
        let mut t = TerminalState::new(40, 5);
        assert!(!t.focus_reporting);
        t.feed(b"\x1b[?1004h");
        assert!(t.focus_reporting, "CSI ? 1004 h 应开启 focus reporting");
        t.feed(b"\x1b[?1004l");
        assert!(!t.focus_reporting, "CSI ? 1004 l 应关闭");
    }

    /// 这些输入模式不应影响文本渲染。
    #[test]
    fn input_modes_do_not_affect_text() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b[?2004h\x1b[?1000h\x1b[?1004h");
        t.feed(b"text");
        assert_eq!(t.snapshot_trimmed(), vec!["text"]);
    }
}

#[cfg(test)]
mod title_cursor_tests {
    use super::*;

    /// OSC 22 push / OSC 23 pop 标题栈。
    #[test]
    fn title_stack_push_pop() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b]2;initial\x07");
        assert_eq!(t.title.as_deref(), Some("initial"));
        // CSI 22 t 压栈（保存当前标题），然后设置新标题
        t.feed(b"\x1b[22t"); // push title
        t.feed(b"\x1b]2;new\x07");
        assert_eq!(t.title.as_deref(), Some("new"));
        assert_eq!(t.title_stack, vec!["initial".to_string()]);
        // CSI 23 t 弹栈（恢复保存的标题）
        t.feed(b"\x1b[23t"); // pop title
        assert_eq!(t.title.as_deref(), Some("initial"));
        assert!(t.title_stack.is_empty());
    }

    /// DECSCUSR 设置光标形状（块/下划线/竖条）+ 闪烁。
    #[test]
    fn decscur_cursor_shape() {
        let mut t = TerminalState::new(40, 5);
        assert_eq!(t.cursor_shape, CursorShape::Block);
        // CSI 3 SP q = 下划线 + 闪烁
        t.feed(b"\x1b[3 q");
        assert_eq!(t.cursor_shape, CursorShape::Underline);
        assert!(t.cursor_blinking);
        // CSI 5 SP q = 竖条 + 闪烁
        t.feed(b"\x1b[5 q");
        assert_eq!(t.cursor_shape, CursorShape::Beam);
        assert!(t.cursor_blinking);
        // CSI 0 SP q = 复位
        t.feed(b"\x1b[0 q");
        assert_eq!(t.cursor_shape, CursorShape::Block);
        assert!(!t.cursor_blinking);
    }

    /// OSC 50 CursorShape= 设置光标形状。
    #[test]
    fn osc50_cursor_shape() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b]50;CursorShape=1\x07"); // beam
        assert_eq!(t.cursor_shape, CursorShape::Beam);
        t.feed(b"\x1b]50;CursorShape=2\x07"); // underline
        assert_eq!(t.cursor_shape, CursorShape::Underline);
    }
}

#[cfg(test)]
mod keyboard_protocol_tests {
    use super::*;

    /// kitty keyboard protocol（CSI ? u / = u）模式位合并。
    #[test]
    fn kitty_keyboard_mode() {
        let mut t = TerminalState::new(40, 5);
        // 默认无模式
        assert_eq!(t.keyboard_mode, KeyboardModes::default());
        // CSI = 1 u：设置 DISAMBIGUATE_ESC_CODES
        t.feed(b"\x1b[=1u");
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::DISAMBIGUATE_ESC_CODES));
        // CSI = 2 u：设置 REPORT_EVENT_TYPES
        t.feed(b"\x1b[=2u");
        assert!(t.keyboard_mode.contains(KeyboardModes::REPORT_EVENT_TYPES));
    }

    /// XTMODKEYS modifyOtherKeys（CSI > 4 ; m）。
    #[test]
    fn modify_other_keys() {
        let mut t = TerminalState::new(40, 5);
        assert_eq!(t.modify_other_keys, ModifyOtherKeys::Reset);
        t.feed(b"\x1b[>4;1m"); // enable except well-defined
        assert_eq!(
            t.modify_other_keys,
            ModifyOtherKeys::EnableExceptWellDefined
        );
        t.feed(b"\x1b[>4;2m"); // enable all
        assert_eq!(t.modify_other_keys, ModifyOtherKeys::EnableAll);
        t.feed(b"\x1b[>4;0m"); // reset
        assert_eq!(t.modify_other_keys, ModifyOtherKeys::Reset);
    }

    /// keyboard 协议状态不应影响文本渲染。
    #[test]
    fn keyboard_protocol_does_not_affect_text() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b[>1u\x1b[>4;1m");
        t.feed(b"text");
        assert_eq!(t.snapshot_trimmed(), vec!["text"]);
    }
}

#[cfg(test)]
mod charset_tests {
    use super::*;

    /// DEC line-drawing：ESC ( 0 指定 G0 为 special charset，之后打印映射为框线字符。
    #[test]
    fn dec_line_drawing_boxes() {
        let mut t = TerminalState::new(40, 5);
        // ESC ( 0 指定 G0 = special charset，然后打印 'q'（横线）'x'（竖线）
        t.feed(b"\x1b(0q x");
        assert_eq!(t.snapshot_trimmed(), vec!["─ │"]);
        // 回到 ASCII：ESC ( B
        t.feed(b"\x1b(B");
        t.feed(b" plain");
        assert_eq!(t.snapshot_trimmed(), vec!["─ │ plain"]);
    }

    /// SI/SO 切换 G0/G1。
    #[test]
    fn si_so_switch_charset() {
        let mut t = TerminalState::new(40, 5);
        // G0 = special, G1 = ASCII（默认）
        t.feed(b"\x1b(0"); // G0 = line drawing
        t.feed(b"\x1b)0"); // G1 = line drawing
                           // SO 切到 G1，打印 'q' → 横线
        t.feed(b"\x0e"); // SO
        t.feed(b"q");
        // SI 切回 G0，打印 'q' → 仍是横线（G0 也是 line drawing）
        t.feed(b"\x0f"); // SI
        t.feed(b"q");
        assert_eq!(t.snapshot_trimmed(), vec!["──"]);
    }
}

#[cfg(test)]
mod widechar_tests {
    use super::*;

    /// CJK 宽字符占 2 格：光标前进 2 列。
    #[test]
    fn cjk_wide_advances_two() {
        let mut t = TerminalState::new(10, 3);
        t.feed("中".as_bytes()); // 宽字符
        assert_eq!(t.cursor_col(), 2, "CJK 应前进 2 列");
        t.feed("a".as_bytes());
        assert_eq!(t.cursor_col(), 3);
        // 第 0 行第 0 格是宽字符，第 1 格是占位
        assert_eq!(t.line(0).chars().next(), Some('中'));
    }

    /// 组合字符（零宽）附着到前一个字符，不占格。
    #[test]
    fn combining_char_attaches() {
        let mut t = TerminalState::new(10, 3);
        t.feed("e\u{0301}".as_bytes()); // e + combining acute accent
                                        // e 占 1 格，组合符附着到它，光标应停在 e 之后（col 1）
        assert_eq!(t.cursor_col(), 1);
        let cell = t.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{0301}'); // combining char overwrote the cell content per our model
    }

    /// 宽字符与普通字符混排，列推进正确。
    #[test]
    fn mixed_wide_and_narrow() {
        let mut t = TerminalState::new(10, 3);
        t.feed("a中b".as_bytes());
        // a(col0) 中(col1,2) b(col3)
        assert_eq!(t.cursor_col(), 4);
    }
}

#[cfg(test)]
mod scrollback_tests {
    use super::*;

    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    /// 持续输出导致整屏上滚时，顶部行进入 scrollback。
    #[test]
    fn overflow_accumulates_scrollback() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"line1\r\nline2\r\nline3");
        assert_eq!(t.scrollback_lines(), 0, "未超屏前无 scrollback");
        t.feed(b"\r\nline4");
        // 超屏：line1 滚出屏幕进入 scrollback
        assert_eq!(t.scrollback_lines(), 1);
        assert_eq!(t.scrollback_line(0), Some("line1"));
        assert_eq!(snap(&t), vec!["line2", "line3", "line4"]);
    }

    /// scrollback 有上限（不无界增长）。
    #[test]
    fn scrollback_is_bounded() {
        let mut t = TerminalState::new(10, 2);
        for i in 0..(SCROLLBACK_MAX_LINES + 50) {
            t.feed(format!("row{i}\r\n").as_bytes());
        }
        assert!(
            t.scrollback_lines() <= SCROLLBACK_MAX_LINES,
            "scrollback 应有界"
        );
    }

    /// 清屏不应污染 scrollback（ESC[2J 只清当前屏）。
    #[test]
    fn clear_screen_does_not_pollute_scrollback() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"a\r\nb\r\nc");
        let before = t.scrollback_lines();
        t.feed(b"\x1b[2J"); // clear all
        assert_eq!(t.scrollback_lines(), before, "清屏不应改动 scrollback");
        assert_eq!(snap(&t), Vec::<String>::new());
    }
}
