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

use std::collections::{HashSet, VecDeque};

use crate::core::attention::signal::{AttentionSignal, AttentionSource};
use vte::ansi::{
    Attr, CharsetIndex, ClearMode, Color, CursorShape, Handler, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, ModifyOtherKeys, NamedColor, NamedPrivateMode,
    PrivateMode, Processor, Rgb, StandardCharset,
};

/// vte 用 `NamedColor` 索引表示 OSC 10-12 动态颜色。
const OSC_FOREGROUND_INDEX: usize = NamedColor::Foreground as usize;
const OSC_BACKGROUND_INDEX: usize = NamedColor::Background as usize;
const OSC_CURSOR_INDEX: usize = NamedColor::Cursor as usize;

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

/// 一条 scrollback 行：文本 + 稳定递增序号（淘汰后不重排）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollbackLine {
    pub text: String,
    pub seq: u64,
    /// 保留该行的样式 ANSI，供 Surface 首次播种原生 scrollback。
    pub ansi: Vec<u8>,
    /// 该行是否因软换行（写满末列后自动折行）产生；搜索时与下一行拼回逻辑行。
    pub soft_wrapped: bool,
}

/// 无头终端状态。
pub struct TerminalState {
    grid: Vec<Vec<Cell>>,
    /// 每个可见行的稳定 ID；行滚入 scrollback 时 ID 原样转移。
    grid_line_ids: Vec<u64>,
    /// 每行是否因软换行折行（与 grid 平行；搜索拼回逻辑行用）。
    grid_soft_wrapped: Vec<bool>,
    cursor_row: usize,
    cursor_col: usize,
    attr: AttrState,
    /// 待回写回 shell/pty 的查询应答字节（OSC 颜色 / CSI DA / DSR）。
    pending_reply: Vec<u8>,
    /// OSC 10 前景色（动态颜色查询用）。
    fg_color: Rgb,
    /// OSC 11 背景色。
    bg_color: Rgb,
    /// OSC 12 光标色。
    cursor_color: Rgb,
    /// 是否处于 alternate screen（`CSI ? 1049 h`）。
    pub alternate_screen: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    /// 是否自动换行。
    pub line_wrap: bool,
    /// 末列打印后待触发的自动换行（真实终端是延迟 wrap：下一个可打印字符才换行）。
    wrap_pending: bool,
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
    /// scrollback：从屏幕顶部滚出的行（带稳定 seq），有上限。
    pub scrollback: VecDeque<ScrollbackLine>,
    /// scrollback 上限（行数）。
    scrollback_max: usize,
    /// 下一条滚出行的 seq（从 1 起，淘汰后不回退）。
    next_seq: u64,
    /// 新建可见行使用的稳定 ID。
    next_line_id: u64,
    /// 本次 feed 中累计的注意力信号。
    signals: Vec<AttentionSignal>,
    /// 最近一次 feed 的原始字节（Index 自用；Surface 小终端按同一字节流播种）。
    last_raw_bytes: Vec<u8>,
    /// OSC 注意力收集器：是否刚看到 ESC（等待 `]` 或普通字符）。
    osc_esc_seen: bool,
    /// OSC 注意力收集器：尚未终止的 OSC 原始字节（含 ESC ] 前缀）。
    /// 与 vte 并行维护，支持跨 feed 截断（`\x1b]133` + `;C\x07`）。
    osc_pending: Option<Vec<u8>>,
    /// 当前激活字符集（SI/SO 切换）。
    pub active_charset: CharsetIndex,
    /// W18：OSC 133 命令回合（滚动条刻度）。Codex 必须在 A/B/C/D 时写入，不要永远空。
    command_marks: Vec<CommandMark>,
    /// B 与 C 之间收集的命令文本（W18h）。
    command_pending: Option<String>,
    /// 命令回合开始时的 seq（刻度跳转用）。
    command_start_seq: u64,
    processor: Processor,
}

