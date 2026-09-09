//! TUI-owned ANSI screen emulator.
//!
//! The TUI is a frontend surface: it consumes raw pane bytes and keeps one
//! terminal state per pane. Core has another terminal model for Index and
//! activity processing, but the TUI must not render through that Core type.
//! This emulator therefore contains only the screen, cursor, style, and
//! parser-reply state needed by the ratatui surface.

use vte::ansi::{
    Attr, CharsetIndex, ClearMode, Color, CursorShape, Handler, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, ModifyOtherKeys, NamedColor, NamedPrivateMode,
    PrivateMode, Processor, Rgb, StandardCharset,
};

const OSC_FOREGROUND_INDEX: usize = NamedColor::Foreground as usize;
const OSC_BACKGROUND_INDEX: usize = NamedColor::Background as usize;
const OSC_CURSOR_INDEX: usize = NamedColor::Cursor as usize;

/// A single styled terminal cell.
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
            Attr::Foreground(color) => self.fg = Some(*color),
            Attr::Background(color) => self.bg = Some(*color),
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

/// A VTE-backed terminal state used only by the TUI surface.
pub struct TerminalState {
    grid: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: (usize, usize),
    attr: AttrState,
    pending_reply: Vec<u8>,
    fg_color: Rgb,
    bg_color: Rgb,
    cursor_color: Rgb,
    palette: [Rgb; 16],
    scroll_top: usize,
    scroll_bottom: usize,
    line_wrap: bool,
    wrap_pending: bool,
    pub alternate_screen: bool,
    pub show_cursor: bool,
    pub bracketed_paste: bool,
    pub mouse_reporting: bool,
    pub mouse_clicks: bool,
    pub mouse_button_motion: bool,
    pub mouse_all_motion: bool,
    pub mouse_sgr: bool,
    pub title: Option<String>,
    pub cursor_shape: CursorShape,
    pub cursor_blinking: bool,
    pub keyboard_mode: KeyboardModes,
    pub modify_other_keys: ModifyOtherKeys,
    pub charsets: [StandardCharset; 4],
    pub active_charset: CharsetIndex,
    processor: Processor,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl TerminalState {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            grid: vec![vec![Cell::blank(); cols]; rows],
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: (0, 0),
            attr: AttrState::default(),
            pending_reply: Vec::new(),
            fg_color: Rgb { r: 0, g: 0, b: 0 },
            bg_color: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            cursor_color: Rgb { r: 0, g: 0, b: 0 },
            palette: default_palette(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            line_wrap: true,
            wrap_pending: false,
            alternate_screen: false,
            show_cursor: true,
            bracketed_paste: false,
            mouse_reporting: false,
            mouse_clicks: false,
            mouse_button_motion: false,
            mouse_all_motion: false,
            mouse_sgr: false,
            title: None,
            cursor_shape: CursorShape::Block,
            cursor_blinking: false,
            keyboard_mode: KeyboardModes::default(),
            modify_other_keys: ModifyOtherKeys::Reset,
            charsets: [StandardCharset::default(); 4],
            active_charset: CharsetIndex::G0,
            processor: Processor::default(),
        }
    }

    pub fn cols(&self) -> usize {
        self.grid.first().map_or(0, Vec::len)
    }

    pub fn rows(&self) -> usize {
        self.grid.len()
    }

    pub fn cursor_row(&self) -> usize {
        self.cursor_row
    }

