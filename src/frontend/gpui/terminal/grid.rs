//! CellGrid：一个 pane 的单元格状态，frontend 侧的 VT 真相。
//!
//! 只服务于绘制：字符、前景/背景色、少量属性、光标、scrollback。
//! 不承担搜索、OSC 133、attention（那些归 Core Index）。

use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;

use super::mouse::MouseMode;

/// 一屏之外保留的历史行数上限。
pub const DEFAULT_SCROLLBACK: usize = 2_000;

/// 宽字符的占位格。SwiftTerm 也用 NUL 表示「这一格属于左边那个双宽字形」，
/// 因此 `screen_lines` 与选词都把它当 filler。
pub const FILLER: char = '\0';

/// 某个字符占几格。
///
/// 宽（CJK / emoji 呈现）算 2 格，组合标记与零宽字符算 0 格，其余 1 格。
/// SwiftTerm 用生成表 `UnicodeWidthData.eastAsianWide` 判宽，`unicode-width`
/// 是同一份 Unicode 数据的实现，结果一致（歧义宽度按窄算，与 SwiftTerm 相同）。
pub fn char_width(ch: char) -> usize {
    match UnicodeWidthChar::width(ch) {
        Some(0) => 0,
        Some(width) => width,
        None => 0,
    }
}

/// 单元格颜色。`Default` 表示跟随主题，不是黑色。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// 只保留绘制需要的 SGR 属性。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strike: bool,
}

/// 一个格子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attributes,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attributes::default(),
        }
    }
}

impl Cell {
    /// 该格是否需要绘制背景色。
    pub fn has_background(&self) -> bool {
        self.bg != Color::Default || self.attrs.reverse
    }

    /// 反显后的实际前景/背景。
    pub fn resolved(&self) -> (Color, Color) {
        if self.attrs.reverse {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        }
    }
}

/// 当前画笔（SGR 状态）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attributes,
}

impl Pen {
    fn cell(&self, ch: char) -> Cell {
        Cell {
            ch,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
        }
    }

    fn reset(&mut self) {
        *self = Pen::default();
    }
}

/// 一个 pane 的网格。
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    scrollback: VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: Option<(usize, usize, Pen)>,
    pen: Pen,
    /// 延迟换行：写满最后一列后光标仍停在该列，下一个字符才换行。
    wrap_pending: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    alt_screen: Option<AltScreen>,
    cursor_visible: bool,
    bracketed_paste: bool,
    app_cursor_keys: bool,
    mouse_mode: MouseMode,
    sgr_mouse: bool,
    title: String,
}