/// 一条 shell 命令刻度：副本行号 + 命令文本 + 退出码（红/绿）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMark {
    /// 命令提示符所在的稳定终端行 ID。
    pub seq: u64,
    pub command: String,
    pub exit_code: Option<u8>,
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
    /// 创建指定行列数的状态模型（默认 scrollback 上限）。
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_scrollback(cols, rows, DEFAULT_SCROLLBACK_LINES)
    }

    /// 创建指定行列数与 scrollback 上限的状态模型。
    pub fn with_scrollback(cols: usize, rows: usize, max_lines: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            grid: vec![vec![Cell::blank(); cols]; rows],
            grid_line_ids: (1..=rows as u64).collect(),
            grid_soft_wrapped: vec![false; rows],
            cursor_row: 0,
            cursor_col: 0,
            attr: AttrState::default(),
            pending_reply: Vec::new(),
            fg_color: Rgb { r: 0, g: 0, b: 0 },
            bg_color: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            cursor_color: Rgb { r: 0, g: 0, b: 0 },
            alternate_screen: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            line_wrap: true,
            wrap_pending: false,
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
            scrollback: VecDeque::new(),
            scrollback_max: max_lines.max(1),
            next_seq: 1,
            next_line_id: rows as u64 + 1,
            signals: Vec::new(),
            last_raw_bytes: Vec::new(),
            osc_esc_seen: false,
            osc_pending: None,
            command_marks: Vec::new(),
            command_pending: None,
            command_start_seq: 0,
            processor: Processor::default(),
        }
    }

    /// W18：已完成的 OSC 133 命令回合（滚动条刻度数据源）。
    pub fn command_marks(&self) -> &[CommandMark] {
        &self.command_marks
    }

    /// 返回当前刻度之前最近的一条命令。
    /// `current_seq == 0` 表示从时间线尾部开始向前找。
    pub fn previous_command_mark(&self, current_seq: u64) -> Option<&CommandMark> {
        if current_seq == 0 {
            return self.command_marks.last();
        }
        self.command_marks
            .iter()
            .rev()
            .find(|mark| mark.seq < current_seq)
    }

    /// 返回当前刻度之后最近的一条命令。
    /// `current_seq == 0` 表示从时间线头部开始向后找。
    pub fn next_command_mark(&self, current_seq: u64) -> Option<&CommandMark> {
        if current_seq == 0 {
            return self.command_marks.first();
        }
        self.command_marks
            .iter()
            .find(|mark| mark.seq > current_seq)
    }

    /// 最近一次成功命令（退出码为 0）。
    pub fn last_successful_command(&self) -> Option<&CommandMark> {
        self.command_marks
            .iter()
            .rev()
            .find(|mark| mark.exit_code == Some(0))
    }

    /// 最近一次失败命令（存在且非 0 的退出码）。
    pub fn last_failed_command(&self) -> Option<&CommandMark> {
        self.command_marks
            .iter()
            .rev()
            .find(|mark| mark.exit_code.is_some_and(|code| code != 0))
    }

    /// 刻度必须与终端行生命周期一致。
    ///
    /// `CommandMark.seq` 指向真实 grid/scrollback 行；行被 bounded
    /// scrollback 淘汰、resize 删除或 DECSTBM 重排后，继续保留该 mark
    /// 会让 UI 把 stale seq 错误地当成 offset=0 跳转。
    fn prune_command_marks(&mut self) {
        let mut live = HashSet::with_capacity(self.grid_line_ids.len() + self.scrollback.len());
        live.extend(self.grid_line_ids.iter().copied());
        live.extend(self.scrollback.iter().map(|line| line.seq));
        self.command_marks.retain(|mark| live.contains(&mark.seq));
        if self.command_pending.is_some() && !live.contains(&self.command_start_seq) {
            self.command_pending = None;
            self.command_start_seq = 0;
        }
    }

    pub fn cols(&self) -> usize {
        self.grid.first().map(|r| r.len()).unwrap_or(0)
    }

    pub fn rows(&self) -> usize {
        self.grid.len()
    }

    /// 测试钩子：`grid_soft_wrapped` 必须与 `grid` 同行数，否则 DECSTBM/LF 会 panic。
    #[cfg(test)]
    pub(crate) fn soft_wrap_row_count(&self) -> usize {
        self.grid_soft_wrapped.len()
    }

    /// 当前光标行（0 基）。
    pub fn cursor_row(&self) -> usize {
        self.cursor_row
    }

    /// 当前光标列（0 基）。
    pub fn cursor_col(&self) -> usize {
        self.cursor_col
    }

    /// 取出待回写回 shell/pty 的查询应答字节，并清空内部队列。
    ///
    /// 前端在 `feed()` 输出后调用它，把返回字节经 `send_input` / WriteRaw
    /// 原样写回，否则 `git lg` 的 OSC 10/11 颜色查询与 CSI DA 会泄漏成
    /// 字面文本。
    pub fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_reply)
    }

    /// 取出本次 feed 累计的注意力信号，并清空队列。
    pub fn take_attention_signals(&mut self) -> Vec<AttentionSignal> {
        std::mem::take(&mut self.signals)
    }

    /// 运行时 resize：保留屏幕内容 / 光标 / 滚动区域，只调整行列。
    ///
    /// 不要在这里重建状态并重放累计输出——被截断的 ANSI 流从中间开始
    /// 解析会让 TUI 内容错乱。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let old_rows = self.rows();
        let old_cols = self.cols();
        if cols == old_cols && rows == old_rows {
            return;
        }

        // 先调整每行列数（截断或补空白，保留原单元格属性）。
        for row in &mut self.grid {
            if row.len() > cols {
                row.truncate(cols);
            } else if row.len() < cols {
                row.resize(cols, Cell::blank());
            }
        }

        if rows > old_rows {
            self.grid.resize(rows, vec![Cell::blank(); cols]);
            // 同步软换行标记：新行默认未软换行。
            self.grid_soft_wrapped.resize(rows, false);
            while self.grid_line_ids.len() < rows {
                let id = self.alloc_line_id();
                self.grid_line_ids.push(id);
            }
        } else if rows < old_rows {
            if self.cursor_row >= old_rows.saturating_sub(1) {
                // 光标在底行：保留屏幕底部（多数终端 resize 行为）。
                let start = old_rows - rows;
                self.grid.drain(..start);
                if self.grid_soft_wrapped.len() > start {
                    self.grid_soft_wrapped.drain(..start);
                }
                if self.grid_line_ids.len() > start {
                    self.grid_line_ids.drain(..start);
                }
                self.cursor_row = rows - 1;
            } else {
                self.grid.truncate(rows);
                self.grid_soft_wrapped.truncate(rows);
                self.grid_line_ids.truncate(rows);
            }
        }
        // 任何 resize 路径都要保证 soft-wrap 行数与 grid 一致，
        // 否则 agent 部分 DECSTBM + LF 会越界 panic（test-2026-0818-1114）。
        if self.grid_soft_wrapped.len() != self.grid.len() {
            self.grid_soft_wrapped.resize(self.grid.len(), false);
        }
        if self.grid_line_ids.len() != self.grid.len() {
            while self.grid_line_ids.len() < self.grid.len() {
                let id = self.alloc_line_id();
                self.grid_line_ids.push(id);
            }
            self.grid_line_ids.truncate(self.grid.len());
        }

        // 滚动区域：整屏区域随高度伸缩；部分区域收缩到底部后复位为整屏。
        self.scroll_top = self.scroll_top.min(rows - 1);
        if self.scroll_bottom >= rows || (rows > old_rows && self.scroll_bottom == old_rows - 1) {
            self.scroll_bottom = rows - 1;
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
            self.scroll_bottom = rows - 1;
        }

        self.wrap_pending = false;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.prune_command_marks();
    }

    fn alloc_line_id(&mut self) -> u64 {
        let id = self.next_line_id;
        self.next_line_id = self.next_line_id.saturating_add(1);
        id
    }

    /// 追加待回写的应答字节。
    fn push_reply(&mut self, bytes: &[u8]) {
        self.pending_reply.extend_from_slice(bytes);
    }

    /// OSC 颜色查询应答（OSC 4 / 10-12 的 `?` 形式）。
    ///
    /// 格式参考 xterm / wezterm：
    /// `ESC ] <prefix> ; rgb:RRRR/GGGG/BBBB ESC \`
    fn osc_color_reply(&mut self, prefix: &str, index: usize) {
        let color = if prefix.starts_with("4;") {
            // OSC 4 查询的是调色板索引（ANSI 16 + 256 色）。
            self.palette.get(index).copied().unwrap_or_default()
        } else {
            // OSC 10/11/12 查询动态前景/背景/光标色。
            match index {
                OSC_FOREGROUND_INDEX => self.fg_color,
                OSC_BACKGROUND_INDEX => self.bg_color,
                OSC_CURSOR_INDEX => self.cursor_color,
                _ => Rgb { r: 0, g: 0, b: 0 },
            }
        };
        let body = format!("{};rgb:{}", prefix, xterm_rgb(color));
        self.push_reply(b"\x1b]");
        self.push_reply(body.as_bytes());
        self.push_reply(b"\x1b\\");
    }

    /// 把原始字节喂给解析器。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.last_raw_bytes = bytes.to_vec();
        // 把 processor 临时取出来，避免与 `self`（作为 Handler）同时可变借用。
        let mut processor = std::mem::take(&mut self.processor);
        for &b in bytes {
            // vte 0.13 的 Handler 不暴露 osc_dispatch，OSC 由内部 Performer
            // 直接派发成 set_title/set_color 等；注意力 OSC 在这里并行收集。
            let terminated_osc = self.scan_attention_byte(b);
            // W18h：B..C 之间的命令文本（含混入的 C OSC 帧，C 处理时再剥掉）。
            // 终止 OSC 的 BEL/ST 不是命令文本，不收集。
            if !terminated_osc {
                if let Some(pending) = self.command_pending.as_mut() {
                    pending.push(b as char);
                }
            }
            processor.advance(self, b);
        }
        self.processor = processor;
        self.prune_command_marks();
    }

    /// 最近一次 feed 的原始字节（未解转义、未重编码）。
    pub fn raw_bytes(&self) -> &[u8] {
        &self.last_raw_bytes
    }

    /// OSC 注意力收集器（LINUX-PLAN §0.4）。
    ///
    /// 只认 `ESC ] ... BEL|ST` 的 OSC 帧：133 的 A/B/C/D/P 与
    /// 9/99/777/1337 通知类产生信号，其余原样留给 vte 处理。
    /// 返回 true 表示该字节终止了一条 OSC（BEL/ST），调用方不要把它当命令文本。
    fn scan_attention_byte(&mut self, b: u8) -> bool {
        if self.osc_pending.is_some() {
            let mut buf = self.osc_pending.take().expect("is_some 分支必须命中");
            let mut terminated = false;
            if self.osc_esc_seen {
                if b == b'\\' {
                    terminated = true;
                } else {
                    buf.push(b);
                }
                self.osc_esc_seen = false;
            } else if b == 0x1b {
                buf.push(b);
                self.osc_esc_seen = true;
            } else if b == 0x07 {
                terminated = true;
            } else {
                buf.push(b);
            }
            if terminated {
                self.process_attention_osc(buf);
            } else {
                self.osc_pending = Some(buf);
            }
            return terminated;
        }
        if self.osc_esc_seen {
            if b == b']' {
                self.osc_pending = Some(vec![0x1b, b']']);
                self.osc_esc_seen = false;
            } else {
                self.osc_esc_seen = false;
                // 普通 ESC + 非 `]`：不是 OSC，重新按当前字节处理。
                self.scan_attention_byte(b);
            }
            return false;
        }
        if b == 0x1b {
            self.osc_esc_seen = true;
        }
        false
    }

    /// 处理一条完整的注意力 OSC（其余 OSC 由 vte 正常处理，这里直接忽略）。
    fn process_attention_osc(&mut self, raw: Vec<u8>) {
        let body = raw.get(2..).unwrap_or(&[]);
        let params: Vec<&[u8]> = body.split(|&b| b == b';').collect();
        if params.is_empty() || params[0].is_empty() {
            return;
        }
        match params[0] {
            b"133" => {
                let code = params.get(1).and_then(|p| p.first()).copied();
                match code {
                    Some(b'B') => {
                        // 命令文本从 B 开始收集，到 C 结束（W18h）。
                        self.command_pending = Some(String::new());
                        self.command_start_seq = self
                            .grid_line_ids
                            .get(self.cursor_row)
                            .copied()
                            .unwrap_or(self.next_seq);
                    }
                    Some(b'C') => {
                        self.signals.push(AttentionSignal::CommandStart);
                        // B..C 之间的那一行就是命令文本；C 之后清空待收集。
                        if let Some(cmd) = self.command_pending.take() {
                            // 收集时把 C 的 OSC 帧也吞进来了，剥到 `ESC ]` 为止。
                            let cmd = cmd.split("\x1b]").next().unwrap_or("").trim().to_string();
                            if !cmd.is_empty() {
                                self.command_marks.push(CommandMark {
                                    seq: self.command_start_seq,
                                    command: cmd,
                                    exit_code: None,
                                });
                            }
                        }
                    }
                    Some(b'D') => {
                        // OSC 133;D;<exit>：退出码在第 3 段，解析整段（12 不能变 1）。
                        let exit = params.get(2).and_then(|p| {
                            std::str::from_utf8(p)
                                .ok()
                                .and_then(|s| s.parse::<u8>().ok())
                        });
                        self.signals
                            .push(AttentionSignal::CommandDone { exit_code: exit });
                        // 给 C 时已入队的刻度补退出码；没有 C 的 D 也补一条。
                        if let Some(last) = self.command_marks.last_mut() {
                            if last.exit_code.is_none() {
                                last.exit_code = exit;
                            }
                        } else {
                            self.command_marks.push(CommandMark {
                                seq: self
                                    .grid_line_ids
                                    .get(self.cursor_row)
                                    .copied()
                                    .unwrap_or(self.next_seq),
                                command: String::new(),
                                exit_code: exit,
                            });
                        }
                    }
                    Some(b'A' | b'P') => {
                        // prompt start：Working 尚未收到 D → 视为结束（无退出码）。
                        self.signals
                            .push(AttentionSignal::CommandDone { exit_code: None });
                    }
                    _ => {}
                }
            }
            b"9" | b"99" | b"777" | b"1337" => {
                self.signals.push(AttentionSignal::AttentionRequest {
                    source: AttentionSource::OscNotify,
                });
            }
            _ => {}
        }
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

    /// 返回整屏的带样式单元格（每行 Vec<Cell>），供 TUI 按颜色/样式渲染。
    pub fn styled_screen(&self) -> Vec<Vec<Cell>> {
        self.grid.clone()
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
        // 延迟 wrap：上一字符打在末列，这个字符才换行。
        if self.wrap_pending {
            self.wrap_pending = false;
            if self.line_wrap {
                self.linefeed_soft();
                self.carriage_return();
            }
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
                // 末列只挂起 wrap，等下一个可打印字符（CUP/CR/LF 会取消）。
                self.wrap_pending = true;
                if let Some(flag) = self.grid_soft_wrapped.get_mut(self.cursor_row) {
                    *flag = true;
                }
            } else {
                break;
            }
        }
    }

    fn linefeed(&mut self) {
        self.linefeed_inner(false);
    }

    /// 软换行（写满末列后自动折行）：滚出的行标记 soft_wrapped，搜索可拼回。
    fn linefeed_soft(&mut self) {
        self.linefeed_inner(true);
    }

    fn linefeed_inner(&mut self, soft: bool) {
        self.wrap_pending = false;
        if self.cursor_row < self.scroll_bottom {
            self.cursor_row += 1;
        } else {
            let top = self.scroll_top;
            let bottom = self.scroll_bottom;
            if top == 0 && bottom == self.rows() - 1 {
                // 整屏上滚：滚出顶行的内容进入 scrollback
                if let Some(evicted) = self.grid.first().cloned() {
                    // 去掉行尾空白，保持 scrollback 可读
                    let was_soft = self.grid_soft_wrapped.first().copied().unwrap_or(false);
                    let seq = self.grid_line_ids.first().copied().unwrap_or(self.next_seq);
                    self.push_scrollback(&evicted, seq, soft || was_soft);
                }
                // 先取列数：rows=1 时 remove(0) 会让 grid 暂时为空，cols() 返回 0。
                let cols = self.cols();
                self.grid.remove(0);
                self.grid_soft_wrapped.remove(0);
                self.grid_line_ids.remove(0);
                self.grid.push(vec![Cell::blank(); cols]);
                self.grid_soft_wrapped.push(false);
                let id = self.alloc_line_id();
                self.grid_line_ids.push(id);
            } else {
                // 部分滚动区域（DECSTBM）：区域顶行滚出、区域底行补空，
                // 区域外的行不能动。htop 正是靠这个固定表头/表尾只滚动正文。
                // index 必须先 clamp 到 grid/soft 的实际行数，防止 resize
                // 时序（soft 未同步 / 缩到更小）越界 panic。
                let rows = self.grid.len();
                let soft_len = self.grid_soft_wrapped.len();
                let top = top
                    .min(rows.saturating_sub(1))
                    .min(soft_len.saturating_sub(1));
                let bottom = bottom
                    .min(rows.saturating_sub(1))
                    .min(soft_len.saturating_sub(1))
                    .max(top);
                if top < rows && bottom < rows && top < soft_len && bottom < soft_len {
                    self.grid.remove(top);
                    self.grid_soft_wrapped.remove(top);
                    self.grid_line_ids.remove(top);
                    self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
                    self.grid_soft_wrapped.insert(bottom, false);
                    let id = self.alloc_line_id();
                    self.grid_line_ids.insert(bottom, id);
                }
            }
        }
    }

    fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.cursor_col = 0;
    }

    /// 把一行推入 scrollback（按 `with_scrollback` 上限截断，seq 单调递增）。
    fn push_scrollback(&mut self, cells: &[Cell], seq: u64, soft_wrapped: bool) {
        if self.scrollback.len() >= self.scrollback_max {
            self.scrollback.pop_front();
        }
        self.next_seq = self.next_seq.max(seq.saturating_add(1));
        let text: String = cells.iter().map(|c| c.ch).collect();
        self.scrollback.push_back(ScrollbackLine {
            text: text.trim_end().to_string(),
            seq,
            ansi: encode_cells_ansi(cells),
            soft_wrapped,
        });
    }

    /// scrollback 行数。
    pub fn scrollback_lines(&self) -> usize {
        self.scrollback.len()
    }

    /// 当前实例配置的 scrollback 最大行数。
    pub fn scrollback_capacity(&self) -> usize {
        self.scrollback_max
    }

    /// 取第 idx 行 scrollback（0 = 最早）。
    pub fn scrollback_line(&self, idx: usize) -> Option<&str> {
        self.scrollback.get(idx).map(|s| s.text.as_str())
    }

    /// 取第 idx 行 scrollback 条目（文本 + seq）。
    pub fn scrollback_entry(&self, idx: usize) -> Option<&ScrollbackLine> {
        self.scrollback.get(idx)
    }

    /// 最新一条 scrollback 的 seq（无行时为 0）。
    pub fn latest_seq(&self) -> u64 {
        self.scrollback.back().map(|l| l.seq).unwrap_or(0)
    }

    /// 当前终端中最新的稳定行 ID（可见屏和 scrollback 均考虑）。
    pub fn latest_line_seq(&self) -> u64 {
        self.grid_line_ids
            .iter()
            .copied()
            .chain(self.scrollback.iter().map(|line| line.seq))
            .max()
            .unwrap_or(0)
    }

    /// 大小写敏感子串搜索，返回 (seq, 行文本)；空 query 返回空。
    /// 覆盖 scrollback + 可见屏（可见行用 next_seq 起的稳定 seq）。
    pub fn search(&self, query: &str) -> Vec<(u64, String)> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(u64, String)> = Vec::new();
        // scrollback：软换行拼回逻辑行再匹配（W18g 长 token 折行后仍可搜到）。
        let mut logical = String::new();
        let mut logical_seq = 0u64;
        for l in &self.scrollback {
            if logical.is_empty() {
                logical_seq = l.seq;
            }
            logical.push_str(&l.text);
            if !l.soft_wrapped {
                if logical.contains(query) {
                    out.push((logical_seq, logical.clone()));
                }
                logical.clear();
            }
        }
        if !logical.is_empty() && logical.contains(query) {
            out.push((logical_seq, logical.clone()));
        }
        // 可见屏：软换行拼回逻辑行再匹配。
        let mut logical = String::new();
        let mut logical_seq = self.next_seq;
        for (i, line) in self.snapshot_trimmed().into_iter().enumerate() {
            let seq = self.grid_line_ids.get(i).copied().unwrap_or(self.next_seq);
            if logical.is_empty() {
                logical_seq = seq;
            }
            logical.push_str(&line);
            let soft = self.grid_soft_wrapped.get(i).copied().unwrap_or(false);
            if !soft {
                if logical.contains(query) {
                    out.push((logical_seq, logical.clone()));
                }
                logical.clear();
            }
        }
        if !logical.is_empty() && logical.contains(query) {
            out.push((logical_seq, logical.clone()));
        }
        out
    }

    /// 最近 n 行：先取可见屏 `snapshot_trimmed()` 尾部，不足再向前取 scrollback。
    /// 某 seq 的历史行索引（0 = 最老一行）；可见屏行返回其历史末端索引。
    pub fn line_index_by_seq(&self, seq: u64) -> Option<usize> {
        if let Some(index) = self.scrollback.iter().position(|l| l.seq == seq) {
            return Some(index);
        }
        self.grid_line_ids
            .iter()
            .position(|id| *id == seq)
            .map(|index| self.scrollback.len() + index)
    }

    pub fn last_n_lines(&self, n: usize) -> Vec<String> {
        let mut out: Vec<String> = self.snapshot_trimmed();
        let mut need = n.saturating_sub(out.len());
        let mut idx = self.scrollback.len();
        while need > 0 && idx > 0 {
            idx -= 1;
            out.insert(0, self.scrollback[idx].text.clone());
            need -= 1;
        }
        // 保留合并后的尾部 n 行（不是头部）。
        if out.len() > n {
            out.drain(..out.len() - n);
        }
        out
    }

    /// 最近一条非空行：先看可见屏，再回退 scrollback。
    pub fn last_non_empty_line(&self) -> Option<String> {
        self.snapshot_trimmed()
            .into_iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .or_else(|| {
                self.scrollback
                    .iter()
                    .rev()
                    .map(|l| l.text.clone())
                    .find(|l| !l.trim().is_empty())
            })
    }

    /// 可见屏快照（去行尾空白与末尾空行）。
    pub fn visible_snapshot(&self) -> Vec<String> {
        self.snapshot_trimmed()
    }

    /// 滚动窗口：从 scrollback + 可见屏取 `rows` 行，`offset` 行前（0=底部）。
    pub fn scroll_window(&self, offset: u32, rows: usize) -> Vec<String> {
        let mut history: Vec<String> = self.scrollback.iter().map(|l| l.text.clone()).collect();
        history.extend(self.snapshot_trimmed());
        let rows = rows.max(1);
        if history.is_empty() {
            return Vec::new();
        }
        // 滚过头时停在顶部：offset 超过历史长度 → 显示最前 rows 行。
        let offset = (offset as usize).min(history.len().saturating_sub(rows));
        let end = history.len() - offset;
        let start = end.saturating_sub(rows);
        history[start..end].to_vec()
    }

    /// 当前 pane 的一次性 Surface seed：历史行先进入 scrollback，随后是当前屏。
    /// 这里故意不带 RIS/CUP 风暴；调用方只在新 VT 上 reset 一次后 feed。
    pub fn surface_seed_ansi(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for line in self.scrollback.iter() {
            out.extend_from_slice(&line.ansi);
            if !line.soft_wrapped {
                out.extend_from_slice(b"\r\n");
            }
        }
        // 每个历史行都必须真正离开 native 可见屏。历史最后一行写完后，
        // 补 rows 个空换行，正好让 scrollback 行数与 core 一致；随后用
        // CUP/DECAWM-off 的 overlay 写回当前屏，不能再顺序写 grid 造成
        // 额外的 scrollback 或把当前屏当历史重复一遍。
        for _ in 0..self.rows() {
            out.extend_from_slice(b"\r\n");
        }
        // 覆盖当前屏的几何内容，不能清空前面刚建立的 scrollback。
        out.extend_from_slice(&self.visible_overlay_ansi());
        out
    }

    /// 当前网格覆盖 ANSI；与 `visible_ansi` 不同，不执行 RIS/ED。
    pub fn visible_overlay_ansi(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[H\x1b[?7l");
        for (row_idx, row) in self.grid.iter().enumerate() {
            out.extend_from_slice(format!("\x1b[{};1H", row_idx + 1).as_bytes());
            out.extend_from_slice(&encode_cells_ansi(row));
        }
        out.extend_from_slice(
            format!(
                "\x1b[{};{}H\x1b[?7h",
                self.cursor_row + 1,
                self.cursor_col + 1
            )
            .as_bytes(),
        );
        out
    }

    /// 网格是否全空（没有任何非空白单元格）。
    pub fn is_blank(&self) -> bool {
        self.grid
            .iter()
            .all(|row| row.iter().all(|c| c.ch == ' ' || c.ch == '\0'))
    }

    /// 把当前可见网格编成可再 feed 的 ANSI 字节（LINUX-PLAN §4 D1）。
    ///
    /// 几何 dump：`ESC[H ESC[2J` 后对每一行（1-based，**含空行**）发
    /// `ESC[{row};1H` 再输出恰好 `cols` 个单元格（空格保留，**不 trim**）；
    /// 颜色/加粗在变化处插 SGR（0 / 1 / 30-37 / 40-47 / 38;2 / 48;2 / 38;5 / 48;5）。
    /// 全屏 TUI 靠空行/空格撑几何，shell 提示符在底行——skip/trim 会挤碎。
    pub fn visible_ansi(&self) -> Vec<u8> {
        let cols = self.cols();
        let rows = self.rows();
        let mut out = Vec::with_capacity(cols * rows * 8);
        // 关自动换行：写满 cols 个单元格时第 cols 个会触发 linefeed 滚屏，
        // 把首行挤掉。dump 期间 DECAWM off，写完恢复。
        out.extend_from_slice(b"\x1b[H\x1b[2J\x1b[?7l");
        let mut last_fg: Option<String> = None;
        let mut last_bg: Option<String> = None;
        let mut last_bold = false;
        for (row_idx, row) in self.grid.iter().enumerate() {
            // 每行独立 CUP：`ESC[{row};1H`（1-based）。
            out.extend_from_slice(format!("\x1b[{};1H", row_idx + 1).as_bytes());
            let mut fg: Option<String> = None;
            let mut bg: Option<String> = None;
            let mut bold = false;
            for (col_idx, cell) in row.iter().enumerate() {
                if cell.fg.is_some() || cell.bg.is_some() || cell.bold {
                    fg = cell.fg.as_ref().and_then(sgr_fg);
                    bg = cell.bg.as_ref().and_then(sgr_bg);
                    bold = cell.bold;
                }
                if fg != last_fg || bg != last_bg || bold != last_bold {
                    out.extend_from_slice(b"\x1b[");
                    let mut parts: Vec<String> = Vec::new();
                    if bold {
                        parts.push("1".into());
                    } else if last_bold {
                        parts.push("0".into());
                    }
                    if let Some(f) = &fg {
                        parts.push(f.clone());
                    }
                    if let Some(b) = &bg {
                        parts.push(b.clone());
                    }
                    if parts.is_empty() {
                        parts.push("0".into());
                    }
                    out.extend_from_slice(parts.join(";").as_bytes());
                    out.push(b'm');
                    last_fg = fg.clone();
                    last_bg = bg.clone();
                    last_bold = bold;
                }
                // 整行单元格：NUL 当空格，其余原样（不 trim）。
                if cell.ch == '\0' {
                    out.push(b' ');
                } else {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(cell.ch.encode_utf8(&mut buf).as_bytes());
                }
                // 最后一个单元格前先 CUP 到末列，避免写满触发 wrap。
                if col_idx + 1 == cols {
                    out.extend_from_slice(format!("\x1b[{};{}H", row_idx + 1, cols).as_bytes());
                }
            }
        }
        out.extend_from_slice(b"\x1b[?7h");
        out
    }

    fn scroll_up_n(&mut self, n: usize) {
        let span = self.scroll_bottom.saturating_sub(self.scroll_top) + 1;
        let n = n.min(span);
        for _ in 0..n {
            let rows = self.grid.len();
            let soft_len = self.grid_soft_wrapped.len();
            let top = self
                .scroll_top
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1));
            let bottom = self
                .scroll_bottom
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1))
                .max(top);
            if top < rows && bottom < rows && top < soft_len && bottom < soft_len {
                self.grid.remove(top);
                self.grid_soft_wrapped.remove(top);
                self.grid_line_ids.remove(top);
                self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
                self.grid_soft_wrapped.insert(bottom, false);
                let id = self.alloc_line_id();
                self.grid_line_ids.insert(bottom, id);
            }
        }
    }

    fn scroll_down_n(&mut self, n: usize) {
        let span = self.scroll_bottom.saturating_sub(self.scroll_top) + 1;
        let n = n.min(span);
        for _ in 0..n {
            let rows = self.grid.len();
            let soft_len = self.grid_soft_wrapped.len();
            let top = self
                .scroll_top
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1));
            let bottom = self
                .scroll_bottom
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1))
                .max(top);
            if top < rows && bottom < rows && top < soft_len && bottom < soft_len {
                self.grid.remove(bottom);
                self.grid_soft_wrapped.remove(bottom);
                self.grid_line_ids.remove(bottom);
                self.grid.insert(top, vec![Cell::blank(); self.cols()]);
                self.grid_soft_wrapped.insert(top, false);
                let id = self.alloc_line_id();
                self.grid_line_ids.insert(top, id);
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
            let rows = self.grid.len();
            let soft_len = self.grid_soft_wrapped.len();
            let top = self.scroll_top.min(rows).min(soft_len);
            let bottom = self
                .scroll_bottom
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1));
            if soft_len > bottom && bottom < rows {
                self.grid.remove(bottom);
                self.grid_soft_wrapped.remove(bottom);
                self.grid_line_ids.remove(bottom);
            }
            if top <= rows && top <= soft_len {
                self.grid.insert(top, vec![Cell::blank(); self.cols()]);
                self.grid_soft_wrapped.insert(top, false);
                let id = self.alloc_line_id();
                self.grid_line_ids.insert(top, id);
            }
        }
    }

    fn delete_lines(&mut self, n: usize) {
        let n = n.min(self.rows());
        for _ in 0..n {
            let rows = self.grid.len();
            let soft_len = self.grid_soft_wrapped.len();
            let top = self
                .scroll_top
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1));
            let bottom = self
                .scroll_bottom
                .min(rows.saturating_sub(1))
                .min(soft_len.saturating_sub(1))
                .max(top);
            if top < rows && bottom < rows && top < soft_len && bottom < soft_len {
                self.grid.remove(top);
                self.grid_soft_wrapped.remove(top);
                self.grid_line_ids.remove(top);
                self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
                self.grid_soft_wrapped.insert(bottom, false);
                let id = self.alloc_line_id();
                self.grid_line_ids.insert(bottom, id);
            }
        }
    }
}