    pub fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_reply)
    }

    /// Feed raw bytes without converting them into text or a Core snapshot.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut processor = std::mem::take(&mut self.processor);
        for &byte in bytes {
            processor.advance(self, byte);
        }
        self.processor = processor;
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.grid
            .iter()
            .map(|row| row.iter().map(|cell| cell.ch).collect())
            .collect()
    }

    pub fn styled_screen(&self) -> Vec<Vec<Cell>> {
        self.grid.clone()
    }

    /// Resize in place and retain the current visible terminal state.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let old_rows = self.rows();

        for row in &mut self.grid {
            row.resize(cols, Cell::blank());
            row.truncate(cols);
        }

        if rows > old_rows {
            self.grid.resize(rows, vec![Cell::blank(); cols]);
        } else if rows < old_rows {
            if self.cursor_row >= old_rows.saturating_sub(1) {
                let start = old_rows - rows;
                self.grid.drain(..start);
                self.cursor_row = rows - 1;
            } else {
                self.grid.truncate(rows);
                self.cursor_row = self.cursor_row.min(rows - 1);
            }
        }

        self.scroll_top = self.scroll_top.min(rows - 1);
        if self.scroll_bottom >= rows || (rows > old_rows && self.scroll_bottom == old_rows - 1) {
            self.scroll_bottom = rows - 1;
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
            self.scroll_bottom = rows - 1;
        }
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.saved_cursor.0 = self.saved_cursor.0.min(rows - 1);
        self.saved_cursor.1 = self.saved_cursor.1.min(cols - 1);
        self.wrap_pending = false;
    }

    fn push_reply(&mut self, bytes: &[u8]) {
        self.pending_reply.extend_from_slice(bytes);
    }

    fn put_char(&mut self, ch: char) {
        if is_combining(ch) {
            let col = self.cursor_col.saturating_sub(1);
            if let Some(cell) = self
                .grid
                .get_mut(self.cursor_row)
                .and_then(|r| r.get_mut(col))
            {
                cell.ch = ch;
            }
            return;
        }

        if self.wrap_pending {
            self.wrap_pending = false;
            if self.line_wrap {
                self.linefeed_inner();
                self.carriage_return_inner();
            }
        }

        let charset = self
            .charsets
            .get(self.active_charset as usize)
            .copied()
            .unwrap_or_default();
        let mapped = charset.map(ch);
        if let Some(cell) = self
            .grid
            .get_mut(self.cursor_row)
            .and_then(|row| row.get_mut(self.cursor_col))
        {
            self.attr.apply_to(cell);
            cell.ch = mapped;
        }

        let advance = if is_wide(mapped) { 2 } else { 1 };
        for _ in 0..advance {
            if self.cursor_col + 1 < self.cols() {
                self.cursor_col += 1;
            } else if self.line_wrap {
                self.wrap_pending = true;
            }
        }
    }

    fn linefeed_inner(&mut self) {
        self.wrap_pending = false;
        if self.cursor_row < self.scroll_bottom {
            self.cursor_row += 1;
            return;
        }

        let top = self.scroll_top.min(self.rows().saturating_sub(1));
        let bottom = self
            .scroll_bottom
            .min(self.rows().saturating_sub(1))
            .max(top);
        if top < self.rows() && bottom < self.rows() {
            self.grid.remove(top);
            self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
        }
    }

    fn carriage_return_inner(&mut self) {
        self.wrap_pending = false;
        self.cursor_col = 0;
    }

    fn scroll_up_rows(&mut self, count: usize) {
        let span = self.scroll_bottom.saturating_sub(self.scroll_top) + 1;
        for _ in 0..count.min(span) {
            let top = self.scroll_top.min(self.rows().saturating_sub(1));
            let bottom = self
                .scroll_bottom
                .min(self.rows().saturating_sub(1))
                .max(top);
            if top < self.rows() && bottom < self.rows() {
                self.grid.remove(top);
                self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn scroll_down_rows(&mut self, count: usize) {
        let span = self.scroll_bottom.saturating_sub(self.scroll_top) + 1;
        for _ in 0..count.min(span) {
            let top = self.scroll_top.min(self.rows().saturating_sub(1));
            let bottom = self
                .scroll_bottom
                .min(self.rows().saturating_sub(1))
                .max(top);
            if top < self.rows() && bottom < self.rows() {
                self.grid.remove(bottom);
                self.grid.insert(top, vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn clear_screen_impl(&mut self, mode: ClearMode) {
        let (row, col) = (self.cursor_row, self.cursor_col);
        match mode {
            ClearMode::All | ClearMode::Saved => {
                for line in &mut self.grid {
                    for cell in line {
                        *cell = Cell::blank();
                    }
                }
            }
            ClearMode::Below => {
                if let Some(line) = self.grid.get_mut(row) {
                    for cell in line.iter_mut().skip(col) {
                        *cell = Cell::blank();
                    }
                }
                for line in self.grid.iter_mut().skip(row + 1) {
                    for cell in line {
                        *cell = Cell::blank();
                    }
                }
            }
            ClearMode::Above => {
                for line in self.grid.iter_mut().take(row) {
                    for cell in line {
                        *cell = Cell::blank();
                    }
                }
                if let Some(line) = self.grid.get_mut(row) {
                    for cell in line.iter_mut().take(col + 1) {
                        *cell = Cell::blank();
                    }
                }
            }
        }
    }

    fn clear_line_impl(&mut self, mode: LineClearMode) {
        let (row, col) = (self.cursor_row, self.cursor_col);
        if let Some(line) = self.grid.get_mut(row) {
            match mode {
                LineClearMode::Right => {
                    for cell in line.iter_mut().skip(col) {
                        *cell = Cell::blank();
                    }
                }
                LineClearMode::Left => {
                    for cell in line.iter_mut().take(col + 1) {
                        *cell = Cell::blank();
                    }
                }
                LineClearMode::All => {
                    for cell in line {
                        *cell = Cell::blank();
                    }
                }
            }
        }
    }

    fn erase_chars_impl(&mut self, count: usize) {
        let row = self.cursor_row;
        let col = self.cursor_col;
        if let Some(line) = self.grid.get_mut(row) {
            for cell in line.iter_mut().skip(col).take(count) {
                *cell = Cell::blank();
            }
        }
    }

    fn insert_blank_cells(&mut self, count: usize) {
        let row = self.cursor_row;
        let col = self.cursor_col;
        let cols = self.cols();
        if let Some(line) = self.grid.get_mut(row) {
            for _ in 0..count.min(cols.saturating_sub(col)) {
                line.pop();
                line.insert(col, Cell::blank());
            }
        }
    }

    fn delete_cells(&mut self, count: usize) {
        let row = self.cursor_row;
        let col = self.cursor_col;
        let cols = self.cols();
        if let Some(line) = self.grid.get_mut(row) {
            for _ in 0..count.min(cols.saturating_sub(col)) {
                line.remove(col);
                line.push(Cell::blank());
            }
        }
    }

    fn insert_blank_lines_impl(&mut self, count: usize) {
        let start = self.cursor_row.clamp(self.scroll_top, self.scroll_bottom);
        let count = count.min(self.scroll_bottom.saturating_sub(start) + 1);
        for _ in 0..count {
            let bottom = self.scroll_bottom.min(self.rows().saturating_sub(1));
            if start < self.rows() && bottom < self.rows() {
                self.grid.remove(bottom);
                self.grid.insert(start, vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn delete_lines_impl(&mut self, count: usize) {
        let start = self.cursor_row.clamp(self.scroll_top, self.scroll_bottom);
        let count = count.min(self.scroll_bottom.saturating_sub(start) + 1);
        for _ in 0..count {
            let bottom = self.scroll_bottom.min(self.rows().saturating_sub(1));
            if start < self.rows() && bottom < self.rows() {
                self.grid.remove(start);
                self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
            }
        }
    }

    fn sync_mouse_reporting(&mut self) {
        self.mouse_reporting =
            self.mouse_clicks || self.mouse_button_motion || self.mouse_all_motion;
    }

    fn color_reply(&mut self, prefix: &str, index: usize) {
        let color = if prefix.starts_with("4;") {
            self.palette.get(index).copied().unwrap_or_default()
        } else {
            match index {
                OSC_FOREGROUND_INDEX => self.fg_color,
                OSC_BACKGROUND_INDEX => self.bg_color,
                OSC_CURSOR_INDEX => self.cursor_color,
                _ => Rgb { r: 0, g: 0, b: 0 },
            }
        };
        let body = format!("{prefix};rgb:{}", xterm_rgb(color));
        self.push_reply(b"\x1b]");
        self.push_reply(body.as_bytes());
        self.push_reply(b"\x1b\\");
    }
}

impl Handler for TerminalState {
    fn input(&mut self, c: char) {
        self.put_char(c);
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        match intermediate {
            None | Some('?') => self.push_reply(b"\x1b[?65;4;1;2;6;21;22;17;28c"),
            Some('>') => self.push_reply(b"\x1b[>65;20;1c"),
            _ => {}
        }
    }

    fn device_status(&mut self, code: usize) {
        match code {
            5 => self.push_reply(b"\x1b[0n"),
            6 => {
                let reply = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                self.push_reply(reply.as_bytes());
            }
            _ => {}
        }
    }

    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, _terminator: &str) {
        self.color_reply(&prefix, index);
    }

    fn goto(&mut self, line: i32, col: usize) {
        self.wrap_pending = false;
        self.cursor_row = line.clamp(0, self.rows() as i32 - 1) as usize;
        self.cursor_col = col.min(self.cols().saturating_sub(1));
    }

    fn goto_line(&mut self, line: i32) {
        self.wrap_pending = false;
        self.cursor_row = line.clamp(0, self.rows() as i32 - 1) as usize;
    }

    fn goto_col(&mut self, col: usize) {
        self.wrap_pending = false;
        self.cursor_col = col.min(self.cols().saturating_sub(1));
    }

    fn insert_blank(&mut self, count: usize) {
        self.insert_blank_cells(count);
    }

    fn move_up(&mut self, count: usize) {
        self.wrap_pending = false;
        self.cursor_row = self.cursor_row.saturating_sub(count);
    }

    fn move_down(&mut self, count: usize) {
        self.wrap_pending = false;
        self.cursor_row = (self.cursor_row + count).min(self.rows().saturating_sub(1));
    }

    fn move_forward(&mut self, count: usize) {
        self.wrap_pending = false;
        self.cursor_col = (self.cursor_col + count).min(self.cols().saturating_sub(1));
    }

    fn move_backward(&mut self, count: usize) {
        self.wrap_pending = false;
        self.cursor_col = self.cursor_col.saturating_sub(count);
    }

    fn move_down_and_cr(&mut self, count: usize) {
        self.move_down(count);
        self.carriage_return_inner();
    }

    fn move_up_and_cr(&mut self, count: usize) {
        self.move_up(count);
        self.carriage_return_inner();
    }

    fn put_tab(&mut self, count: u16) {
        let next = ((self.cursor_col / 8) + usize::from(count)) * 8;
        self.cursor_col = next.min(self.cols().saturating_sub(1));
        self.wrap_pending = false;
    }

    fn backspace(&mut self) {
        self.wrap_pending = false;
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    fn carriage_return(&mut self) {
        self.carriage_return_inner();
    }

    fn linefeed(&mut self) {
        self.linefeed_inner();
    }

    fn newline(&mut self) {
        self.linefeed_inner();
        self.carriage_return_inner();
    }

    fn scroll_up(&mut self, count: usize) {
        self.scroll_up_rows(count);
    }

    fn scroll_down(&mut self, count: usize) {
        self.scroll_down_rows(count);
    }

    fn insert_blank_lines(&mut self, count: usize) {
        self.insert_blank_lines_impl(count);
    }

    fn delete_lines(&mut self, count: usize) {
        self.delete_lines_impl(count);
    }

    fn erase_chars(&mut self, count: usize) {
        self.erase_chars_impl(count);
    }

    fn delete_chars(&mut self, count: usize) {
        self.delete_cells(count);
    }

    fn save_cursor_position(&mut self) {
        self.saved_cursor = (self.cursor_row, self.cursor_col);
    }

    fn restore_cursor_position(&mut self) {
        self.cursor_row = self.saved_cursor.0.min(self.rows().saturating_sub(1));
        self.cursor_col = self.saved_cursor.1.min(self.cols().saturating_sub(1));
        self.wrap_pending = false;
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        self.clear_line_impl(mode);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        self.clear_screen_impl(mode);
    }

    fn reset_state(&mut self) {
        self.attr.reset();
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.saved_cursor = (0, 0);
        self.line_wrap = true;
        self.wrap_pending = false;
        self.show_cursor = true;
    }

    fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.cursor_row == self.scroll_top {
            self.scroll_down_rows(1);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.attr.apply(&attr);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        let PrivateMode::Named(mode) = mode else {
            return;
        };
        match mode {
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => self.alternate_screen = true,
            NamedPrivateMode::LineWrap => self.line_wrap = true,
            NamedPrivateMode::ShowCursor => self.show_cursor = true,
            NamedPrivateMode::BracketedPaste => self.bracketed_paste = true,
            NamedPrivateMode::ReportMouseClicks => {
                self.mouse_clicks = true;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportCellMouseMotion => {
                self.mouse_button_motion = true;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportAllMouseMotion => {
                self.mouse_all_motion = true;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::SgrMouse => self.mouse_sgr = true,
            _ => {}
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        let PrivateMode::Named(mode) = mode else {
            return;
        };
        match mode {
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => self.alternate_screen = false,
            NamedPrivateMode::LineWrap => self.line_wrap = false,
            NamedPrivateMode::ShowCursor => self.show_cursor = false,
            NamedPrivateMode::BracketedPaste => self.bracketed_paste = false,
            NamedPrivateMode::ReportMouseClicks => {
                self.mouse_clicks = false;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportCellMouseMotion => {
                self.mouse_button_motion = false;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportAllMouseMotion => {
                self.mouse_all_motion = false;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::SgrMouse => self.mouse_sgr = false,
            _ => {}
        }
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let rows = self.rows();
        self.scroll_top = top.saturating_sub(1).min(rows.saturating_sub(1));
        self.scroll_bottom = bottom
            .map(|line| line.saturating_sub(1))
            .unwrap_or(rows.saturating_sub(1))
            .min(rows.saturating_sub(1));
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
            self.scroll_bottom = rows.saturating_sub(1);
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.wrap_pending = false;
    }

    fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.cursor_shape = shape;
    }

    fn set_cursor_style(&mut self, style: Option<vte::ansi::CursorStyle>) {
        if let Some(style) = style {
            self.cursor_shape = style.shape;
            self.cursor_blinking = style.blinking;
        } else {
            self.cursor_shape = CursorShape::Block;
            self.cursor_blinking = false;
        }
    }

    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.active_charset = index;
    }

    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        if let Some(slot) = self.charsets.get_mut(index as usize) {
            *slot = charset;
        }
    }

    fn set_color(&mut self, index: usize, color: Rgb) {
        if let Some(slot) = self.palette.get_mut(index) {
            *slot = color;
        }
    }

    fn reset_color(&mut self, index: usize) {
        if index < self.palette.len() {
            self.palette[index] = default_palette()[index];
        }
    }

    fn set_hyperlink(&mut self, link: Option<vte::ansi::Hyperlink>) {
        self.attr.link = link.map(|link| link.uri);
    }

    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        self.keyboard_mode = match behavior {
            KeyboardModesApplyBehavior::Replace => mode,
            KeyboardModesApplyBehavior::Union => self.keyboard_mode | mode,
            KeyboardModesApplyBehavior::Difference => self.keyboard_mode & !mode,
        };
    }

    fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys) {
        self.modify_other_keys = mode;
    }
}

fn default_palette() -> [Rgb; 16] {
    [
        Rgb { r: 0, g: 0, b: 0 },
        Rgb { r: 205, g: 0, b: 0 },
        Rgb { r: 0, g: 205, b: 0 },
        Rgb {
            r: 205,
            g: 205,
            b: 0,
        },
        Rgb { r: 0, g: 0, b: 205 },
        Rgb {
            r: 205,
            g: 0,
            b: 205,
        },
        Rgb {
            r: 0,
            g: 205,
            b: 205,
        },
        Rgb {
            r: 229,
            g: 229,
            b: 229,
        },
        Rgb {
            r: 127,
            g: 127,
            b: 127,
        },
        Rgb { r: 255, g: 0, b: 0 },
        Rgb { r: 0, g: 255, b: 0 },
        Rgb {
            r: 255,
            g: 255,
            b: 0,
        },
        Rgb { r: 0, g: 0, b: 255 },
        Rgb {
            r: 255,
            g: 0,
            b: 255,
        },
        Rgb {
            r: 0,
            g: 255,
            b: 255,
        },
        Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    ]
}

fn xterm_rgb(rgb: Rgb) -> String {
    let duplicate = |value: u8| format!("{value:02x}{value:02x}");
    format!(
        "{}/{}/{}",
        duplicate(rgb.r),
        duplicate(rgb.g),
        duplicate(rgb.b)
    )
}

fn is_combining(ch: char) -> bool {
    matches!(
        ch,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

fn is_wide(ch: char) -> bool {
    matches!(
        ch,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_screen_and_preserves_styles() {
        let mut terminal = TerminalState::new(12, 3);
        terminal.feed(b"plain \x1b[31mred\x1b[0m");
        let screen = terminal.styled_screen();
        assert_eq!(screen[0][0].ch, 'p');
        assert_eq!(screen[0][6].fg, Some(Color::Named(NamedColor::Red)));
        assert_eq!(terminal.snapshot()[0].trim_end(), "plain red");
    }

    #[test]
    fn replies_to_terminal_queries() {
        let mut terminal = TerminalState::new(80, 24);
        terminal.feed(b"\x1b]10;?\x07\x1b[c");
        let reply = terminal.take_reply();
        assert!(reply.windows(7).any(|part| part == b"]10;rgb"));
        assert!(reply.windows(3).any(|part| part == b"65;"));
        assert!(terminal.take_reply().is_empty());
    }

    #[test]
    fn resize_retains_visible_tail() {
        let mut terminal = TerminalState::new(10, 4);
        terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
        terminal.resize(10, 2);
        let screen = terminal.snapshot();
        assert_eq!(screen[0].trim_end(), "three");
        assert_eq!(screen[1].trim_end(), "four");
    }
}