struct AltScreen {
    cells: Vec<Cell>,
    cursor: (usize, usize),
    pen: Pen,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            scrollback: VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK,
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: None,
            pen: Pen::default(),
            wrap_pending: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            alt_screen: None,
            cursor_visible: true,
            bracketed_paste: false,
            app_cursor_keys: false,
            mouse_mode: MouseMode::Off,
            sgr_mouse: false,
            title: String::new(),
        }
    }

    pub fn with_scrollback_limit(mut self, limit: usize) -> Self {
        self.scrollback_limit = limit;
        self
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    pub fn app_cursor_keys(&self) -> bool {
        self.app_cursor_keys
    }

    /// 当前是否在 alternate screen（全屏 TUI）。
    pub fn alternate_screen(&self) -> bool {
        self.alt_screen.is_some()
    }

    /// 应用要求的鼠标上报模式。
    pub fn mouse_mode(&self) -> MouseMode {
        self.mouse_mode
    }

    /// 是否用 SGR 编码上报（DECSET 1006）。
    pub fn sgr_mouse(&self) -> bool {
        self.sgr_mouse
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scrollback_row(&self, index: usize) -> Option<&[Cell]> {
        self.scrollback.get(index).map(Vec::as_slice)
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells.get(row * self.cols + col)
    }

    /// 可见屏幕的文本行：跳过占位格、行尾去空白、丢掉末尾空行。
    ///
    /// 移植自 `chrome/FlatChrome.swift` 的 `ScreenText.lines`；AX / agent
    /// 快照读的就是这个形状，占位 NUL 不能漏出去。
    pub fn screen_lines(&self) -> Vec<String> {
        screen_lines(self.cols, self.rows, |col, row| {
            self.cell(row, col).map_or(' ', |cell| cell.ch)
        })
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        let start = row.min(self.rows.saturating_sub(1)) * self.cols;
        &self.cells[start..start + self.cols]
    }

    /// 尺寸变化：保留左上内容，不重排换行（v1 限制，见 docs/FRONTEND-GPUI.md §8）。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut cells = vec![Cell::default(); cols * rows];
        let copy_rows = rows.min(self.rows);
        let copy_cols = cols.min(self.cols);
        for row in 0..copy_rows {
            let src = row * self.cols;
            let dst = row * cols;
            cells[dst..dst + copy_cols].copy_from_slice(&self.cells[src..src + copy_cols]);
        }
        self.cells = cells;
        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.wrap_pending = false;
    }

    pub(crate) fn print(&mut self, ch: char) {
        let width = char_width(ch);
        if width == 0 {
            // 组合标记 / 零宽字符：`Cell` 只存一个 `char`，暂不合并进前一个
            // 字形（SwiftTerm 存的是 Character 簇）。丢掉比写错格安全；
            // SwiftTerm 的 grapheme 缓存测试不在移植范围（见 README §6）。
            return;
        }
        if self.wrap_pending {
            self.cursor_col = 0;
            self.line_feed();
            self.wrap_pending = false;
        }
        // 双宽字符在最后一列放不下：先换行，不能把它劈成一半。
        if width == 2 && self.cursor_col + 2 > self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
        let index = self.cursor_row * self.cols + self.cursor_col;
        self.cells[index] = self.pen.cell(ch);
        if width == 2 {
            // 第二格是占位：绘制时跳过，提取文本时也跳过。
            self.cells[index + 1] = self.pen.cell(FILLER);
        }
        let next = self.cursor_col + width;
        if next >= self.cols {
            self.cursor_col = self.cols - 1;
            self.wrap_pending = true;
        } else {
            self.cursor_col = next;
        }
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor_col = 0;
        self.wrap_pending = false;
    }

    pub(crate) fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    pub(crate) fn backspace(&mut self) {
        self.wrap_pending = false;
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    pub(crate) fn horizontal_tab(&mut self) {
        let next = (self.cursor_col / 8 + 1) * 8;
        self.cursor_col = next.min(self.cols - 1);
        self.wrap_pending = false;
    }

    pub(crate) fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.cursor_row == self.scroll_top {
            self.scroll_down(1);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    /// 整屏向上滚动，`top` 行进历史。
    pub(crate) fn scroll_up(&mut self, count: usize) {
        self.scroll_region_up(self.scroll_top, self.scroll_bottom, count, true);
    }

    pub(crate) fn scroll_down(&mut self, count: usize) {
        self.scroll_region_down(self.scroll_top, self.scroll_bottom, count);
    }

    fn scroll_region_up(&mut self, top: usize, bottom: usize, count: usize, keep_history: bool) {
        let count = count.min(bottom - top + 1);
        for _ in 0..count {
            let start = top * self.cols;
            let end = (top + 1) * self.cols;
            let row: Vec<Cell> = self.cells[start..end].to_vec();
            // 只有整屏滚动才进历史；分区滚动与应用内的 IL/DL 不是历史。
            if keep_history && top == 0 && bottom + 1 == self.rows {
                self.push_scrollback(row);
            }
            self.cells.copy_within(
                (top + 1) * self.cols..(bottom + 1) * self.cols,
                top * self.cols,
            );
            for col in 0..self.cols {
                self.cells[bottom * self.cols + col] = self.pen.cell(' ');
            }
        }
    }

    fn scroll_region_down(&mut self, top: usize, bottom: usize, count: usize) {
        let count = count.min(bottom - top + 1);
        for _ in 0..count {
            self.cells
                .copy_within(top * self.cols..bottom * self.cols, (top + 1) * self.cols);
            for col in 0..self.cols {
                self.cells[top * self.cols + col] = self.pen.cell(' ');
            }
        }
    }

    /// 回填一批比现有 scrollback 更老的历史行（Core 的 `PaneHistory`）。
    ///
    /// 顺序按传入顺序（最老在前）；超出上限时丢最老的行，返回丢掉的条数，
    /// 调用方据此修正视口偏移。
    pub(crate) fn prepend_history(&mut self, rows: Vec<Vec<Cell>>) -> usize {
        if self.scrollback_limit == 0 || rows.is_empty() {
            return rows.len();
        }
        let mut all = rows;
        all.extend(self.scrollback.drain(..));
        let dropped = all.len().saturating_sub(self.scrollback_limit);
        all.drain(..dropped);
        self.scrollback = all.into();
        dropped
    }

    /// 追加一行历史（实时滚出屏幕的行）。只由 VT 路径调用。
    fn push_scrollback(&mut self, row: Vec<Cell>) {
        if self.scrollback_limit == 0 {
            return;
        }
        if self.scrollback.len() >= self.scrollback_limit {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(row);
    }

    pub(crate) fn reset(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
        self.pen.reset();
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.wrap_pending = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    pub(crate) fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
        self.wrap_pending = false;
    }

    pub(crate) fn move_cursor(&mut self, delta_row: isize, delta_col: isize) {
        let row = (self.cursor_row as isize + delta_row).clamp(0, self.rows as isize - 1) as usize;
        let col = (self.cursor_col as isize + delta_col).clamp(0, self.cols as isize - 1) as usize;
        self.set_cursor(row, col);
    }

    pub(crate) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top = top.min(self.rows - 1);
        let bottom = bottom.min(self.rows - 1);
        if top >= bottom {
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
            return;
        }
        self.scroll_top = top;
        self.scroll_bottom = bottom;
        self.set_cursor(0, 0);
    }

    pub(crate) fn clear_range(&mut self, start: usize, end: usize) {
        for cell in &mut self.cells[start..end] {
            *cell = self.pen.cell(' ');
        }
    }

    pub(crate) fn erase_display(&mut self, mode: u16) {
        let cursor = self.cursor_row * self.cols + self.cursor_col;
        match mode {
            0 => self.clear_range(cursor, self.cells.len()),
            1 => self.clear_range(0, cursor + 1),
            2 | 3 => self.clear_range(0, self.cells.len()),
            _ => {}
        }
    }

    pub(crate) fn erase_line(&mut self, mode: u16) {
        let row_start = self.cursor_row * self.cols;
        let row_end = row_start + self.cols;
        let cursor = row_start + self.cursor_col;
        match mode {
            0 => self.clear_range(cursor, row_end),
            1 => self.clear_range(row_start, cursor + 1),
            2 => self.clear_range(row_start, row_end),
            _ => {}
        }
    }

    /// IL：光标行起向下腾出 `count` 行，新行用当前画笔。
    pub(crate) fn insert_lines(&mut self, count: usize) {
        let count = count.min(self.scroll_bottom - self.cursor_row + 1);
        let top = self.cursor_row;
        let bottom = self.scroll_bottom;
        self.scroll_region_down(top, bottom, count);
    }

    pub(crate) fn delete_lines(&mut self, count: usize) {
        let count = count.min(self.scroll_bottom - self.cursor_row + 1);
        let top = self.cursor_row;
        let bottom = self.scroll_bottom;
        for _ in 0..count {
            self.cells.copy_within(
                (top + 1) * self.cols..(bottom + 1) * self.cols,
                top * self.cols,
            );
            for col in 0..self.cols {
                self.cells[bottom * self.cols + col] = self.pen.cell(' ');
            }
        }
    }

    pub(crate) fn insert_chars(&mut self, count: usize) {
        let count = count.min(self.cols - self.cursor_col);
        let row_start = self.cursor_row * self.cols;
        let from = row_start + self.cursor_col;
        let to = row_start + self.cols;
        self.cells.copy_within(from..to - count, from + count);
        self.clear_range(from, from + count);
    }

    pub(crate) fn delete_chars(&mut self, count: usize) {
        let count = count.min(self.cols - self.cursor_col);
        let row_start = self.cursor_row * self.cols;
        let from = row_start + self.cursor_col;
        let to = row_start + self.cols;
        self.cells.copy_within(from + count..to, from);
        self.clear_range(to - count, to);
    }

    pub(crate) fn erase_chars(&mut self, count: usize) {
        let count = count.min(self.cols - self.cursor_col);
        let start = self.cursor_row * self.cols + self.cursor_col;
        self.clear_range(start, start + count);
    }

    pub(crate) fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cursor_row, self.cursor_col, self.pen));
    }

    pub(crate) fn restore_cursor(&mut self) {
        if let Some((row, col, pen)) = self.saved_cursor {
            self.pen = pen;
            self.set_cursor(row, col);
        }
    }

    pub(crate) fn set_private_mode(&mut self, mode: u16, enabled: bool) {
        match mode {
            1 => self.app_cursor_keys = enabled,
            25 => self.cursor_visible = enabled,
            47 | 1047 | 1049 => self.set_alt_screen(mode, enabled),
            2004 => self.bracketed_paste = enabled,
            1006 => self.sgr_mouse = enabled,
            other => {
                // 9 / 1000 / 1002 / 1003 共用「最近一个模式生效」的语义：
                // 关掉当前模式才回到 Off，关别的模式不影响正在用的那个。
                if let Some(target) = MouseMode::from_mode_number(other) {
                    if enabled {
                        self.mouse_mode = target;
                    } else if self.mouse_mode == target {
                        self.mouse_mode = MouseMode::Off;
                    }
                }
            }
        }
    }

    fn set_alt_screen(&mut self, mode: u16, enabled: bool) {
        if enabled {
            if self.alt_screen.is_some() {
                return;
            }
            if mode == 1049 {
                self.save_cursor();
            }
            let saved = AltScreen {
                cells: self.cells.clone(),
                cursor: (self.cursor_row, self.cursor_col),
                pen: self.pen,
            };
            self.alt_screen = Some(saved);
            for cell in &mut self.cells {
                *cell = Cell::default();
            }
            if mode == 1049 {
                self.set_cursor(0, 0);
            }
        } else if let Some(saved) = self.alt_screen.take() {
            self.cells = saved.cells;
            self.cursor_row = saved.cursor.0.min(self.rows - 1);
            self.cursor_col = saved.cursor.1.min(self.cols - 1);
            self.pen = saved.pen;
            if mode == 1049 {
                self.restore_cursor();
            }
        }
    }

    pub(crate) fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub(crate) fn apply_sgr(&mut self, params: &[u16]) {
        let mut index = 0;
        if params.is_empty() {
            self.pen.reset();
            return;
        }
        while index < params.len() {
            let code = params[index];
            index += 1;
            match code {
                0 => self.pen.reset(),
                1 => self.pen.attrs.bold = true,
                2 => self.pen.attrs.dim = true,
                3 => self.pen.attrs.italic = true,
                4 => self.pen.attrs.underline = true,
                7 => self.pen.attrs.reverse = true,
                8 => self.pen.attrs.hidden = true,
                9 => self.pen.attrs.strike = true,
                21 | 22 => {
                    self.pen.attrs.bold = false;
                    self.pen.attrs.dim = false;
                }
                23 => self.pen.attrs.italic = false,
                24 => self.pen.attrs.underline = false,
                27 => self.pen.attrs.reverse = false,
                28 => self.pen.attrs.hidden = false,
                29 => self.pen.attrs.strike = false,
                30..=37 => self.pen.fg = Color::Indexed((code - 30) as u8),
                38 => {
                    if let Some((color, consumed)) = parse_extended_color(&params[index..]) {
                        self.pen.fg = color;
                        index += consumed;
                    } else {
                        index = params.len();
                    }
                }
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((code - 40) as u8),
                48 => {
                    if let Some((color, consumed)) = parse_extended_color(&params[index..]) {
                        self.pen.bg = color;
                        index += consumed;
                    } else {
                        index = params.len();
                    }
                }
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((code - 100 + 8) as u8),
                _ => {}
            }
        }
    }
}