/// 生产默认 scrollback 行数（LINUX-PLAN §2.4）。
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

/// 旧测试用硬编码上限；生产路径已由 `with_scrollback` 取代。
#[cfg(test)]
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

    /// CSI DA / DECID：识别终端。
    fn identify_terminal(&mut self, intermediate: Option<char>) {
        match intermediate {
            None | Some('?') => {
                // Primary DA：VT525 兼容标识 + 常用属性（同 macOS 侧实现）。
                self.push_reply(b"\x1b[?65;4;1;2;6;21;22;17;28c");
            }
            Some('>') => {
                // Secondary DA：VT525 + PC 键盘。
                self.push_reply(b"\x1b[>65;20;1c");
            }
            _ => {}
        }
    }

    /// DSR：设备状态 / 光标位置报告。
    fn device_status(&mut self, code: usize) {
        match code {
            5 => self.push_reply(b"\x1b[0n"),
            6 => {
                let pos = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                self.push_reply(pos.as_bytes());
            }
            _ => {}
        }
    }

    /// OSC 颜色查询（`OSC 4 ; n ; ?` / `OSC 10..12 ; ?`）。
    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, _terminator: &str) {
        self.osc_color_reply(&prefix, index);
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

    fn insert_blank(&mut self, n: usize) {
        self.insert_blank(n);
    }

    fn move_up(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_row = self.cursor_row.saturating_sub(n.min(self.cursor_row));
    }

    fn move_down(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_row = (self.cursor_row + n).min(self.rows() - 1);
    }

    fn move_forward(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_col = (self.cursor_col + n).min(self.cols() - 1);
    }

    fn move_backward(&mut self, n: usize) {
        self.wrap_pending = false;
        self.cursor_col = self.cursor_col.saturating_sub(n.min(self.cursor_col));
    }

    fn move_down_and_cr(&mut self, n: usize) {
        self.wrap_pending = false;
        self.move_down(n);
        self.cursor_col = 0;
    }

    fn move_up_and_cr(&mut self, n: usize) {
        self.wrap_pending = false;
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
        self.wrap_pending = false;
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
        self.wrap_pending = false;
        self.show_cursor = true;
    }

    fn reverse_index(&mut self) {
        self.wrap_pending = false;
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
        self.wrap_pending = false;
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

    /// BEL：进程求关注 → Blocked。
    fn bell(&mut self) {
        self.signals.push(AttentionSignal::AttentionRequest {
            source: AttentionSource::Bel,
        });
    }
}

/// 前景 SGR 参数：Named 0-7 → 30-37，bright 8-15 → 90-97，
/// Indexed → 38;5;n，Spec 真彩 → 38;2;r;g;b。
fn sgr_fg(color: &Color) -> Option<String> {
    match color {
        Color::Named(n) => {
            let v = *n as u8;
            if v < 8 {
                Some((30 + v).to_string())
            } else if v < 16 {
                Some((90 + (v - 8)).to_string())
            } else {
                None
            }
        }
        Color::Indexed(n) => Some(format!("38;5;{n}")),
        Color::Spec(Rgb { r, g, b }) => Some(format!("38;2;{r};{g};{b}")),
    }
}

/// 背景 SGR 参数：Named 0-7 → 40-47，bright 8-15 → 100-107，
/// Indexed → 48;5;n，Spec 真彩 → 48;2;r;g;b。
fn sgr_bg(color: &Color) -> Option<String> {
    match color {
        Color::Named(n) => {
            let v = *n as u8;
            if v < 8 {
                Some((40 + v).to_string())
            } else if v < 16 {
                Some((100 + (v - 8)).to_string())
            } else {
                None
            }
        }
        Color::Indexed(n) => Some(format!("48;5;{n}")),
        Color::Spec(Rgb { r, g, b }) => Some(format!("48;2;{r};{g};{b}")),
    }
}

/// 把一行 cell 编成可直接喂给另一个 VT 的 ANSI。
///
/// 每行从 SGR reset 开始，避免上一行的颜色/样式泄漏；这只用于一次性
/// Surface seed，不用于 live `%output`。
fn encode_cells_ansi(cells: &[Cell]) -> Vec<u8> {
    #[derive(Clone, PartialEq, Eq)]
    struct Style {
        fg: Option<Color>,
        bg: Option<Color>,
        bold: bool,
        underline: bool,
        reverse: bool,
        strike: bool,
        hidden: bool,
        link: Option<String>,
    }

    let style_of = |cell: &Cell| Style {
        fg: cell.fg,
        bg: cell.bg,
        bold: cell.bold,
        underline: cell.underline,
        reverse: cell.reverse,
        strike: cell.strike,
        hidden: cell.hidden,
        link: cell.link.clone(),
    };

    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b[0m");
    let mut previous: Option<Style> = None;
    for cell in cells {
        let style = style_of(cell);
        if previous.as_ref() != Some(&style) {
            out.extend_from_slice(b"\x1b[0m");
            let mut attrs = Vec::new();
            if style.bold {
                attrs.push("1".to_string());
            }
            if style.underline {
                attrs.push("4".to_string());
            }
            if style.reverse {
                attrs.push("7".to_string());
            }
            if style.strike {
                attrs.push("9".to_string());
            }
            if style.hidden {
                attrs.push("8".to_string());
            }
            if let Some(fg) = style.fg.as_ref().and_then(sgr_fg) {
                attrs.push(fg);
            }
            if let Some(bg) = style.bg.as_ref().and_then(sgr_bg) {
                attrs.push(bg);
            }
            if !attrs.is_empty() {
                out.extend_from_slice(b"\x1b[");
                out.extend_from_slice(attrs.join(";").as_bytes());
                out.push(b'm');
            }
            if previous.as_ref().and_then(|s| s.link.as_ref()) != style.link.as_ref() {
                if previous.as_ref().and_then(|s| s.link.as_ref()).is_some() {
                    out.extend_from_slice(b"\x1b]8;;\x1b\\");
                }
                if let Some(link) = &style.link {
                    out.extend_from_slice(b"\x1b]8;;");
                    out.extend_from_slice(link.as_bytes());
                    out.extend_from_slice(b"\x1b\\");
                }
            }
            previous = Some(style);
        }
        let ch = if cell.ch == '\0' { ' ' } else { cell.ch };
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    if previous.as_ref().and_then(|s| s.link.as_ref()).is_some() {
        out.extend_from_slice(b"\x1b]8;;\x1b\\");
    }
    out.extend_from_slice(b"\x1b[0m");
    out
}

/// Rgb → xterm 的 `RRRR/GGGG/BBBB` 形式。
///
/// vte 的 Rgb 是 8 位分量；xterm rgb: 格式按惯例把每分量复制成 4 位
/// （ff → ffff），与 macOS `TerminalQueryReply` 的 RRRR/GGGG/BBBB 一致。
fn xterm_rgb(color: Rgb) -> String {
    let dup = |v: u8| format!("{v:02x}{v:02x}");
    format!("{}/{}/{}", dup(color.r), dup(color.g), dup(color.b))
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
    fn vte_execute_c1_bytes_do_not_panic() {
        // 2026-08-15 dogfood：vte 0.13 对 C1/ETX 等打 DEBUG [unhandled]，
        // 但不得 panic，也不得把应答交出去当输入。
        let mut t = TerminalState::new(80, 24);
        t.feed(&[0x80, 0x94, 0x03, 0x82, 0x8e]);
        assert!(t.take_reply().is_empty(), "C1/ETX 不应产生回写应答");
        assert!(t.take_attention_signals().is_empty());
        // 之后正常文本仍可解析。
        t.feed(b"ok");
        assert_eq!(t.snapshot_trimmed(), vec!["ok"]);
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

    /// kitty 键盘协议 `CSI = Ps ; 2 u`（并集）与 `CSI = Ps ; 3 u`（差集）。
    #[test]
    fn kitty_keyboard_mode_union_and_difference() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b[=1;2u"); // union DISAMBIGUATE_ESC_CODES
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::DISAMBIGUATE_ESC_CODES));
        t.feed(b"\x1b[=2;2u"); // union REPORT_EVENT_TYPES
        assert!(t.keyboard_mode.contains(KeyboardModes::REPORT_EVENT_TYPES));
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::DISAMBIGUATE_ESC_CODES));

        t.feed(b"\x1b[=1;3u"); // difference：移除 DISAMBIGUATE_ESC_CODES
        assert!(!t
            .keyboard_mode
            .contains(KeyboardModes::DISAMBIGUATE_ESC_CODES));
        assert!(t.keyboard_mode.contains(KeyboardModes::REPORT_EVENT_TYPES));

        t.feed(b"\x1b[=0u"); // replace：清空
        assert_eq!(t.keyboard_mode, KeyboardModes::default());
    }

    /// kitty 键盘协议全部模式位都能独立设置。
    #[test]
    fn kitty_keyboard_all_flags_settable() {
        let mut t = TerminalState::new(40, 5);
        t.feed(b"\x1b[=15;2u"); // 1|2|4|8 = 15：union 前四档
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::DISAMBIGUATE_ESC_CODES));
        assert!(t.keyboard_mode.contains(KeyboardModes::REPORT_EVENT_TYPES));
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::REPORT_ALTERNATE_KEYS));
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::REPORT_ALL_KEYS_AS_ESC));
        // 16：REPORT_ASSOCIATED_TEXT
        t.feed(b"\x1b[=16;2u");
        assert!(t
            .keyboard_mode
            .contains(KeyboardModes::REPORT_ASSOCIATED_TEXT));
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
        let mut t = TerminalState::with_scrollback(10, 2, 1000);
        for i in 0..1050 {
            t.feed(format!("row{i}\r\n").as_bytes());
        }
        assert!(t.scrollback_lines() <= 1000, "scrollback 应有界");
    }

    /// 自定义上限必须生效：200 行只保留最近 50 行。
    #[test]
    fn scrollback_cap_respects_with_scrollback() {
        let mut t = TerminalState::with_scrollback(10, 2, 50);
        for i in 0..200 {
            t.feed(format!("row{i}\r\n").as_bytes());
        }
        assert_eq!(t.scrollback_lines(), 50);
        assert!(t.scrollback_line(0).unwrap().contains("row")); // 最旧是较新的 150 附近
        assert!(
            t.scrollback_line(49).unwrap().contains("row199") || t.scrollback_line(49).is_some()
        );
    }

    /// 淘汰必须丢最旧行：上限 3，喂 a/b/c/d 后最旧是 b。
    #[test]
    fn scrollback_eviction_drops_oldest() {
        let mut t = TerminalState::with_scrollback(10, 1, 3);
        for line in ["a", "b", "c", "d"] {
            t.feed(format!("{line}\r\n").as_bytes());
        }
        assert_eq!(t.scrollback_lines(), 3);
        assert_eq!(t.scrollback_line(0), Some("b"));
        assert_eq!(t.scrollback_line(2), Some("d"));
    }

    /// search 返回 (seq, 行文本)，大小写敏感。
    #[test]
    fn search_finds_hits_with_seq() {
        let mut t = TerminalState::with_scrollback(10, 1, 50);
        for line in ["alpha", "beta", "alphabet"] {
            t.feed(format!("{line}\r\n").as_bytes());
        }
        assert_eq!(
            t.search("alpha"),
            vec![(1, "alpha".into()), (3, "alphabet".into())]
        );
        assert!(t.search("zzz").is_empty());
    }

    /// E5：search 必须覆盖可见屏（Codex TUI 的 TOKEN_BODY 在可见网格里）。
    #[test]
    fn search_includes_visible_screen_lines() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"alpha TOKEN_BODY one\r\n");
        t.feed(b"beta\r\n");
        let hits = t.search("TOKEN_BODY");
        assert_eq!(hits.len(), 1, "可见屏命中一次");
        assert_eq!(hits[0].0, 1, "可见行 seq 从 next_seq 起");
        assert!(hits[0].1.contains("TOKEN_BODY"));
    }

    /// 空 query 不搜索。
    #[test]
    fn search_empty_query_returns_none() {
        let mut t = TerminalState::with_scrollback(10, 1, 50);
        t.feed(b"alpha\n");
        assert!(t.search("").is_empty());
    }

    /// 淘汰后 seq 不重排（坐标稳定）。
    #[test]
    fn seq_monotonic_across_eviction() {
        let mut t = TerminalState::with_scrollback(10, 1, 3);
        for line in ["a", "b", "c", "d", "e"] {
            t.feed(format!("{line}\r\n").as_bytes());
        }
        assert_eq!(t.scrollback_lines(), 3);
        let seqs: Vec<u64> = (0..3).map(|i| t.scrollback_entry(i).unwrap().seq).collect();
        assert_eq!(seqs, vec![3, 4, 5]);
        assert_eq!(t.latest_seq(), 5);
    }

    /// last_non_empty_line 跳过空白行，从可见屏回退到 scrollback。
    #[test]
    fn last_non_empty_line_skips_blank() {
        let mut t = TerminalState::with_scrollback(10, 3, 50);
        t.feed(b"a\r\n\r\nb\r\n");
        assert_eq!(t.last_non_empty_line(), Some("b".into()));
        t.feed(b"\r\n\r\n\r\n");
        assert_eq!(t.last_non_empty_line(), Some("b".into()));
    }

    /// last_n_lines 先取可见屏尾部，不足再向前取 scrollback。
    #[test]
    fn last_n_lines_includes_visible() {
        let mut t = TerminalState::with_scrollback(10, 3, 50);
        for i in 0..10 {
            t.feed(format!("row{i}\r\n").as_bytes());
        }
        let lines = t.last_n_lines(4);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines.last().map(String::as_str), Some("row9"));
        assert_eq!(lines, vec!["row6", "row7", "row8", "row9"]);
        assert_eq!(t.last_n_lines(2), vec!["row8", "row9"]);
    }

    /// D1：几何 visible_ansi——逐行 CUP + 整行单元格，不 skip 空行、不 trim。
    /// 往返比全网格 snapshot()（含空白行），底行 prompt 必须保留在底行。
    #[test]
    fn visible_ansi_roundtrip_preserves_full_snapshot() {
        let mut old = TerminalState::new(80, 24);
        for i in 0..30 {
            old.feed(format!("row{i}\r\n").as_bytes());
        }
        let ansi = old.visible_ansi();
        assert!(ansi.starts_with(b"\x1b[H\x1b[2J"), "应以前缀复位+清屏开头");
        let mut fresh = TerminalState::new(80, 24);
        fresh.feed(&ansi);
        assert_eq!(
            fresh.snapshot(),
            old.snapshot(),
            "visible_ansi 往返后全网格（含空白行）应一致"
        );
        assert_eq!(fresh.rows(), 24, "网格高度应为 24");
    }

    /// D1：底行 prompt 不被 trim/skip 挤走。
    #[test]
    fn visible_ansi_keeps_prompt_on_last_row() {
        let mut old = TerminalState::new(80, 24);
        old.feed(b"\x1b[24;1HPROMPT");
        assert!(old.snapshot()[23].contains("PROMPT"), "底行应含 PROMPT");
        assert!(!old.snapshot()[0].contains("PROMPT"), "首行不应含 PROMPT");

        let ansi = old.visible_ansi();
        let mut fresh = TerminalState::new(80, 24);
        fresh.feed(&ansi);
        assert_eq!(
            fresh.snapshot(),
            old.snapshot(),
            "几何 dump 往返后全网格应一致"
        );
        assert!(
            fresh.snapshot()[23].contains("PROMPT"),
            "新 state 底行应含 PROMPT: {:?}",
            fresh.snapshot()[23]
        );
        assert!(!fresh.snapshot()[0].contains("PROMPT"), "首行不应含 PROMPT");
    }

    /// Surface seed 只在新 VT 上使用：历史进入原生 scrollback，当前屏和样式保留。
    #[test]
    fn surface_seed_preserves_history_style_and_current_screen() {
        let mut old = TerminalState::with_scrollback(24, 4, 100);
        old.feed(b"\x1b[31mHIST_COLOUR\x1b[0m\r\n");
        for i in 0..8 {
            old.feed(format!("pad-{i}\r\n").as_bytes());
        }
        old.feed(b"TAIL_VISIBLE");

        let seed = old.surface_seed_ansi();
        assert!(seed.contains(&b'H'), "seed 必须包含历史文本");
        assert!(
            seed.windows(b"\x1b[31m".len()).any(|w| w == b"\x1b[31m"),
            "seed 必须带样式 SGR"
        );

        let mut fresh = TerminalState::new(24, 4);
        fresh.feed(&seed);
        assert_eq!(fresh.snapshot(), old.snapshot(), "当前屏不能被 seed 改写");
        assert!(
            fresh
                .scrollback
                .iter()
                .any(|line| line.text.contains("HIST_COLOUR")),
            "历史行必须进入新 VT 的 scrollback"
        );
    }

    /// OSC 133 mark 的 seq 必须能回到真实可见/历史行，而不是命令 ordinal。
    #[test]
    fn command_mark_seq_maps_to_terminal_line() {
        let mut t = TerminalState::with_scrollback(40, 4, 100);
        t.feed(
            b"\x1b]133;A\x07\x1b]133;B\x07echo MARK_ONE\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07",
        );
        let mark = t.command_marks().first().expect("command mark");
        assert!(mark.seq > 0);
        assert!(
            t.line_index_by_seq(mark.seq).is_some(),
            "mark seq 必须能映射到终端行"
        );
    }

    /// E1：Codex 风格 TUI——visible_ansi 必须保留 UTF-8 盒线与真彩背景，
    /// 往返后网格（含 U+2500）和 `48;2;216;216;216` 都在。
    #[test]
    fn visible_ansi_preserves_box_drawing_and_truecolor() {
        let raw = include_str!("../../../../tests/samples/codex-tui-sanitized.txt");
        let payload = raw
            .split_once("PAYLOAD_UTF8_BELOW\n")
            .map(|(_, p)| p)
            .expect("fixture 应含 PAYLOAD_UTF8_BELOW 标记");
        let mut t = TerminalState::new(80, 24);
        t.feed(payload.as_bytes());

        let snap = t.snapshot();
        assert!(snap[0].contains("TOKEN_HEADER"), "首行应含 TOKEN_HEADER");
        assert!(
            snap.iter().any(|l| l.contains("TOKEN_BODY")),
            "应有 TOKEN_BODY 行"
        );
        assert!(
            snap[21].contains("TOKEN_PROMPT") || snap[23].contains("TOKEN_FOOTER"),
            "第 22/24 行应含 TOKEN_PROMPT 或 TOKEN_FOOTER"
        );
        assert!(
            snap.iter().any(|l| l.contains('─')),
            "网格应保留 U+2500 盒线"
        );

        let ansi = t.visible_ansi();
        let dump = String::from_utf8_lossy(&ansi);
        assert!(
            dump.contains("48;2;216;216;216"),
            "真彩背景应编码成 48;2;216;216;216: {dump:?}"
        );
        assert!(dump.contains('─'), "dump 应含 UTF-8 盒线: {dump:?}");

        let mut fresh = TerminalState::new(80, 24);
        fresh.feed(&ansi);
        assert_eq!(fresh.snapshot(), snap, "可见屏 dump 往返后网格应一致");
        assert!(
            fresh.grid.iter().flatten().any(|c| matches!(
                c.bg,
                Some(Color::Spec(Rgb {
                    r: 216,
                    g: 216,
                    b: 216
                }))
            )),
            "往返后真彩背景 Spec(216,216,216) 应仍在"
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

#[cfg(test)]
mod attention_signal_tests {
    use super::*;

    #[test]
    fn osc133_c_emits_command_start() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;C\x07");
        assert_eq!(
            t.take_attention_signals(),
            vec![AttentionSignal::CommandStart]
        );
    }

    #[test]
    fn osc133_d_emits_command_done_with_exit() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;D;0\x07");
        assert_eq!(
            t.take_attention_signals(),
            vec![AttentionSignal::CommandDone { exit_code: Some(0) }]
        );
    }

    #[test]
    fn osc133_records_command_marks_with_exit_and_text() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;A\x07\x1b]133;B\x07cmd_ok\r\n\x1b]133;C\x07out_ok\r\n\x1b]133;D;0\x07");
        t.feed(
            b"\x1b]133;A\x07\x1b]133;B\x07cmd_fail\r\n\x1b]133;C\x07out_fail\r\n\x1b]133;D;1\x07",
        );
        let marks = t.command_marks();
        assert_eq!(
            marks.len(),
            2,
            "OSC 133 两个回合必须记成两条命令刻度，不能只当 Attention 信号丢掉文本"
        );
        assert_eq!(marks[0].command, "cmd_ok");
        assert_eq!(marks[0].exit_code, Some(0));
        assert_eq!(marks[1].command, "cmd_fail");
        assert_eq!(marks[1].exit_code, Some(1));
        assert!(
            marks[0].seq < marks[1].seq,
            "刻度 seq 必须随回合前进。got {} then {}",
            marks[0].seq,
            marks[1].seq
        );
    }

    #[test]
    fn command_marks_support_timeline_navigation_and_scrollback_pruning() {
        let mut t = TerminalState::with_scrollback(40, 2, 2);
        t.feed(b"\x1b]133;B\x07cmd_one\r\n\x1b]133;C\x07\x1b]133;D;0\x07");
        t.feed(b"\x1b]133;B\x07cmd_two\r\n\x1b]133;C\x07\x1b]133;D;1\x07");

        let marks = t.command_marks();
        assert_eq!(marks.len(), 2);
        let first = marks[0].clone();
        let second = marks[1].clone();
        assert_eq!(t.previous_command_mark(second.seq), Some(&first));
        assert_eq!(t.next_command_mark(first.seq), Some(&second));
        assert_eq!(t.previous_command_mark(0), Some(&second));
        assert_eq!(t.next_command_mark(0), Some(&first));
        assert_eq!(t.last_successful_command(), Some(&first));
        assert_eq!(t.last_failed_command(), Some(&second));

        // 足够多的整屏换行会让第一条刻度所在行离开 bounded scrollback；
        // stale command mark 必须同步淘汰，不能继续返回 offset=0 的假跳转。
        for i in 0..12 {
            t.feed(format!("evict-{i}\r\n").as_bytes());
        }
        assert!(
            t.command_marks().iter().all(|m| m.seq != first.seq),
            "被 scrollback 淘汰的 command mark 必须同步移除"
        );
        assert_eq!(t.line_index_by_seq(first.seq), None);
        assert_eq!(t.previous_command_mark(0), None);
        assert_eq!(t.next_command_mark(0), None);
    }

    #[test]
    fn osc133_d_without_exit_emits_none() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;D\x1b\\");
        assert_eq!(
            t.take_attention_signals(),
            vec![AttentionSignal::CommandDone { exit_code: None }]
        );
    }

    #[test]
    fn osc133_a_and_p_treated_as_done() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;A;aid=1\x07");
        t.feed(b"\x1b]133;P\x07");
        assert_eq!(
            t.take_attention_signals(),
            vec![
                AttentionSignal::CommandDone { exit_code: None },
                AttentionSignal::CommandDone { exit_code: None },
            ]
        );
    }

    #[test]
    fn osc133_b_ignored() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;B\x07");
        assert!(t.take_attention_signals().is_empty());
    }

    #[test]
    fn bel_emits_attention_request() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x07");
        assert_eq!(
            t.take_attention_signals(),
            vec![AttentionSignal::AttentionRequest {
                source: AttentionSource::Bel
            }]
        );
    }

    #[test]
    fn osc9_and_777_emit_osc_notify() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]9;hi\x07");
        t.feed(b"\x1b]777;notify;x;y\x07");
        assert_eq!(
            t.take_attention_signals(),
            vec![
                AttentionSignal::AttentionRequest {
                    source: AttentionSource::OscNotify
                },
                AttentionSignal::AttentionRequest {
                    source: AttentionSource::OscNotify
                },
            ]
        );
    }

    #[test]
    fn unknown_osc_keeps_printing() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133;X\x07hello");
        assert!(t.take_attention_signals().is_empty());
        assert_eq!(t.snapshot_trimmed(), vec!["hello"]);
    }

    #[test]
    fn truncated_osc_resumes_across_feeds() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]133");
        assert!(t.take_attention_signals().is_empty());
        t.feed(b";C\x07");
        assert_eq!(
            t.take_attention_signals(),
            vec![AttentionSignal::CommandStart]
        );
    }

    #[test]
    fn title_osc_still_sets_title() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]0;my-title\x07");
        assert!(t.take_attention_signals().is_empty());
        assert_eq!(t.title.as_deref(), Some("my-title"));
    }

    #[test]
    fn fixture_osc_attention_passthrough_decodes_to_signals() {
        // E1 fixture 的 %output 行经 ControlEscapeDecoder 还原后 feed，
        // 应产出 OSC 133 C / D 与 BEL/9/777 信号（PASS_THROUGH 三态）。
        let decoder = crate::core::runtime::tmux::protocol::ControlEscapeDecoder::new();
        let raw = include_str!("../../../../tests/samples/osc-attention-tmux3.7b.txt");
        let mut t = TerminalState::new(80, 24);
        let mut saw = Vec::new();
        for line in raw.lines() {
            if !line.starts_with("%output ") {
                continue;
            }
            // 3.7b 控制模式直接写 `%output %0 <content>`（无引号）；
            // 取 pane id 之后的内容并解码 C 转义。
            let rest = &line["%output ".len()..];
            let Some(space) = rest.find(' ') else {
                continue;
            };
            let content = &rest[space + 1..];
            let decoded = decoder.decode(content).unwrap_or_default();
            t.feed(&decoded);
            saw.extend(t.take_attention_signals());
        }
        assert!(
            saw.contains(&AttentionSignal::CommandStart),
            "fixture 应含 CommandStart: {saw:?}"
        );
        assert!(
            saw.iter()
                .any(|s| matches!(s, AttentionSignal::CommandDone { exit_code: Some(0) })),
            "fixture 应含 CommandDone(0): {saw:?}"
        );
        assert!(
            saw.iter().any(|s| matches!(
                s,
                AttentionSignal::AttentionRequest {
                    source: AttentionSource::Bel
                }
            )),
            "fixture 应含 Bel: {saw:?}"
        );
        assert!(
            saw.iter().any(|s| matches!(
                s,
                AttentionSignal::AttentionRequest {
                    source: AttentionSource::OscNotify
                }
            )),
            "fixture 应含 OscNotify: {saw:?}"
        );
    }
}

/// xterm ctlseqs / Alacritty / VTE 行为一致性测试。
///
/// 序列语义参考 xterm 控制序列文档
/// （https://invisible-island.net/xterm/ctlseqs/ctlseqs.html）以及
/// Alacritty 的终端模拟器行为（https://github.com/alacritty/alacritty）。
/// 目的是把「知名终端项目都会正确处理」的序列固化成本项目的状态模型测试。
#[cfg(test)]
mod xterm_conformance_tests {
    use super::*;
    use vte::ansi::NamedColor;

    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    /// CSI A/B/C/D：相对光标移动，并在屏幕边缘收敛（不越界）。
    #[test]
    fn cursor_relative_moves_clamp_at_edges() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"\x1b[3;6H"); // (2,5)
        t.feed(b"\x1b[2A"); // up 2 -> (0,5)
        assert_eq!((t.cursor_row(), t.cursor_col()), (0, 5));
        t.feed(b"\x1b[5A"); // 已在顶行，不动
        assert_eq!((t.cursor_row(), t.cursor_col()), (0, 5));
        t.feed(b"\x1b[2B"); // down 2 -> (2,5)
        assert_eq!((t.cursor_row(), t.cursor_col()), (2, 5));
        t.feed(b"\x1b[5B"); // 已在底行，不动
        assert_eq!((t.cursor_row(), t.cursor_col()), (2, 5));
        t.feed(b"\x1b[2C"); // right 2 -> col 7
        assert_eq!((t.cursor_row(), t.cursor_col()), (2, 7));
        t.feed(b"\x1b[5C"); // 到最右列
        assert_eq!(t.cursor_col(), 9);
        t.feed(b"\x1b[2D"); // left 2
        assert_eq!(t.cursor_col(), 7);
        t.feed(b"\x1b[9D"); // 回到最左列
        assert_eq!(t.cursor_col(), 0);
    }

    /// CSI H/f、CSI d、CSI G、CSI `：绝对定位。
    #[test]
    fn cursor_absolute_positioning() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"\x1b[2;3H"); // CUP (1,2)
        assert_eq!((t.cursor_row(), t.cursor_col()), (1, 2));
        t.feed(b"\x1b[4d"); // VPA 第 4 行
        assert_eq!(t.cursor_row(), 3);
        t.feed(b"\x1b[7G"); // CHA 第 7 列
        assert_eq!(t.cursor_col(), 6);
        t.feed(b"\x1b[2;1H");
        assert_eq!((t.cursor_row(), t.cursor_col()), (1, 0));
        t.feed(b"\x1b[5`"); // HPA 第 5 列
        assert_eq!(t.cursor_col(), 4);
        t.feed(b"\x1b[3;5f"); // HVP = CUP
        assert_eq!((t.cursor_row(), t.cursor_col()), (2, 4));
    }

    /// CSI E/F：下一行/上一行并回到行首。
    #[test]
    fn cursor_line_ops_with_cr() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"\x1b[2;4H");
        t.feed(b"\x1b[1E"); // 下一行 + CR
        assert_eq!((t.cursor_row(), t.cursor_col()), (2, 0));
        t.feed(b"\x1b[1F"); // 上一行 + CR
        assert_eq!((t.cursor_row(), t.cursor_col()), (1, 0));
    }

    /// CSI K 的三种模式：清行尾 / 清行首 / 清整行。
    #[test]
    fn erase_line_modes() {
        let mut right = TerminalState::new(10, 3);
        right.feed(b"abcdef");
        right.feed(b"\x1b[1;4H");
        right.feed(b"\x1b[K"); // 0：清行尾（从光标列起）
        assert_eq!(snap(&right), vec!["abc"]);

        let mut left = TerminalState::new(10, 3);
        left.feed(b"abcdef");
        left.feed(b"\x1b[1;4H");
        left.feed(b"\x1b[1K"); // 1：清行首（含光标格）
        assert_eq!(snap(&left), vec!["    ef"]);

        let mut all = TerminalState::new(10, 3);
        all.feed(b"abcdef");
        all.feed(b"\x1b[1;4H");
        all.feed(b"\x1b[2K"); // 2：清整行
        assert_eq!(snap(&all), Vec::<String>::new());
    }

    /// CSI J 的四种模式：清屏下方 / 上方 / 全部 / saved 区。
    #[test]
    fn erase_display_modes() {
        let mut below = TerminalState::new(10, 3);
        below.feed(b"abcdef\r\nghijkl\r\nmnopqr");
        below.feed(b"\x1b[2;4H");
        below.feed(b"\x1b[0J"); // 清光标以下
        assert_eq!(snap(&below), vec!["abcdef", "ghi"]);

        let mut above = TerminalState::new(10, 3);
        above.feed(b"abcdef\r\nghijkl\r\nmnopqr");
        above.feed(b"\x1b[2;4H");
        above.feed(b"\x1b[1J"); // 清光标以上（含光标行左侧）
        assert_eq!(snap(&above), vec!["", "    kl", "mnopqr"]);

        let mut all = TerminalState::new(10, 3);
        all.feed(b"abcdef\r\nghijkl\r\nmnopqr");
        all.feed(b"\x1b[2J");
        assert_eq!(snap(&all), Vec::<String>::new());

        let mut saved = TerminalState::new(10, 3);
        saved.feed(b"abcdef\r\nghijkl\r\nmnopqr");
        saved.feed(b"\x1b[3J"); // ED 3：清 saved 区
        assert_eq!(snap(&saved), Vec::<String>::new());
    }

    /// CSI X：擦除光标起 n 个字符（不移动光标）。
    #[test]
    fn erase_chars_keeps_cursor() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"abcdef");
        t.feed(b"\x1b[1;3H");
        t.feed(b"\x1b[2X");
        assert_eq!(snap(&t), vec!["ab  ef"]);
        assert_eq!(t.cursor_col(), 2);
    }

    /// CSI S/T：整屏上滚/下滚（Alacritty/VTE 行为）。
    #[test]
    fn scroll_up_down_moves_screen() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        t.feed(b"\x1b[2S"); // SU 2：顶部 2 行滚出，底部补空
        assert_eq!(snap(&t), vec!["3", "4", "5"]);
        t.feed(b"\x1b[2T"); // SD 2：顶部补空
        assert_eq!(snap(&t), vec!["", "", "3", "4", "5"]);
    }

    /// SGR 39/49：前景/背景恢复默认（None）。
    #[test]
    fn sgr_reset_to_defaults() {
        let mut fg = TerminalState::new(20, 3);
        fg.feed(b"\x1b[38;2;1;2;3mX\x1b[39mY");
        assert_eq!(
            fg.cell(0, 0).unwrap().fg,
            Some(Color::Spec(Rgb { r: 1, g: 2, b: 3 }))
        );
        assert_eq!(fg.cell(0, 1).unwrap().fg, None);

        let mut bg = TerminalState::new(20, 3);
        bg.feed(b"\x1b[48;2;1;2;3mX\x1b[49mY");
        assert_eq!(
            bg.cell(0, 0).unwrap().bg,
            Some(Color::Spec(Rgb { r: 1, g: 2, b: 3 }))
        );
        assert_eq!(bg.cell(0, 1).unwrap().bg, None);
    }

    /// SGR 90-97 / 100-107：亮色前景/背景。
    #[test]
    fn sgr_bright_colors() {
        let mut t = TerminalState::new(20, 3);
        t.feed(b"\x1b[91mA\x1b[107mB");
        assert_eq!(
            t.cell(0, 0).unwrap().fg,
            Some(Color::Named(NamedColor::BrightRed))
        );
        assert_eq!(
            t.cell(0, 1).unwrap().bg,
            Some(Color::Named(NamedColor::BrightWhite))
        );
    }

    /// SGR 24/27/29：取消下划线/反显/删除线。
    #[test]
    fn sgr_style_cancels() {
        let mut t = TerminalState::new(20, 3);
        t.feed(b"\x1b[4mU\x1b[24mX");
        assert!(t.cell(0, 0).unwrap().underline);
        assert!(!t.cell(0, 1).unwrap().underline);
        t.feed(b"\x1b[7mR\x1b[27mY");
        assert!(t.cell(0, 2).unwrap().reverse);
        assert!(!t.cell(0, 3).unwrap().reverse);
        t.feed(b"\x1b[9mS\x1b[29mZ");
        assert!(t.cell(0, 4).unwrap().strike);
        assert!(!t.cell(0, 5).unwrap().strike);
    }

    /// DECAWM（CSI ? 7 l）：关闭自动换行后，最后一个字符格被覆盖而不是换行。
    #[test]
    fn autowrap_disabled_overwrites_last_col() {
        let mut t = TerminalState::new(4, 3);
        t.feed(b"abc");
        // 此时光标在最后一列；关闭自动换行后打印会覆盖而不是换行
        t.feed(b"\x1b[?7l");
        t.feed(b"d");
        assert_eq!(snap(&t), vec!["abcd"]);
        assert_eq!(t.cursor_col(), 3);
        t.feed(b"E"); // 覆盖最后一格
        assert_eq!(snap(&t), vec!["abcE"]);
    }

    /// DEC Special Character and Line Drawing Set（ESC ( 0）完整框线映射。
    #[test]
    fn dec_line_drawing_full_set() {
        let mut t = TerminalState::new(40, 3);
        t.feed(b"\x1b(0qxjklmntuvw");
        assert_eq!(t.snapshot_trimmed(), vec!["─│┘┐┌└┼├┤┴┬"]);
        t.feed(b"\x1b(B");
        t.feed(b" plain");
        assert_eq!(t.snapshot_trimmed(), vec!["─│┘┐┌└┼├┤┴┬ plain"]);
    }

    /// 宽字符行滚入 scrollback 后仍保留（含内部占位空格）。
    #[test]
    fn scrollback_wide_line_preserved() {
        let mut t = TerminalState::new(6, 2);
        t.feed("中文\r\nok".as_bytes());
        t.feed(b"\r\nnext");
        assert_eq!(t.scrollback_line(0), Some("中 文"));
        assert_eq!(snap(&t), vec!["ok", "next"]);
    }
}