/// `38;5;n` / `38;2;r;g;b`，返回颜色与消耗的参数个数。
fn parse_extended_color(params: &[u16]) -> Option<(Color, usize)> {
    match params.first()? {
        5 => {
            let index = *params.get(1)? as u8;
            Some((Color::Indexed(index), 2))
        }
        2 => {
            let r = *params.get(1)? as u8;
            let g = *params.get(2)? as u8;
            let b = *params.get(3)? as u8;
            Some((Color::Rgb(r, g, b), 4))
        }
        _ => None,
    }
}

/// 按行列读取器生成屏幕行。
///
/// `character_at` 的参数顺序是 `(col, row)`，与 Swift 的 `characterAt` 一致。
/// `cols`/`rows` 为 0 时返回空，不留一个空串。
pub fn screen_lines(
    cols: usize,
    rows: usize,
    character_at: impl Fn(usize, usize) -> char,
) -> Vec<String> {
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut line = String::with_capacity(cols);
        for col in 0..cols {
            let ch = character_at(col, row);
            if ch == FILLER {
                continue;
            }
            line.push(ch);
        }
        out.push(line.trim_end().to_string());
    }
    while matches!(out.last(), Some(line) if line.is_empty()) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cols: usize, rows: usize) -> Grid {
        Grid::new(cols, rows)
    }

    /// 对照 `testScreenTextSkipsWideCharStubs`：占位 NUL 不能漏进可见文本。
    #[test]
    fn screen_lines_skip_wide_char_stubs() {
        let cells = [['中', FILLER, '文', FILLER]];
        let lines = screen_lines(4, 1, |col, row| cells[row][col]);
        assert_eq!(lines, vec!["中文".to_string()]);
    }

    /// 对照 `testScreenTextTrimIsPerLine`（`A B` / 空行 / `CD`）。
    #[test]
    fn screen_lines_trim_per_line_and_drop_trailing_blanks() {
        let mut cells = [
            ['A', ' ', 'B', ' '],
            [' ', ' ', ' ', ' '],
            ['C', 'D', ' ', ' '],
            [' ', ' ', ' ', ' '],
        ];
        let lines = screen_lines(4, 4, |col, row| cells[row][col]);
        assert_eq!(lines, vec!["A B", "", "CD"]);
        assert!(!lines.iter().any(|line| line == " "), "行尾空白必须去掉");

        cells[0] = ['X', 'Y', 'Z', ' '];
        let updated = screen_lines(4, 4, |col, row| cells[row][col]);
        assert_eq!(updated, vec!["XYZ", "", "CD"]);
        assert!(
            !updated.iter().any(|line| line.contains("A B")),
            "旧屏幕不得残留"
        );
    }

    /// 对照 `testScreenTextHandlesZeroDims`。
    #[test]
    fn screen_lines_handle_zero_dims() {
        assert!(screen_lines(0, 10, |_, _| ' ').is_empty());
        assert!(screen_lines(10, 0, |_, _| ' ').is_empty());
    }

    #[test]
    fn wide_chars_occupy_two_cells() {
        let mut grid = grid(6, 2);
        grid.print('中');
        grid.print('文');
        assert_eq!(grid.cell(0, 0).unwrap().ch, '中');
        assert_eq!(grid.cell(0, 1).unwrap().ch, FILLER);
        assert_eq!(grid.cell(0, 2).unwrap().ch, '文');
        assert_eq!(grid.cell(0, 3).unwrap().ch, FILLER);
        assert_eq!(grid.cell(0, 4).unwrap().ch, ' ');
        assert_eq!(grid.cursor(), (0, 4));
        assert_eq!(grid.screen_lines(), vec!["中文".to_string()]);
    }

    /// 宽字符放不进最后一列时先换行，不能劈成一半。
    #[test]
    fn wide_char_wraps_instead_of_splitting_at_the_last_column() {
        let mut grid = grid(2, 2);
        grid.print('a');
        grid.print('中');
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'a');
        assert_eq!(grid.cell(0, 1).unwrap().ch, ' ');
        assert_eq!(grid.cell(1, 0).unwrap().ch, '中');
        assert_eq!(grid.cell(1, 1).unwrap().ch, FILLER);
        assert_eq!(grid.screen_lines(), vec!["a".to_string(), "中".to_string()]);
    }

    /// 宽字符正好填满最后两列：光标停在最后一列并延迟换行。
    #[test]
    fn wide_char_ending_the_row_defers_the_line_break() {
        let mut grid = grid(2, 2);
        grid.print('中');
        assert_eq!(grid.cursor(), (0, 1));
        grid.print('x');
        assert_eq!(grid.cell(1, 0).unwrap().ch, 'x');
    }

    #[test]
    fn char_width_matches_east_asian_rules() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('ア'), 2);
        assert_eq!(char_width('\u{0301}'), 0, "组合重音不占格");
        assert_eq!(char_width('\u{200b}'), 0, "零宽空格不占格");
    }

    /// 组合标记暂时被丢弃，不能污染格子（见 `print` 的注释）。
    #[test]
    fn combining_marks_do_not_consume_a_cell() {
        let mut grid = grid(4, 1);
        grid.print('e');
        grid.print('\u{0301}');
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'e');
        assert_eq!(grid.cell(0, 1).unwrap().ch, ' ');
        assert_eq!(grid.cursor(), (0, 1));
    }

    #[test]
    fn prints_and_advances() {
        let mut grid = grid(10, 3);
        grid.print('h');
        grid.print('i');
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'h');
        assert_eq!(grid.cell(0, 1).unwrap().ch, 'i');
        assert_eq!(grid.cursor(), (0, 2));
    }

    #[test]
    fn carriage_return_and_line_feed() {
        let mut grid = grid(10, 3);
        grid.print('a');
        grid.carriage_return();
        grid.line_feed();
        grid.print('b');
        assert_eq!(grid.cell(1, 0).unwrap().ch, 'b');
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'a');
    }

    #[test]
    fn deferred_wrap_on_last_column() {
        let mut grid = grid(4, 2);
        for ch in "abcd".chars() {
            grid.print(ch);
        }
        // 写满最后一列后光标仍停在最后一列。
        assert_eq!(grid.cursor(), (0, 3));
        grid.print('e');
        assert_eq!(grid.cell(1, 0).unwrap().ch, 'e');
    }

    #[test]
    fn line_feed_at_bottom_scrolls_into_scrollback() {
        let mut grid = grid(4, 2);
        grid.print('a');
        grid.line_feed();
        grid.print('b');
        grid.line_feed();
        grid.carriage_return();
        grid.print('c');
        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.scrollback_row(0).unwrap()[0].ch, 'a');
        assert_eq!(grid.cell(1, 0).unwrap().ch, 'c');
    }

    #[test]
    fn sgr_basic_and_extended_colors() {
        let mut grid = grid(10, 2);
        grid.apply_sgr(&[31]);
        assert_eq!(grid.pen.fg, Color::Indexed(1));
        grid.apply_sgr(&[38, 5, 196]);
        assert_eq!(grid.pen.fg, Color::Indexed(196));
        grid.apply_sgr(&[38, 2, 10, 20, 30]);
        assert_eq!(grid.pen.fg, Color::Rgb(10, 20, 30));
        grid.apply_sgr(&[0]);
        assert_eq!(grid.pen.fg, Color::Default);
        assert!(!grid.pen.attrs.bold);
    }

    #[test]
    fn sgr_bold_and_reverse() {
        let mut grid = grid(10, 2);
        grid.apply_sgr(&[1, 7]);
        assert!(grid.pen.attrs.bold);
        assert!(grid.pen.attrs.reverse);
        let cell = grid.pen.cell('x');
        let (fg, bg) = cell.resolved();
        assert_eq!(fg, Color::Default);
        assert_eq!(bg, Color::Default);
        grid.apply_sgr(&[22, 27]);
        assert!(!grid.pen.attrs.bold);
        assert!(!grid.pen.attrs.reverse);
    }

    #[test]
    fn erase_line_modes() {
        let mut grid = grid(4, 2);
        for ch in "abcd".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 2);
        grid.erase_line(0);
        assert_eq!(grid.cell(0, 1).unwrap().ch, 'b');
        assert_eq!(grid.cell(0, 2).unwrap().ch, ' ');
        assert_eq!(grid.cell(0, 3).unwrap().ch, ' ');
    }

    #[test]
    fn insert_and_delete_chars() {
        let mut grid = grid(6, 1);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 1);
        grid.delete_chars(2);
        let text: String = (0..6).map(|c| grid.cell(0, c).unwrap().ch).collect();
        assert_eq!(text, "adef  ");
        grid.set_cursor(0, 1);
        grid.insert_chars(2);
        let text: String = (0..6).map(|c| grid.cell(0, c).unwrap().ch).collect();
        assert_eq!(text, "a  def");
    }

    #[test]
    fn alt_screen_roundtrip() {
        let mut grid = grid(6, 2);
        grid.print('m');
        grid.set_private_mode(1049, true);
        assert_eq!(grid.cell(0, 0).unwrap().ch, ' ');
        grid.print('x');
        grid.set_private_mode(1049, false);
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'm');
    }

    #[test]
    fn resize_keeps_top_left_content() {
        let mut grid = grid(4, 2);
        for ch in "ab".chars() {
            grid.print(ch);
        }
        grid.resize(6, 3);
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'a');
        assert_eq!(grid.cell(0, 1).unwrap().ch, 'b');
        assert_eq!(grid.cols(), 6);
        assert_eq!(grid.rows(), 3);
    }

    #[test]
    fn resize_clamps_cursor() {
        let mut grid = grid(10, 5);
        grid.set_cursor(4, 9);
        grid.resize(3, 2);
        assert_eq!(grid.cursor(), (1, 2));
    }

    #[test]
    fn scroll_region_does_not_touch_scrollback() {
        let mut grid = grid(4, 4);
        grid.set_scroll_region(1, 2);
        grid.set_cursor(2, 0);
        grid.line_feed();
        assert_eq!(grid.scrollback_len(), 0);
    }
}