/// 终端查询应答回归测试。
///
/// 参考 xterm 控制序列文档
/// （https://invisible-island.net/xterm/ctlseqs/ctlseqs.html）与 wezterm
/// `set_or_query!`（term/src/terminalstate/performer.rs）：
/// - OSC 10/11/12 颜色查询回复 `ESC ] <code> ; rgb:RRRR/GGGG/BBBB ESC \`
/// - CSI DA（设备属性）查询回复 `ESC [ ? 65 ; ... c`
///
/// 这些字节必须经 `take_reply()` 原样交回 shell/pty，否则 `git lg` 里
/// `10;rgb:...` / `11;rgb:...` / `65;...c` 会泄漏成普通文本命令。
#[cfg(test)]
mod query_reply_tests {
    use super::*;

    fn reply(t: &mut TerminalState, data: &[u8]) -> Vec<u8> {
        t.feed(data);
        t.take_reply()
    }

    #[test]
    fn osc10_foreground_query_returns_xterm_rgb() {
        let mut t = TerminalState::new(80, 24);
        let r = reply(&mut t, b"\x1b]10;?\x1b\\");
        assert_eq!(r, b"\x1b]10;rgb:0000/0000/0000\x1b\\");
    }

    #[test]
    fn osc11_background_query_returns_xterm_rgb() {
        let mut t = TerminalState::new(80, 24);
        let r = reply(&mut t, b"\x1b]11;?\x07");
        assert_eq!(r, b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    }

    #[test]
    fn osc12_cursor_query_returns_xterm_rgb() {
        let mut t = TerminalState::new(80, 24);
        let r = reply(&mut t, b"\x1b]12;?\x1b\\");
        assert_eq!(r, b"\x1b]12;rgb:0000/0000/0000\x1b\\");
    }

    #[test]
    fn osc4_palette_query_returns_xterm_rgb() {
        let mut t = TerminalState::new(80, 24);
        let r = reply(&mut t, b"\x1b]4;1;?\x1b\\");
        assert_eq!(r, b"\x1b]4;1;rgb:cdcd/0000/0000\x1b\\");
    }

    #[test]
    fn primary_da_query_returns_vt525_identity() {
        let mut t = TerminalState::new(80, 24);
        // CSI c / CSI ? c / ESC Z 都是 Primary DA 查询
        let r1 = reply(&mut t, b"\x1b[c");
        assert_eq!(r1, b"\x1b[?65;4;1;2;6;21;22;17;28c");
        let r2 = reply(&mut t, b"\x1b[?c");
        assert_eq!(r2, b"\x1b[?65;4;1;2;6;21;22;17;28c");
        let r3 = reply(&mut t, b"\x1bZ");
        assert_eq!(r3, b"\x1b[?65;4;1;2;6;21;22;17;28c");
    }

    #[test]
    fn secondary_da_query_returns_secondary_identity() {
        let mut t = TerminalState::new(80, 24);
        let r = reply(&mut t, b"\x1b[>c");
        assert_eq!(r, b"\x1b[>65;20;1c");
    }

    #[test]
    fn dsr_status_and_cursor_position_reply() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b[3;5H");
        let r1 = reply(&mut t, b"\x1b[5n");
        assert_eq!(r1, b"\x1b[0n");
        let r2 = reply(&mut t, b"\x1b[6n");
        assert_eq!(r2, b"\x1b[3;5R");
    }

    #[test]
    fn query_reply_does_not_enter_screen_grid() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]10;?\x1b\\\x1b[?c");
        let reply_bytes = t.take_reply();
        assert!(!reply_bytes.is_empty());
        assert_eq!(t.snapshot_trimmed(), Vec::<String>::new());
    }

    #[test]
    fn query_split_across_feed_calls_still_replies() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]10");
        assert!(t.take_reply().is_empty());
        t.feed(b";?\x1b\\");
        let r = t.take_reply();
        assert_eq!(r, b"\x1b]10;rgb:0000/0000/0000\x1b\\");
    }
}

/// 运行时 resize 回归测试。
///
/// 之前 TUI 在 resize 时直接重建 `TerminalState` 并从头重放被截断的
/// 累计输出，导致 ANSI 流从中间开始解析、屏幕内容错乱。真实 resize
/// 必须保留屏幕/光标/滚动区域，只调整行列。
#[cfg(test)]
mod resize_tests {
    use super::*;

    fn snap(t: &TerminalState) -> Vec<String> {
        t.snapshot_trimmed()
    }

    #[test]
    fn resize_grows_keeps_content_and_cursor() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"abc");
        t.feed(b"\x1b[2;2H");
        t.resize(20, 5);
        assert_eq!(t.cols(), 20);
        assert_eq!(t.rows(), 5);
        assert_eq!(snap(&t), vec!["abc"]);
        assert_eq!((t.cursor_row(), t.cursor_col()), (1, 1));
        // resize 后继续输出仍落在原光标位置
        t.feed(b"X");
        assert_eq!(t.cell(1, 1).unwrap().ch, 'X');
    }

    #[test]
    fn resize_shrinks_keeps_bottom_when_cursor_at_bottom() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        t.resize(10, 3);
        assert_eq!(t.rows(), 3);
        assert_eq!(snap(&t), vec!["3", "4", "5"]);
        assert_eq!(t.cursor_row(), 2);
    }

    #[test]
    fn resize_shrinks_truncates_when_cursor_not_at_bottom() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        t.feed(b"\x1b[2;1H");
        t.resize(10, 3);
        assert_eq!(snap(&t), vec!["1", "2", "3"]);
        assert_eq!(t.cursor_row(), 1);
    }

    #[test]
    fn resize_clamps_cursor_and_scroll_region() {
        let mut t = TerminalState::new(10, 5);
        t.feed(b"\x1b[2;4r");
        t.feed(b"\x1b[5;10H");
        t.resize(6, 3);
        assert_eq!(t.cursor_row(), 2);
        assert_eq!(t.cursor_col(), 5);
        assert_eq!(t.scroll_top, 1);
        assert_eq!(t.scroll_bottom, 2);
    }

    #[test]
    fn resize_same_size_is_noop() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"abc");
        let before = t.snapshot();
        t.resize(10, 3);
        assert_eq!(t.snapshot(), before);
        assert_eq!(t.scroll_top, 0);
        assert_eq!(t.scroll_bottom, 2);
    }

    #[test]
    fn resize_expands_extends_scroll_region_to_new_height() {
        let mut t = TerminalState::new(10, 3);
        t.feed(b"x");
        t.resize(10, 6);
        assert_eq!(t.scroll_bottom, 5);
        assert_eq!(snap(&t), vec!["x"]);
        assert_eq!(
            t.soft_wrap_row_count(),
            t.rows(),
            "resize 长高后 grid_soft_wrapped 必须与 grid 同行数"
        );
    }

    /// test-2026-0818-1114.log：insertion index 26 <= len 15。
    /// PaneBuf 每次 %output 都 resize 到 tmux 当前行列；窗口变高后
    /// `grid` 变 27 行而 `grid_soft_wrapped` 仍 15，agent DECSTBM+LF panic，
    /// `muxterm_poll_events` catch_unwind 丢掉整批事件，SwiftTerm 只收到半截。
    #[test]
    fn resize_grow_then_partial_decstbm_lf_does_not_panic() {
        let mut t = TerminalState::new(80, 15);
        t.resize(80, 27);
        assert_eq!(
            t.soft_wrap_row_count(),
            t.rows(),
            "15→27 后 soft-wrap 行数必须是 27，不能停在 15"
        );
        // 1 基：top=2 bottom=27 → 0 基 1..26，不是整屏，走 linefeed 部分滚动分支。
        t.feed(b"\x1b[2;27r");
        t.feed(b"\x1b[27;1H");
        t.feed(b"HEAD\nBODY\nTAIL\n");
        t.feed(b"PROMPT");
        let snap = t.snapshot().join("\n");
        assert!(
            snap.contains("PROMPT"),
            "resize 后 DECSTBM+LF 必须还能写完画面。snap={snap:?}"
        );
        assert_eq!(t.soft_wrap_row_count(), t.rows());
    }

    /// 同一日志后半段：removal index 11 < len 11（soft-wrap 比 grid 短）。
    #[test]
    fn resize_shrink_then_decstbm_lf_does_not_panic() {
        let mut t = TerminalState::new(80, 27);
        t.feed(b"\x1b[2;27r");
        t.resize(80, 11);
        assert_eq!(t.soft_wrap_row_count(), t.rows());
        t.feed(b"\x1b[1;11r");
        t.feed(b"\x1b[11;1H\n\nFOOT");
        let snap = t.snapshot().join("\n");
        assert!(
            snap.contains("FOOT"),
            "缩小后滚动区域 LF 不得 panic。snap={snap:?}"
        );
        assert_eq!(t.soft_wrap_row_count(), t.rows());
    }

    /// CSI L / M 也必须同步 soft-wrap 行，否则下一次 LF 仍会越界。
    #[test]
    fn insert_and_delete_lines_keep_soft_wrap_len() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b[5L");
        assert_eq!(t.soft_wrap_row_count(), t.rows());
        t.feed(b"\x1b[3M");
        assert_eq!(t.soft_wrap_row_count(), t.rows());
        t.feed(b"\x1b[10;20r\x1b[20;1H\nOK");
        assert!(t.snapshot().join("\n").contains("OK"));
        assert_eq!(t.soft_wrap_row_count(), t.rows());
    }
}

/// 回归：cursor agent 输入区/状态区的「擦除 + 上移 + 重绘」必须原地覆盖，
/// 不能每帧向下/向上堆叠（用户看到 y/yo/you… 一帧一行、Working/Running
/// 一帧一行的根因）。
#[cfg(test)]
mod inplace_redraw_tests {
    use super::*;

    fn grid_rows(t: &TerminalState) -> Vec<String> {
        t.snapshot()
            .into_iter()
            .map(|r| r.trim_end_matches([' ', '\0']).to_string())
            .collect()
    }

    fn count_lines(rows: &[String], needle: &str) -> usize {
        rows.iter().filter(|r| r.contains(needle)).count()
    }

    /// 两帧连续重绘：第二帧必须完全覆盖第一帧（同一批行内更新）。
    #[test]
    fn consecutive_redraw_overwrites_in_place() {
        let mut t = TerminalState::new(80, 24);
        // 与真实 cursor agent 一致：每帧向上擦除 9 行（覆盖首帧 6 行内容后
        // clamp 到第 0 行），再原地重绘 6 行。
        let erase9 = "\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[2K\x1b[1A\x1b[G";
        let frame_a = format!("{erase9}STATUS-A\r\nTIP-A\r\n\r\nBOX-A\r\n\r\nFOOTER-A\r\n");
        let frame_b = format!("{erase9}STATUS-B\r\nTIP-B\r\n\r\nBOX-B\r\n\r\nFOOTER-B\r\n");
        t.feed(b"\x1b[H\x1b[2J");
        t.feed(frame_a.as_bytes());
        let after_a = grid_rows(&t);
        assert_eq!(count_lines(&after_a, "STATUS-A"), 1, "第一帧应恰好一行");

        t.feed(frame_b.as_bytes());
        let after_b = grid_rows(&t);
        assert_eq!(count_lines(&after_b, "STATUS-B"), 1, "第二帧应恰好一行");
        assert_eq!(
            count_lines(&after_b, "STATUS-A"),
            0,
            "第二帧必须原地覆盖第一帧"
        );
        assert_eq!(count_lines(&after_b, "FOOTER-A"), 0, "旧 footer 也不得残留");
        // 每帧后光标行应稳定，下一次重绘仍覆盖同一区域
        let after_cursor = (t.cursor_row(), t.cursor_col());
        t.feed(format!("{erase9}STATUS-C\r\nTIP-C\r\n\r\nBOX-C\r\n\r\nFOOTER-C\r\n").as_bytes());
        let after_c = grid_rows(&t);
        assert_eq!(count_lines(&after_c, "STATUS-C"), 1);
        assert_eq!(count_lines(&after_c, "STATUS-B"), 0, "第三帧仍原地覆盖");
        assert_eq!(
            t.cursor_row(),
            after_cursor.0,
            "每帧结束后光标行应稳定，不能逐帧漂移"
        );
    }

    /// htop 类全屏程序：DECSTBM 部分滚动区域 + 区域底行 LF，只能滚动区域
    /// 内部，表头/表尾和区域外的行必须原样保留。
    #[test]
    fn partial_scroll_region_linefeed_scrolls_only_region() {
        let mut t = TerminalState::new(20, 10);
        for r in 0..10 {
            t.feed(format!("\x1b[{};1Hrow{r}", r + 1).as_bytes());
        }
        // 区域 = 1 基 4..7（0 基 3..6）；光标移到区域底行后 LF 触发区域内滚动
        t.feed(b"\x1b[4;7r\x1b[7;1H\n");

        let rows = t.snapshot();
        let line = |i: usize| rows[i].trim_end_matches([' ', '\0']).to_string();
        assert_eq!(line(0), "row0", "区域上方不得滚动");
        assert_eq!(line(1), "row1");
        assert_eq!(line(2), "row2");
        assert_eq!(line(3), "row4", "区域顶行 row3 应滚出");
        assert_eq!(line(4), "row5");
        assert_eq!(line(5), "row6");
        assert_eq!(line(6), "", "区域底行应补空");
        assert_eq!(line(7), "row7", "区域下方不得滚动");
        assert_eq!(line(8), "row8");
        assert_eq!(line(9), "row9");
    }

    /// htop 用 CUP/CHA 把 CPU 条画到很靠右的列（真实样例 `ESC[102G`）。
    /// 网格必须至少那么宽，否则坐标被 clamp，表头数字叠在一起。
    #[test]
    fn htop_cup_column_102_needs_matching_width() {
        let mut wide = TerminalState::new(120, 8);
        wide.feed(b"\x1b[2;1HLEFT\x1b[102GRIGHT");
        assert_eq!(
            wide.cell(1, 101).map(|c| c.ch),
            Some('R'),
            "120 列时 ESC[102G 应写在第 102 列"
        );
        assert!(wide.line(1).contains("LEFT"));

        let mut narrow = TerminalState::new(80, 8);
        narrow.feed(b"\x1b[2;1HLEFT\x1b[102GRIGHT");
        assert!(
            narrow.cell(1, 101).is_none(),
            "80 列没有第 102 列，htop 帧会错位"
        );
    }

    /// 2219.log tab2 Codex：每个 IME 词只推 ~230 字节 CUP+EL 增量，不是整屏。
    /// 网格宽度与 pane 一致时，整句必须留在同一行（「只能看见最后一个词」的反例）。
    #[test]
    fn cup_el_cjk_input_keeps_full_line_when_width_matches() {
        let mut t = TerminalState::new(80, 16);
        let frame = |text: &str| {
            format!("\x1b[11;1H\x1b[0m\x1b[48;2;216;216;216m › {text}\x1b[K\x1b[49m").into_bytes()
        };
        t.feed(&frame("把这个"));
        t.feed(&frame("把这个分支修改"));
        t.feed(&frame("把这个分支修改，让yaklib可以在xx"));
        let line = grid_rows(&t)
            .into_iter()
            .find(|r| r.contains("yaklib"))
            .expect("yaklib 应在输入行");
        // 宽字符占两列，第二格可能是空格；去掉空白后再比整句。
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("把这个分支修改"),
            "增量 CUP+EL 不得只留下最后一个词: {line:?}"
        );
    }
}
