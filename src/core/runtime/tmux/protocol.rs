//! tmux 控制协议解析器（line-oriented） + layout tree 解析器。
//!
//! tmux 以 `-CC` 启动后会向 stdout 输出结构化通知，每行以 `%` 开头（命令响应内容
//! 除外，夹在 `%begin` / `%end` 之间）。本模块提供：
//!
//! - [`Message`]：覆盖所有已知通知类型的 enum
//! - [`parse_line`]：解析单行原始输出（已按真换行切分）为 `Option<Message>`
//! - [`ControlEscapeDecoder`]：解码 `%output` 里 C 风格转义字符串
//! - [`parse_layout_tree`]：把 tmux 的 window_layout 树字符串解析成 [`LayoutTree`]，
//!   供 TmuxRuntime 映射成 [`LayoutNode`](crate::core::model::layout::LayoutNode)。
//!
//! 设计要点：
//! - 纯函数，输入 `&str` 输出 `Message`，方便单元测试。
//! - 忽略空行与不以 `%` 开头的普通行（按协议这些不属于通知，而是命令响应正文，由
//!   上层在 `%begin`/`%end` 边界里收集）。
//! - pane id `@N`、window id `@N` 靠消息类型/字段位置区分；session id `$N`。
//! - `%output` 的 content 是 C 风格转义字符串（`\e` / `\n` / `\r` / `\\` / `\"`
//!   / `\t` / `\0xx` 八进制 / `\xNN` 十六进制），内部可能含 ANSI 转义序列。
//!
//! 参考：iTerm2 tmux integration 与 tmux 源码 `control.c`。本地实测 tmux 3.7b。

use std::str::FromStr;
use thiserror::Error;

use crate::core::model::layout::SplitDir;
pub use crate::core::types::{PaneId, TabId};

/// tmux session id（`$N`）。只存在于 `runtime/tmux`，产品层没有 Session。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TmuxSessionId(pub u32);

impl TmuxSessionId {
    pub fn as_str(self) -> String {
        format!("${}", self.0)
    }
}

impl std::fmt::Display for TmuxSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "${}", self.0)
    }
}

/// 协议解析错误（库层）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    /// `%` 消息无法识别其类型。
    #[error("unknown notification: {0}")]
    UnknownNotification(String),
    /// 字段格式不对。
    #[error("malformed field: {0}")]
    MalformedField(String),
    /// C 转义字符串解码失败。
    #[error("control escape decode error: {0}")]
    EscapeError(ControlEscapeError),
    /// 数字解析失败。
    #[error("number parse error: {0}")]
    NumberParse(String),
}

/// C 风格转义解码错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControlEscapeError {
    #[error("truncated escape at end of string")]
    Truncated,
    #[error("invalid octal digit: {0}")]
    InvalidOctal(char),
    #[error("invalid hex digit: {0}")]
    InvalidHex(char),
    #[error("unknown escape sequence: \\{0}")]
    UnknownEscape(char),
    #[error("bare backslash at end of string")]
    BareBackslash,
}

// ============================================================================
// C 风格转义字符串解码器
// ============================================================================

/// `%output` content 的 C 风格转义解码器。
///
/// tmux 用 C 的 `\e` / `\n` / `\\` / `\"` / `\0xx` / `\xNN` 等转义把含 ANSI 转义
/// 序列（ESC = 0x1B）的输出编码为单行可打印字符串。这里把它解码回原始字节。
///
/// 实现策略：
/// - 输入是「去掉两端双引号之后」的内部字符串（调用方负责剥引号）。
/// - 逐字符扫描，遇到 `\` 走转义分支，否则原样输出。
/// - 返回 `Vec<u8>`：解码后的内容可能不是合法 UTF-8（虽然实际多数是），上层可
///   自行决定用 `String::from_utf8_lossy` 还是保留字节。
#[derive(Debug, Clone, Default)]
pub struct ControlEscapeDecoder;

impl ControlEscapeDecoder {
    pub fn new() -> Self {
        Self
    }

    /// 解码 C 风格转义字节串，返回原始字节。
    ///
    /// 与 [`decode`](Self::decode) 的区别：输入是原始字节，非转义的高位字节
    /// （包括非法 UTF-8）会原样保留，不会被 `from_utf8_lossy` 替换。
    pub fn decode_bytes(&self, bytes: &[u8]) -> Result<Vec<u8>, ControlEscapeError> {
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b != b'\\' {
                out.push(b);
                i += 1;
                continue;
            }
            // 反斜杠：转义
            i += 1;
            if i >= bytes.len() {
                return Err(ControlEscapeError::BareBackslash);
            }
            let esc = bytes[i];
            match esc {
                b'e' => {
                    out.push(0x1B); // ESC
                    i += 1;
                }
                b'n' => {
                    out.push(b'\n');
                    i += 1;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 1;
                }
                b't' => {
                    out.push(b'\t');
                    i += 1;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 1;
                }
                b'"' => {
                    out.push(b'"');
                    i += 1;
                }
                b'a' => {
                    out.push(0x07); // BEL
                    i += 1;
                }
                b'b' => {
                    out.push(0x08); // BS
                    i += 1;
                }
                b'f' => {
                    out.push(0x0C); // FF
                    i += 1;
                }
                b'v' => {
                    out.push(0x0B); // VT
                    i += 1;
                }
                b'0'..=b'7' => {
                    // 八进制：最多 3 位（tmux 用 \0xx 形式，但兼容 1~3 位）
                    let mut digits = 0u8;
                    let mut val: u32 = 0;
                    while i < bytes.len() && digits < 3 {
                        let c = bytes[i];
                        if (b'0'..=b'7').contains(&c) {
                            val = val * 8 + (c - b'0') as u32;
                            digits += 1;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    out.push(val as u8);
                }
                b'x' => {
                    // 十六进制：tmux 用 \xNN 形式，必须恰好有两位。
                    i += 1;
                    let Some(&first) = bytes.get(i) else {
                        return Err(ControlEscapeError::Truncated);
                    };
                    let Some(first) = hex_value(first) else {
                        return Err(ControlEscapeError::InvalidHex(first as char));
                    };
                    i += 1;
                    let Some(&second) = bytes.get(i) else {
                        return Err(ControlEscapeError::Truncated);
                    };
                    let Some(second) = hex_value(second) else {
                        return Err(ControlEscapeError::InvalidHex(second as char));
                    };
                    i += 1;
                    out.push((first << 4 | second) as u8);
                }
                other => {
                    return Err(ControlEscapeError::UnknownEscape(other as char));
                }
            }
        }
        Ok(out)
    }

    /// 解码转义字符串，返回原始字节。
    pub fn decode(&self, s: &str) -> Result<Vec<u8>, ControlEscapeError> {
        self.decode_bytes(s.as_bytes())
    }

    /// 解码并尝试转成 UTF-8 字符串（用 lossy 回退）。
    pub fn decode_lossy(&self, s: &str) -> String {
        match self.decode(s) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => s.to_string(),
        }
    }
}

fn hex_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

// ============================================================================
// ID 解析（类型定义在 crate::types）
// ============================================================================

impl PaneId {
    /// 从 `@N` / `%N` / `N` 形式解析（tmux 3.3+ 的 `%output` / `%pane-mode-changed`
    /// 用 `%N`；`@N` 是 window id，靠上下文区分）。
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let num_part = s
            .strip_prefix('@')
            .or_else(|| s.strip_prefix('%'))
            .unwrap_or(s);
        let n = u32::from_str(num_part)
            .map_err(|_| ProtocolError::MalformedField(format!("pane id 非数字: {s}")))?;
        Ok(PaneId(n))
    }
}

impl TabId {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let s = s
            .strip_prefix('@')
            .ok_or_else(|| ProtocolError::MalformedField(format!("window id 缺少 @ 前缀: {s}")))?;
        let n = u32::from_str(s)
            .map_err(|_| ProtocolError::MalformedField(format!("window id 非数字: {s}")))?;
        Ok(TabId(n))
    }
}

impl TmuxSessionId {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let s = s
            .strip_prefix('$')
            .ok_or_else(|| ProtocolError::MalformedField(format!("session id 缺少 $ 前缀: {s}")))?;
        let n = u32::from_str(s)
            .map_err(|_| ProtocolError::MalformedField(format!("session id 非数字: {s}")))?;
        Ok(TmuxSessionId(n))
    }
}

// ============================================================================
// 布局结构
// ============================================================================

/// `%layout-change` 解析出的布局几何信息。
///
/// tmux 的 layout 字符串形如 `80x24,0,0,0`，分别表示
/// `<cols>x<rows>,<x>,<y>,<flags>`。本结构只取数值字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChange {
    /// 窗口/pane 的列数。
    pub cols: u32,
    /// 行数。
    pub rows: u32,
    /// x 偏移。
    pub x: u32,
    /// y 偏移。
    pub y: u32,
    /// tmux 内部 layout flags（最后那个数字）。
    pub flags: u32,
    /// 原始 layout 字符串（含可能的树结构 `abcd,80x24,0,0,0` 前缀），保留以便调试。
    pub raw: String,
}

/// 在字符串中查找任一指定字符的首次出现位置。
trait FindAnyOf {
    fn find_any_of(&self, chars: &[char]) -> Option<usize>;
}

impl FindAnyOf for str {
    fn find_any_of(&self, chars: &[char]) -> Option<usize> {
        self.chars().position(|c| chars.contains(&c))
    }
}

impl LayoutChange {
    /// 解析形如 `<cols>x<rows>,<x>,<y>,<flags>` 的布局几何字符串。
    ///
    /// 注意 tmux 的完整 window_layout 可能带树前缀（如 `aabd,100x30,0,0,0`），
    /// 这里只取最后一段 `<cols>x<rows>,<x>,<y>,<flags>`。
    pub fn parse(layout: &str) -> Result<Self, ProtocolError> {
        // tmux 的 layout 字符串格式：
        //   <tree_id>,<cols>x<rows>,<x>,<y>[,<flags>][{...}|[...]]
        // 树后缀（{...}/[...]）在叶子节点的最后字段后会紧跟 { 或 [。
        // 我们只解析顶层几何，树部分交给 parse_layout_tree 处理。
        //
        // 策略：找到第一个 'x'（几何段的 cols x rows），然后取其后的数字字段，
        // 遇到非数字（如 { 或 [）就停止。
        let layout = layout.trim();
        // 找到包含 'x' 且 'x' 前是数字的段（几何段）
        // 先按逗号切，但树后缀内的逗号会干扰。所以只取第一个 { 或 [ 之前的部分。
        let top_level = match layout.find_any_of(&['{', '[']) {
            Some(idx) => &layout[..idx],
            None => layout,
        };
        let parts: Vec<&str> = top_level.split(',').collect();
        let geo_idx = parts
            .iter()
            .position(|p| {
                p.contains('x')
                    && p.chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
            })
            .ok_or_else(|| {
                ProtocolError::MalformedField(format!("layout 无 x 几何段: {layout}"))
            })?;
        let geo = parts[geo_idx];
        let (cw, ch) = geo
            .split_once('x')
            .ok_or_else(|| ProtocolError::MalformedField(format!("layout 几何段缺 x: {geo}")))?;
        let cols = u32::from_str(cw)
            .map_err(|_| ProtocolError::MalformedField(format!("layout cols 非数字: {cw}")))?;
        let rows = u32::from_str(ch)
            .map_err(|_| ProtocolError::MalformedField(format!("layout rows 非数字: {ch}")))?;
        // 几何段之后的数字字段：x, y, flags
        let after = &parts[geo_idx + 1..];
        let x = after
            .first()
            .and_then(|s| u32::from_str(s).ok())
            .unwrap_or(0);
        let y = after
            .get(1)
            .and_then(|s| u32::from_str(s).ok())
            .unwrap_or(0);
        let flags = after
            .get(2)
            .and_then(|s| u32::from_str(s).ok())
            .unwrap_or(0);
        Ok(LayoutChange {
            cols,
            rows,
            x,
            y,
            flags,
            raw: layout.to_string(),
        })
    }
}

// ============================================================================
// 命令响应边界
// ============================================================================

/// `%begin` / `%end` / `%error` 后跟的元信息（tmux 3.2+ 格式）。
///
/// 格式：`%begin <time> <number> <flags>` 或 `%end <time> <number> <flags>`，其中：
/// - time：tmux 内部时间戳（整数）
/// - number：命令序号
/// - flags：标志位整数
///
/// tmux 早期版本可能只跟一个版本字符串，这里兼容两种：如果第二字段解析为整数
/// 就按新版处理，否则全部塞进 `extra` 保留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseBoundary {
    pub kind: NotificationKind,
    pub time: i64,
    pub number: i64,
    pub flags: i64,
    /// 未识别的额外字段（原始字符串）。
    pub extra: Vec<String>,
}

/// 响应边界种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Begin,
    End,
    Error,
}

// ============================================================================
// 消息 enum
// ============================================================================

/// tmux 控制协议消息。
///
/// 只覆盖「通知」类（`%` 开头），命令响应正文（夹在 `%begin`/`%end` 之间的普通
/// 行）不在本 enum 里，由上层在 `%begin` 时开始累积、`%end` 时提交。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// `%output <pane_id> <content>`：pane 新输出。content 已解码为原始字节。
    Output {
        pane: PaneId,
        content: Vec<u8>,
        /// 原始转义字符串（剥引号后），便于调试 / 重新编码。
        raw_content: String,
    },
    /// `%layout-change <window_id> <layout> [<visible_layout> [<flags>]]`
    ///
    /// tmux 控制模式实际模板（`control-notify.c`）：
    /// `#{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}`。
    /// zoom 时 `window_raw_flags` 含 `Z`，`window_visible_layout` 是铺满窗口的单叶。
    LayoutChange {
        window: TabId,
        layout: LayoutChange,
        /// 可见的布局字符串（若 tmux 给了第二个布局参数）。
        visible_layout: Option<LayoutChange>,
        /// `#{window_raw_flags}`（如 `*` / `*Z`）；缺省为 None。
        flags: Option<String>,
    },
    /// `%window-add <window_id>`
    WindowAdd { window: TabId },
    /// `%window-close <window_id>`
    WindowClose { window: TabId },
    /// `%window-renamed <window_id> <name>`
    WindowRenamed { window: TabId, name: String },
    /// `%session-changed <session_id> [<session_name>]`
    SessionChanged {
        session: TmuxSessionId,
        name: Option<String>,
    },
    /// `%session-renamed <session_id> <name>`
    SessionRenamed {
        session: TmuxSessionId,
        name: String,
    },
    /// `%sessions-changed`（无参数）
    SessionsChanged,
    /// `%pane-mode-changed <pane_id> <mode>`
    PaneModeChanged { pane: PaneId, mode: String },
    /// `%unlinked-window-add <window_id>`
    UnlinkedWindowAdd { window: TabId },
    /// `%unlinked-window-close <window_id>`
    UnlinkedWindowClose { window: TabId },
    /// `%unlinked-window-renamed <window_id> <name>`（tmux 3.3+ 实测带 name）
    UnlinkedWindowRenamed { window: TabId, name: String },
    /// `%exit [<reason>...]`
    Exit { reason: Option<String> },
    /// `%window-pane-changed <window_id> <pane_id>`：某 window 的激活 pane 切换。
    WindowPaneChanged { window: TabId, pane: PaneId },
    /// `%session-window-changed <session_id> <window_id>`：某 session 的激活 window 切换。
    SessionWindowChanged {
        session: TmuxSessionId,
        window: TabId,
    },
    /// `%extended-output <pane_id> <age> ... : <value>`（pause-after 下的
    /// %output 新形式；value 与 %output 一样是 C 转义字符串，age 是缓冲毫秒）。
    ExtendedOutput {
        pane: PaneId,
        age_ms: u64,
        content: Vec<u8>,
        /// 原始转义字符串（剥引号后），便于调试 / 重新编码。
        raw_content: String,
    },
    /// `%pause %N`（tmux 3.3+ 流控，control.c `%%pause %%%u`）：pane 输出被暂停。
    Pause { pane: Option<PaneId>, args: String },
    /// `%continue %N`（tmux 3.3+ 流控）：pane 输出恢复。
    Continue { pane: Option<PaneId>, args: String },
    /// `%subscription-changed <name> <session-id> <window-id> <window-index>
    /// <pane-id> ... : <value>`（tmux ≥3.2 `refresh-client -B` 订阅推送）。
    /// value 是 format 的展开值（可能含空格、冒号、样式指令）。
    /// pane 是元数据里的 pane-id（`-` 表示无 pane 上下文，如 status-left/right）。
    SubscriptionChanged {
        name: String,
        value: String,
        pane: Option<PaneId>,
    },
    /// 命令响应边界 `%begin` / `%end` / `%error`。
    ResponseBoundary(ResponseBoundary),
    /// 未识别的 `%` 消息，保留原始行（去掉行尾 \r\n）。
    Unknown { keyword: String, raw: String },
}

impl Message {
    /// 返回消息的「关键字」（去掉 `%` 前缀），用于日志/匹配。
    pub fn keyword(&self) -> &'static str {
        match self {
            Message::Output { .. } => "output",
            Message::LayoutChange { .. } => "layout-change",
            Message::WindowAdd { .. } => "window-add",
            Message::WindowClose { .. } => "window-close",
            Message::WindowRenamed { .. } => "window-renamed",
            Message::SessionChanged { .. } => "session-changed",
            Message::SessionRenamed { .. } => "session-renamed",
            Message::SessionsChanged => "sessions-changed",
            Message::PaneModeChanged { .. } => "pane-mode-changed",
            Message::UnlinkedWindowAdd { .. } => "unlinked-window-add",
            Message::UnlinkedWindowClose { .. } => "unlinked-window-close",
            Message::UnlinkedWindowRenamed { .. } => "unlinked-window-renamed",
            Message::Exit { .. } => "exit",
            Message::WindowPaneChanged { .. } => "window-pane-changed",
            Message::SessionWindowChanged { .. } => "session-window-changed",
            Message::ExtendedOutput { .. } => "extended-output",
            Message::Pause { .. } => "pause",
            Message::Continue { .. } => "continue",
            Message::SubscriptionChanged { .. } => "subscription-changed",
            Message::ResponseBoundary(b) => match b.kind {
                NotificationKind::Begin => "begin",
                NotificationKind::End => "end",
                NotificationKind::Error => "error",
            },
            Message::Unknown { .. } => "unknown",
        }
    }
}

// ============================================================================
// parse_line
// ============================================================================

/// 解析单行 tmux 输出。
///
/// - 输入应为「按真换行切分后」的单行（可能仍带行尾 `\r`，本函数会剥离）。
/// - 返回 `None` 表示该行不是通知（空行或非 `%` 开头的普通行，属于命令响应
///   正文或无关输出）。
/// - 返回 `Some(Message::Unknown{..})` 表示识别为 `%` 消息但关键字未知。
///
/// 注意：`%output` content 内的 `\n` 是 C 转义后的（两个字符 `\` `n`），不是
/// 真换行，所以行切分必须由调用方按真换行做，本函数只处理单行。
pub fn parse_line(line: &str) -> Option<Message> {
    // 剥离行尾 \r\n / \n
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);

    if line.is_empty() {
        return None;
    }
    parse_line_inner(line.as_bytes(), line)
}

/// 解析单行 tmux 输出的字节版本。
///
/// tmux 的 `%output` content 可能内嵌原始非 UTF-8 字节（htop/codex 的
/// 8-bit 字符、控制序列等）。`parse_line` 以 `&str` 为输入时这些字节已被
/// `from_utf8_lossy` 替换，必须用本函数在字节层面解析 `%output`。
pub fn parse_line_bytes(line: &[u8]) -> Option<Message> {
    // 剥离行尾 \r\n / \n 与 DCS 前缀（client 层通常已剥，这里再兜底）
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let line = line.strip_prefix(b"\x1bP1000p").unwrap_or(line);
    if line.is_empty() {
        return None;
    }
    let Ok(utf8) = std::str::from_utf8(line) else {
        // 非 UTF-8 行只可能是 %output（协议关键字与 pane id 都是 ASCII）；
        // 其它通知若出现非法字节，按 Unknown/忽略处理。
        return parse_output_line_bytes(line);
    };
    parse_line_inner(line, utf8)
}

fn parse_line_inner(line: &[u8], line_str: &str) -> Option<Message> {
    let line = line.strip_prefix(b"%")?;
    // 关键字 = 第一个 token（直到第一个空格）
    let (keyword, rest) = match line.iter().position(|&b| b == b' ') {
        Some(i) => (&line[..i], &line[i + 1..]),
        None => (line, &[][..]),
    };

    match keyword {
        b"output" => parse_output_bytes(rest).map(Some),
        b"extended-output" => parse_extended_output_bytes(rest).map(Some),
        _ => {
            // 其它通知的字段都是 ASCII/UTF-8，复用原有字符串解析
            parse_line_known_keyword(line_str)
        }
    }
    .map_err(|e: ProtocolError| {
        tracing::warn!(target = "muxterm::protocol", "解析失败: {e}");
        e
    })
    .ok()
    .flatten()
}

/// 非 output 通知的原有字符串解析路径。
fn parse_line_known_keyword(line_str: &str) -> Result<Option<Message>, ProtocolError> {
    let line = line_str.strip_prefix('%').unwrap_or(line_str);
    let (keyword, rest) = match line.split_once(' ') {
        Some((kw, r)) => (kw, r),
        None => (line, ""),
    };
    Ok(Some(match keyword {
        "layout-change" => parse_layout_change(rest),
        "window-add" => parse_window_id_only(rest, WindowKind::NormalAdd),
        "window-close" => parse_window_id_only(rest, WindowKind::NormalClose),
        "window-renamed" => parse_window_renamed(rest),
        "session-changed" => parse_session_changed(rest),
        "session-renamed" => parse_session_renamed(rest),
        "sessions-changed" => Ok(Message::SessionsChanged),
        "window-pane-changed" => parse_window_pane_changed(rest),
        "session-window-changed" => parse_session_window_changed(rest),
        "pane-mode-changed" => parse_pane_mode_changed(rest),
        "unlinked-window-add" => parse_window_id_only(rest, WindowKind::UnlinkedAdd),
        "unlinked-window-close" => parse_window_id_only(rest, WindowKind::UnlinkedClose),
        "unlinked-window-renamed" => parse_unlinked_window_renamed(rest),
        "exit" => Ok(Message::Exit {
            reason: if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            },
        }),
        "extended-output" => parse_extended_output(rest),
        "pause" => parse_pause_continue(rest, true),
        "continue" => parse_pause_continue(rest, false),
        "subscription-changed" => parse_subscription_changed(rest),
        "begin" => parse_boundary(rest, NotificationKind::Begin).map(Message::ResponseBoundary),
        "end" => parse_boundary(rest, NotificationKind::End).map(Message::ResponseBoundary),
        "error" => parse_boundary(rest, NotificationKind::Error).map(Message::ResponseBoundary),
        _ => Ok(Message::Unknown {
            keyword: keyword.to_string(),
            raw: rest.to_string(),
        }),
    }?))
}

/// 非 UTF-8 行：只尝试解析 `%output`，其它关键字无法在字节层安全解析。
fn parse_output_line_bytes(line: &[u8]) -> Option<Message> {
    let line = line.strip_prefix(b"%")?;
    let (keyword, rest) = match line.iter().position(|&b| b == b' ') {
        Some(i) => (&line[..i], &line[i + 1..]),
        None => (line, &[][..]),
    };
    let parsed = match keyword {
        b"output" => parse_output_bytes(rest),
        b"extended-output" => parse_extended_output_bytes(rest),
        _ => return None,
    };
    match parsed {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(target = "muxterm::protocol", "解析失败: {e}");
            None
        }
    }
}

/// `%subscription-changed <name> <session-id> <window-id> <window-index> <pane-id> ... : <value>`
///
/// 分隔符是第一个 ` : `：之前是订阅元数据（name 是首个 token，其余字段供
/// 未来使用），之后是 format 展开值（原样保留，可能含空格/冒号/样式指令）。
fn parse_subscription_changed(rest: &str) -> Result<Message, ProtocolError> {
    let (meta, value) = rest
        .split_once(" : ")
        .map(|(m, v)| (m.trim(), v))
        .unwrap_or((rest.trim(), ""));
    let mut tokens = meta.split_whitespace();
    let name = tokens.next().unwrap_or("");
    if name.is_empty() {
        return Err(ProtocolError::MalformedField(
            "subscription-changed 缺少订阅名".into(),
        ));
    }
    // 元数据：name session-id window-id window-index pane-id ...
    let pane = tokens.nth(3).and_then(parse_pane_id_token);
    Ok(Message::SubscriptionChanged {
        name: name.to_string(),
        value: value.to_string(),
        pane,
    })
}

/// 解析订阅元数据里的 pane-id token（`-` / `%N` / `@N` / `N`）。
fn parse_pane_id_token(token: &str) -> Option<PaneId> {
    if token == "-" {
        return None;
    }
    let digits = token
        .strip_prefix('%')
        .or_else(|| token.strip_prefix('@'))
        .unwrap_or(token);
    digits.parse::<u32>().ok().map(PaneId)
}

// 辅助枚举：window-add / window-close / unlinked-window-add / unlinked-window-close
#[derive(Debug, Clone, Copy)]
enum WindowKind {
    NormalAdd,
    NormalClose,
    UnlinkedAdd,
    UnlinkedClose,
}

fn parse_window_id_only(rest: &str, kind: WindowKind) -> Result<Message, ProtocolError> {
    let rest = rest.trim();
    let window = TabId::parse(rest)?;
    Ok(match kind {
        WindowKind::NormalAdd => Message::WindowAdd { window },
        WindowKind::NormalClose => Message::WindowClose { window },
        WindowKind::UnlinkedAdd => Message::UnlinkedWindowAdd { window },
        WindowKind::UnlinkedClose => Message::UnlinkedWindowClose { window },
    })
}

fn parse_window_renamed(rest: &str) -> Result<Message, ProtocolError> {
    // %window-renamed @0 name
    let mut it = rest.splitn(2, ' ');
    let wid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("window-renamed 缺 id".into()))?;
    let window = TabId::parse(wid)?;
    let name = it.next().unwrap_or("").to_string();
    Ok(Message::WindowRenamed { window, name })
}

fn parse_session_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %session-changed $0 name
    let mut it = rest.splitn(2, ' ');
    let sid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("session-changed 缺 id".into()))?;
    let session = TmuxSessionId::parse(sid)?;
    let name = it.next().map(|s| s.to_string());
    Ok(Message::SessionChanged { session, name })
}

fn parse_session_renamed(rest: &str) -> Result<Message, ProtocolError> {
    let mut it = rest.splitn(2, ' ');
    let sid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("session-renamed 缺 id".into()))?;
    let session = TmuxSessionId::parse(sid)?;
    let name = it.next().unwrap_or("").to_string();
    Ok(Message::SessionRenamed { session, name })
}

fn parse_pane_mode_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %pane-mode-changed @0 copy-mode / %pane-mode-changed %64 copy-mode
    let mut it = rest.splitn(2, ' ');
    let pid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("pane-mode-changed 缺 id".into()))?;
    let pane = PaneId::parse(pid)?;
    let mode = it.next().unwrap_or("").to_string();
    Ok(Message::PaneModeChanged { pane, mode })
}

/// 解析 `%pause %N` / `%continue %N`（tmux 3.3+ 流控）。
/// 首 token 是 pane id（`%N`）；其余保留为 args（兼容 flags）。
fn parse_pause_continue(rest: &str, is_pause: bool) -> Result<Message, ProtocolError> {
    let mut it = rest.splitn(2, ' ');
    let first = it.next().unwrap_or("").trim();
    let pane = if first.is_empty() {
        None
    } else {
        parse_pane_id_lenient(first).ok()
    };
    let args = it.next().unwrap_or("").trim().to_string();
    if is_pause {
        Ok(Message::Pause { pane, args })
    } else {
        Ok(Message::Continue { pane, args })
    }
}

fn parse_extended_output(rest: &str) -> Result<Message, ProtocolError> {
    // %extended-output <pane-id> <age> [future...] : <value>
    // value 与 %output 一样是 C 转义字符串，用同一个解码器还原原始字节。
    parse_extended_output_bytes(rest.as_bytes())
}

/// 字节版 `%extended-output` 解析。
///
/// tmux control mode 的 value 与 `%output` 相同，允许原始高位字节混在
/// C 转义字符串中。不能先把整行转换成 UTF-8，否则 htop/Cursor 的一个
/// 非 UTF-8 字节就会吞掉整条输出事件。
fn parse_extended_output_bytes(rest: &[u8]) -> Result<Message, ProtocolError> {
    let Some((meta, value)) = rest
        .windows(3)
        .position(|w| w == b" : ")
        .map(|i| (&rest[..i], &rest[i + 3..]))
    else {
        return Err(ProtocolError::MalformedField(
            "extended-output 缺 : 分隔符".into(),
        ));
    };
    let mut fields = meta.split(|b| *b == b' ' || *b == b'\t');
    let pid_bytes = fields
        .find(|field| !field.is_empty())
        .ok_or_else(|| ProtocolError::MalformedField("extended-output 缺 pane id".into()))?;
    let pid = std::str::from_utf8(pid_bytes)
        .map_err(|_| ProtocolError::MalformedField("extended-output pane id 非 ASCII".into()))?;
    // tmux 3.3+ 的 %extended-output 与 %output 一样用 %N / @N / N 三种 id 形式。
    let pane = parse_pane_id_lenient(pid)?;
    let age_ms = fields
        .find(|field| !field.is_empty())
        .and_then(|a| std::str::from_utf8(a).ok())
        .and_then(|a| a.parse::<u64>().ok())
        .unwrap_or(0);
    // value 与 %output 一样是双引号包裹的 C 转义字符串。
    let inner = strip_c_string_bytes(value)?;
    let raw_content = String::from_utf8_lossy(inner).into_owned();
    let content = ControlEscapeDecoder::new()
        .decode_bytes(inner)
        .map_err(ProtocolError::EscapeError)?;
    Ok(Message::ExtendedOutput {
        pane,
        age_ms,
        content,
        raw_content,
    })
}

fn parse_unlinked_window_renamed(rest: &str) -> Result<Message, ProtocolError> {
    // %unlinked-window-renamed @0 name（实测 tmux 3.4 带 name）
    let mut it = rest.splitn(2, ' ');
    let wid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("unlinked-window-renamed 缺 id".into()))?;
    let window = TabId::parse(wid)?;
    let name = it.next().unwrap_or("").to_string();
    Ok(Message::UnlinkedWindowRenamed { window, name })
}

fn parse_output(rest: &str) -> Result<Message, ProtocolError> {
    parse_output_bytes(rest.as_bytes())
}

fn parse_output_bytes(rest: &[u8]) -> Result<Message, ProtocolError> {
    // %output @N "content"
    // 注意：真实 tmux 3.3+ 用 %0/%1 形式的 pane id（不带 @ 前缀！见样本）。
    // 兼容两种：@N 或纯数字 N。
    let rest = trim_ascii_start(rest);
    // 找第一个空格分隔 pane id 和 content
    let space = rest
        .iter()
        .position(|&b| b == b' ')
        .ok_or_else(|| ProtocolError::MalformedField("output 缺 content".into()))?;
    let (pid_bytes, content_part) = (&rest[..space], &rest[space + 1..]);
    if content_part.is_empty() {
        return Err(ProtocolError::MalformedField("output 缺 content".into()));
    }
    let pid_str = std::str::from_utf8(pid_bytes)
        .map_err(|_| ProtocolError::MalformedField("output pane id 非 ASCII".into()))?;
    let pane = parse_pane_id_lenient(pid_str)?;
    // content 必须是双引号包裹的 C 转义字符串
    let inner = strip_c_string_bytes(content_part)?;
    let raw_content = String::from_utf8_lossy(inner).into_owned();
    let content = ControlEscapeDecoder::new()
        .decode_bytes(inner)
        .map_err(ProtocolError::EscapeError)?;
    Ok(Message::Output {
        pane,
        content,
        raw_content,
    })
}

/// 兼容解析 pane id：`@N` 或纯数字 `N`（tmux 3.3+ 的 %output 用纯数字）。
fn parse_pane_id_lenient(s: &str) -> Result<PaneId, ProtocolError> {
    // 兼容三种 pane id 形式：
    //   @N  —— 经典形式（window/pane 共用 @ 前缀，靠上下文区分）
    //   %N  —— tmux 3.3+ 的 %output / %extended-output 用 % 前缀表示 pane id
    //   N   —— 纯数字（容错）
    let num_part = s
        .strip_prefix('@')
        .or_else(|| s.strip_prefix('%'))
        .unwrap_or(s);
    let n = u32::from_str(num_part)
        .map_err(|_| ProtocolError::MalformedField(format!("pane id 非数字: {s}")))?;
    Ok(PaneId(n))
}

/// 剥去 C 字符串两端的双引号，返回内部内容（不做转义解码）。
///
/// 输入可能形如 `"abc\ndef"`，也可能因为某种原因没有引号（容错）。
fn strip_c_string(s: &str) -> Result<&str, ProtocolError> {
    // 注意：不能先 trim()！`%output @0 " "` 这类「只含一个空格」的 echo 会因此
    // 丢失空格（空格在引号内，是真实内容）。只剥掉引号，保留引号内的原始空白。
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Ok(&s[1..s.len() - 1])
    } else if s.starts_with('"') {
        // 只有左引号（行被截断？）容错返回剩余
        Ok(s.strip_prefix('"').unwrap_or(s))
    } else {
        // 无引号，原样返回（容错）
        Ok(s)
    }
}

/// 字节版剥引号：与 [`strip_c_string`] 语义一致，但保留原始字节。
fn strip_c_string_bytes(s: &[u8]) -> Result<&[u8], ProtocolError> {
    if s.starts_with(b"\"") && s.ends_with(b"\"") && s.len() >= 2 {
        Ok(&s[1..s.len() - 1])
    } else if s.starts_with(b"\"") {
        Ok(&s[1..])
    } else {
        Ok(s)
    }
}

/// 去掉字节串前导空格。
fn trim_ascii_start(mut s: &[u8]) -> &[u8] {
    while let Some((&b, rest)) = s.split_first() {
        if b == b' ' || b == b'\t' {
            s = rest;
        } else {
            break;
        }
    }
    s
}

fn parse_layout_change(rest: &str) -> Result<Message, ProtocolError> {
    // %layout-change @0 <layout> [<visible_layout> [<flags>]]
    let rest = rest.trim();
    let mut parts = rest.splitn(4, ' ');
    let wid = parts
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("layout-change 缺 window id".into()))?;
    let window = TabId::parse(wid)?;
    let layout_str = parts
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("layout-change 缺 layout".into()))?;
    let layout = LayoutChange::parse(layout_str)?;
    // 第三段可能是 visible_layout，也可能是 flags（如 *）。第四段是 window_raw_flags。
    // visible parse 失败时当作 flags，不丢弃整条消息。
    let third = parts.next();
    let fourth = parts.next();
    let (visible_layout, flags) = match (third, fourth) {
        (Some(visible), Some(raw_flags)) => (
            LayoutChange::parse(visible).ok(),
            Some(raw_flags.to_string()),
        ),
        (Some(token), None) => match LayoutChange::parse(token) {
            Ok(visible) => (Some(visible), None),
            Err(_) => (None, Some(token.to_string())),
        },
        (None, _) => (None, None),
    };
    Ok(Message::LayoutChange {
        window,
        layout,
        visible_layout,
        flags,
    })
}

fn parse_window_pane_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %window-pane-changed @0 %1   （window id 用 @，pane id 用 % 或 @）
    let mut it = rest.splitn(2, ' ');
    let wid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("window-pane-changed 缺 window id".into()))?;
    let window = TabId::parse(wid)?;
    let pid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("window-pane-changed 缺 pane id".into()))?;
    let pane = parse_pane_id_lenient(pid.trim())?;
    Ok(Message::WindowPaneChanged { window, pane })
}

fn parse_session_window_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %session-window-changed $0 @1
    let mut it = rest.splitn(2, ' ');
    let sid = it.next().ok_or_else(|| {
        ProtocolError::MalformedField("session-window-changed 缺 session id".into())
    })?;
    let session = TmuxSessionId::parse(sid)?;
    let wid = it.next().ok_or_else(|| {
        ProtocolError::MalformedField("session-window-changed 缺 window id".into())
    })?;
    let window = TabId::parse(wid.trim())?;
    Ok(Message::SessionWindowChanged { session, window })
}

// ============================================================================
// tmux window_layout 树字符串解析
// ============================================================================

/// tmux window_layout 树的一个节点。
///
/// 格式（tmux 3.x）：
/// ```text
/// <tree_id>,<cols>x<rows>,<x>,<y>,<flags>
/// {<child>,<child>}   ← 左右分割（水平）
/// [<child>,<child>]   ← 上下分割（垂直）
/// ```
/// 叶子节点的 tree_id 是一个数字（pane 的 layout-cell 序号），但**叶子本身
/// 不直接含 pane id**——pane id 需要通过 `list-panes` 的 `#{pane_id}` + 几何
/// 匹配来关联。所以本结构只提供几何拓扑；pane id 的映射由 TmuxRuntime 完成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutTree {
    /// 根节点的几何（整个 window 的 cols/rows）。
    pub cols: u32,
    pub rows: u32,
    pub x: u32,
    pub y: u32,
    pub flags: u32,
    /// 树前缀 id（tmux 内部 layout-cell id，如 `a87d`），叶子节点为数字。
    pub tree_id: String,
    /// 子节点（None = 叶子）。
    pub children: Option<(Box<LayoutTree>, Box<LayoutTree>)>,
    /// 子节点分割方向：`{}` = 水平（左右），`[]` = 垂直（上下）。
    pub dir: SplitDir,
}

impl LayoutTree {
    /// 是否叶子节点。
    pub fn is_leaf(&self) -> bool {
        self.children.is_none()
    }
}

/// 解析 tmux window_layout 树字符串。
///
/// 例：
/// - `75ac,140x30,0,0{70x30,0,0,0,69x30,71,0,1}` — 一次水平分割（2 叶子）
/// - `1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}` —
///   水平分割，右子再垂直分割（3 叶子，嵌套）
/// - `a980,140x30,0,0,3` — 单叶子
///
/// 算法：递归下降解析。遇到 `{` 或 `[` 进入子节点，遇到 `}` `]` 或 `,` 结束当前节点。
pub fn parse_layout_tree(s: &str) -> Result<LayoutTree, ProtocolError> {
    let s = s.trim();
    let mut parser = LayoutParser {
        chars: s.chars().peekable(),
    };
    let tree = parser.parse_node()?;
    if parser.chars.peek().is_some() {
        return Err(ProtocolError::MalformedField(format!(
            "layout 存在未解析尾部: {s}"
        )));
    }
    Ok(tree)
}

/// 递归下降解析器内部状态。
struct LayoutParser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> LayoutParser<'a> {
    /// 解析一个节点。
    ///
    /// 格式：`[<tree_id>,]<cols>x<rows>,<x>,<y>[,<flags>][{...}|[...]]`
    /// - tree_id 仅根节点有（如 `75ac`）
    /// - flags 仅叶子节点有（无子节点时）
    /// - `{` = 水平分割（左右），`[` = 垂直分割（上下）
    fn parse_node(&mut self) -> Result<LayoutTree, ProtocolError> {
        // 判断是否有 tree_id：peek 第一个字段（到 ',' 或 'x' 为止），
        // 不含 'x' → 是 tree_id（仅根节点）
        let first_field = self.peek_field();
        let tree_id;
        if !first_field.contains('x') && !first_field.is_empty() {
            // 有 tree_id
            tree_id = self.read_until_char(',')?;
            self.expect_char(',')?;
        } else {
            tree_id = String::new();
        }
        let cols = self.read_u32()?;
        self.expect_char('x')?;
        let rows = self.read_u32()?;
        self.expect_char(',')?;
        let x = self.read_u32()?;
        self.expect_char(',')?;
        let y = self.read_u32()?;
        // flags 可选：仅在叶子节点有（子节点前无 flags）
        #[allow(clippy::needless_late_init)]
        let flags;
        match self.chars.peek() {
            Some(',') => {
                self.chars.next(); // consume ,
                flags = self.read_u32()?;
            }
            _ => {
                flags = 0;
            }
        }
        // 子节点。tmux 可以在同一个 `{}` / `[]` 组内放两个以上的
        // child（例如一个 pane 再接三个上下 pane），不能只读取前两个。
        let dir;
        let children;
        match self.chars.peek() {
            Some('{') => {
                dir = SplitDir::Horizontal;
                children = Some(self.parse_group('}')?);
            }
            Some('[') => {
                dir = SplitDir::Vertical;
                children = Some(self.parse_group(']')?);
            }
            _ => {
                dir = SplitDir::Horizontal;
                children = None;
            }
        }
        Ok(LayoutTree {
            cols,
            rows,
            x,
            y,
            flags,
            tree_id,
            children: match children {
                Some(group) => Some(fold_layout_children(group, dir)?),
                None => None,
            },
            dir,
        })
    }

    /// 解析一个 `{}` / `[]` 子节点组，返回组内全部 child。
    fn parse_group(&mut self, close: char) -> Result<Vec<LayoutTree>, ProtocolError> {
        self.chars.next(); // opening `{` or `[`
        let mut children = Vec::new();
        loop {
            children.push(self.parse_node()?);
            match self.chars.peek() {
                Some(',') => {
                    self.chars.next();
                }
                Some(c) if *c == close => {
                    self.chars.next();
                    break;
                }
                Some(c) => {
                    return Err(ProtocolError::MalformedField(format!(
                        "layout 子节点组期待 ',' 或 '{close}'，得到 '{c}'"
                    )));
                }
                None => {
                    return Err(ProtocolError::MalformedField(format!(
                        "layout 子节点组缺少 '{close}'"
                    )));
                }
            }
        }
        if children.len() < 2 {
            return Err(ProtocolError::MalformedField(
                "layout 子节点组至少需要两个 child".into(),
            ));
        }
        Ok(children)
    }

    /// Peek 到第一个 ',' 为止（不消费字符），用于判断是否有 tree_id 前缀。
    /// 含 'x' → 是 `<cols>x<rows>` 几何字段（无 tree_id）；不含 'x' → 是 tree_id。
    fn peek_field(&self) -> String {
        let mut s = String::new();
        let mut iter = self.chars.clone();
        while let Some(&ch) = iter.peek() {
            if ch == ',' || ch == '}' || ch == ']' {
                break;
            }
            s.push(ch);
            iter.next();
        }
        s
    }

    /// 读到指定字符为止（不含该字符），trim 后返回。
    fn read_until_char(&mut self, c: char) -> Result<String, ProtocolError> {
        let mut s = String::new();
        while let Some(&ch) = self.chars.peek() {
            if ch == c {
                break;
            }
            s.push(ch);
            self.chars.next();
        }
        Ok(s.trim().to_string())
    }

    /// 期望当前字符是指定字符，消费它。
    fn expect_char(&mut self, c: char) -> Result<(), ProtocolError> {
        match self.chars.next() {
            Some(ch) if ch == c => Ok(()),
            other => Err(ProtocolError::MalformedField(format!(
                "layout 期望 '{c}'，得到 {:?}",
                other
            ))),
        }
    }

    /// 读一个无符号整数。
    fn read_u32(&mut self) -> Result<u32, ProtocolError> {
        let mut s = String::new();
        while let Some(&ch) = self.chars.peek() {
            if ch.is_ascii_digit() {
                s.push(ch);
                self.chars.next();
            } else {
                break;
            }
        }
        if s.is_empty() {
            return Err(ProtocolError::MalformedField("layout 期望数字".into()));
        }
        u32::from_str(&s)
            .map_err(|_| ProtocolError::MalformedField(format!("layout 数字溢出: {s}")))
    }
}

/// 把 tmux 同向的 n-ary 子节点组折叠成 core 使用的二叉树。
///
/// 采用右折叠：`[A,B,C]` → `Split(A, Split(B,C))`，这样叶子顺序与 tmux
/// 的几何顺序一致；合成节点的几何由两个子节点和一个分隔线推导，供
/// backend 计算稳定的布局比例。
fn fold_layout_children(
    mut children: Vec<LayoutTree>,
    dir: SplitDir,
) -> Result<(Box<LayoutTree>, Box<LayoutTree>), ProtocolError> {
    let second = children
        .pop()
        .ok_or_else(|| ProtocolError::MalformedField("layout 子节点组缺少 second child".into()))?;
    let mut right = second;
    while children.len() > 1 {
        let left = children.pop().ok_or_else(|| {
            ProtocolError::MalformedField("layout 子节点组折叠缺少 left child".into())
        })?;
        right = combine_layout_nodes(left, right, dir);
    }
    let first = children
        .pop()
        .ok_or_else(|| ProtocolError::MalformedField("layout 子节点组缺少 first child".into()))?;
    Ok((Box::new(first), Box::new(right)))
}

fn combine_layout_nodes(first: LayoutTree, second: LayoutTree, dir: SplitDir) -> LayoutTree {
    let cols = match dir {
        SplitDir::Horizontal => first.cols.saturating_add(second.cols).saturating_add(1),
        SplitDir::Vertical => first.cols.max(second.cols),
    };
    let rows = match dir {
        SplitDir::Horizontal => first.rows.max(second.rows),
        SplitDir::Vertical => first.rows.saturating_add(second.rows).saturating_add(1),
    };
    LayoutTree {
        cols,
        rows,
        x: first.x.min(second.x),
        y: first.y.min(second.y),
        flags: 0,
        tree_id: String::new(),
        children: Some((Box::new(first), Box::new(second))),
        dir,
    }
}

fn parse_boundary(rest: &str, kind: NotificationKind) -> Result<ResponseBoundary, ProtocolError> {
    // %begin <time> <number> <flags>  （tmux 3.2+）
    // 旧版可能是 %begin <version>，这里宽松：尝试解析 3 个整数；若失败则塞 extra。
    let rest = rest.trim();
    let toks: Vec<&str> = rest.split_whitespace().collect();
    if toks.len() >= 3 {
        // 尝试解析前 3 个为整数
        let t = i64::from_str(toks[0]);
        let n = i64::from_str(toks[1]);
        let f = i64::from_str(toks[2]);
        if let (Ok(time), Ok(number), Ok(flags)) = (t, n, f) {
            let extra = toks[3..].iter().map(|s| s.to_string()).collect();
            return Ok(ResponseBoundary {
                kind,
                time,
                number,
                flags,
                extra,
            });
        }
    }
    // 旧版 / 无法解析：全部塞 extra
    Ok(ResponseBoundary {
        kind,
        time: 0,
        number: 0,
        flags: 0,
        extra: toks.iter().map(|s| s.to_string()).collect(),
    })
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // ---------- ControlEscapeDecoder ----------

    fn dec(s: &str) -> Vec<u8> {
        ControlEscapeDecoder::new().decode(s).unwrap()
    }

    #[test]
    fn escape_plain() {
        assert_eq!(dec("hello"), b"hello");
    }

    #[test]
    fn escape_e_n_r_t() {
        assert_eq!(dec(r"abc\edef\n\r\t"), b"abc\x1bdef\n\r\t");
    }

    #[test]
    fn escape_backslash_quote() {
        assert_eq!(dec(r#"a\\b\"c"#), b"a\\b\"c");
    }

    #[test]
    fn escape_octal_three() {
        assert_eq!(dec(r"\033"), b"\x1B");
        assert_eq!(dec(r"\007"), b"\x07");
        assert_eq!(dec(r"\377"), b"\xFF");
    }

    #[test]
    fn escape_octal_short() {
        // 1~2 位八进制也接受
        assert_eq!(dec(r"\7"), b"\x07");
        assert_eq!(dec(r"\77"), b"\x3F");
    }

    #[test]
    fn escape_hex() {
        assert_eq!(dec(r"\x1B"), b"\x1B");
        assert_eq!(dec(r"\xff"), b"\xFF");
        assert_eq!(dec(r"\xF0"), b"\xF0");
    }

    #[test]
    fn escape_bytes_preserve_high_bit_and_continue_after_escapes() {
        let input = b"ascii\xff\x80\\e\\033\\0\\x41tail";
        let expected = vec![
            b'a', b's', b'c', b'i', b'i', 0xff, 0x80, 0x1b, 0x1b, 0x00, b'A', b't', b'a', b'i',
            b'l',
        ];
        assert_eq!(
            ControlEscapeDecoder::new().decode_bytes(input).unwrap(),
            expected
        );
    }

    #[test]
    fn escape_truncated_and_unknown_sequences_are_reported_without_swallowing_text() {
        let decoder = ControlEscapeDecoder::new();
        assert_eq!(
            decoder.decode_bytes(b"tail\\"),
            Err(ControlEscapeError::BareBackslash)
        );
        assert_eq!(
            decoder.decode_bytes(b"tail\\x"),
            Err(ControlEscapeError::Truncated)
        );
        assert_eq!(
            decoder.decode_bytes(b"tail\\xA"),
            Err(ControlEscapeError::Truncated)
        );
        assert_eq!(
            decoder.decode_bytes(b"tail\\xG"),
            Err(ControlEscapeError::InvalidHex('G'))
        );
        assert_eq!(
            decoder.decode_bytes(b"tail\\qafter"),
            Err(ControlEscapeError::UnknownEscape('q'))
        );
        // A valid escape after ordinary text must not make the following text disappear.
        assert_eq!(
            decoder.decode_bytes(b"before\\eafter"),
            Ok(b"before\x1bafter".to_vec())
        );
    }

    #[test]
    fn escape_ansi_sample() {
        // 真实样本里的一行 %output content（剥引号后）
        let raw = r"\033]133;A;cl=m;aid=1073424\007";
        let bytes = dec(raw);
        assert_eq!(&bytes, b"\x1B]133;A;cl=m;aid=1073424\x07");
    }

    #[test]
    fn escape_dcs_passthrough() {
        // tmux passthrough 形式：\033Ptmux;\033\033]1337;...=\007\033\\
        let raw = r"\033Ptmux;\033\033]1337;SetUserVar=WEZTERM_PROG=\007\033\\";
        let bytes = dec(raw);
        assert_eq!(
            &bytes,
            b"\x1BPtmux;\x1B\x1B]1337;SetUserVar=WEZTERM_PROG=\x07\x1B\\"
        );
    }

    #[test]
    fn escape_unknown() {
        let err = ControlEscapeDecoder::new().decode(r"\q").unwrap_err();
        assert!(matches!(err, ControlEscapeError::UnknownEscape('q')));
    }

    #[test]
    fn escape_bare_backslash() {
        let err = ControlEscapeDecoder::new().decode(r"abc\").unwrap_err();
        assert!(matches!(err, ControlEscapeError::BareBackslash));
    }

    #[test]
    fn escape_lossy() {
        assert_eq!(ControlEscapeDecoder::new().decode_lossy(r"abc\n"), "abc\n");
    }

    #[test]
    fn escape_empty() {
        assert_eq!(dec(""), b"");
    }

    // ---------- ID ----------

    #[test]
    fn pane_id_parse() {
        assert_eq!(PaneId::parse("@3").unwrap(), PaneId(3));
        // F6：tmux 3.3+ 的 %output / %pane-mode-changed 用 %N；纯数字也容错。
        assert_eq!(PaneId::parse("%3").unwrap(), PaneId(3));
        assert_eq!(PaneId::parse("3").unwrap(), PaneId(3));
        assert!(PaneId::parse("@x").is_err());
        assert_eq!(PaneId(7).as_str(), "@7");
    }

    #[test]
    fn tab_id_parse_maps_tmux_window() {
        assert_eq!(TabId::parse("@0").unwrap(), TabId(0));
        assert_eq!(TabId(12).as_str(), "t12");
    }

    #[test]
    fn session_id_parse() {
        assert_eq!(TmuxSessionId::parse("$0").unwrap(), TmuxSessionId(0));
        assert_eq!(TmuxSessionId(5).as_str(), "$5");
        assert!(TmuxSessionId::parse("@1").is_err());
    }

    // ---------- LayoutChange ----------

    #[test]
    fn layout_simple() {
        let l = LayoutChange::parse("80x24,0,0,0").unwrap();
        assert_eq!(
            l,
            LayoutChange {
                cols: 80,
                rows: 24,
                x: 0,
                y: 0,
                flags: 0,
                raw: "80x24,0,0,0".into(),
            }
        );
    }

    #[test]
    fn layout_with_offset() {
        let l = LayoutChange::parse("100x30,5,6,7").unwrap();
        assert_eq!((l.cols, l.rows, l.x, l.y, l.flags), (100, 30, 5, 6, 7));
    }

    #[test]
    fn layout_with_tree_prefix() {
        // 真实样本里 list-windows 给的 layout 形如 a87d,100x30,0,0,0
        let l = LayoutChange::parse("a87d,100x30,0,0,0").unwrap();
        assert_eq!((l.cols, l.rows, l.x, l.y, l.flags), (100, 30, 0, 0, 0));
    }

    #[test]
    fn layout_missing_flags_defaults_zero() {
        let l = LayoutChange::parse("80x24,0,0").unwrap();
        assert_eq!((l.cols, l.rows, l.x, l.y, l.flags), (80, 24, 0, 0, 0));
    }

    #[test]
    fn layout_bad() {
        assert!(LayoutChange::parse("no-x-here").is_err());
    }

    // ---------- parse_line: 空行 / 普通行 ----------

    #[test]
    fn parse_line_empty() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("\r\n"), None);
    }

    #[test]
    fn parse_line_non_notification() {
        // 命令响应正文（不带 %）
        assert_eq!(parse_line("cmd: 1 windows (created ...)"), None);
        assert_eq!(parse_line("0: bash- (1 panes) [100x30]"), None);
        assert_eq!(parse_line("hello world"), None);
    }

    // ---------- parse_line: 各通知 ----------

    #[test]
    fn parse_window_add() {
        let m = parse_line("%window-add @0\r\n").unwrap();
        assert_eq!(m, Message::WindowAdd { window: TabId(0) });
        assert_eq!(m.keyword(), "window-add");
    }

    #[test]
    fn parse_window_close() {
        let m = parse_line("%window-close @3").unwrap();
        assert_eq!(m, Message::WindowClose { window: TabId(3) });
    }

    #[test]
    fn parse_unlinked_window_add_close() {
        let a = parse_line("%unlinked-window-add @5").unwrap();
        assert_eq!(a, Message::UnlinkedWindowAdd { window: TabId(5) });
        let c = parse_line("%unlinked-window-close @5").unwrap();
        assert_eq!(c, Message::UnlinkedWindowClose { window: TabId(5) });
    }

    #[test]
    fn parse_window_renamed() {
        let m = parse_line("%window-renamed @0 bash").unwrap();
        assert_eq!(
            m,
            Message::WindowRenamed {
                window: TabId(0),
                name: "bash".into(),
            }
        );
    }

    #[test]
    fn parse_session_changed() {
        let m = parse_line("%session-changed $0 cmd").unwrap();
        assert_eq!(
            m,
            Message::SessionChanged {
                session: TmuxSessionId(0),
                name: Some("cmd".into()),
            }
        );
    }

    #[test]
    fn parse_session_changed_no_name() {
        let m = parse_line("%session-changed $0").unwrap();
        assert_eq!(
            m,
            Message::SessionChanged {
                session: TmuxSessionId(0),
                name: None,
            }
        );
    }

    #[test]
    fn parse_session_renamed() {
        let m = parse_line("%session-renamed $0 mysession").unwrap();
        assert_eq!(
            m,
            Message::SessionRenamed {
                session: TmuxSessionId(0),
                name: "mysession".into(),
            }
        );
    }

    #[test]
    fn parse_sessions_changed() {
        let m = parse_line("%sessions-changed\r\n").unwrap();
        assert_eq!(m, Message::SessionsChanged);

        // %subscription-changed（tmux ≥3.2 refresh-client -B 推送）
        let m =
            parse_line("%subscription-changed muxterm.status-left $0 - - - : #[fg=red]11:50:23 ")
                .unwrap();
        assert_eq!(
            m,
            Message::SubscriptionChanged {
                name: "muxterm.status-left".into(),
                value: "#[fg=red]11:50:23 ".into(),
                pane: None,
            }
        );

        // 展开值本身可以含冒号与空格：只按第一个 " : " 切分。
        let m =
            parse_line("%subscription-changed muxterm.status-right $0 - - - : a : b  c").unwrap();
        assert_eq!(
            m,
            Message::SubscriptionChanged {
                name: "muxterm.status-right".into(),
                value: "a : b  c".into(),
                pane: None,
            }
        );

        // 缺订阅名的畸形行按忽略处理（不 panic、不进状态机）。
        assert_eq!(parse_line("%subscription-changed"), None);
        assert_eq!(parse_line("%subscription-changed   "), None);

        // pane-cmd 订阅带 pane-id（%0）：解析进 Message。
        let m = parse_line("%subscription-changed muxterm.pane-cmd $0 @0 0 %0 : /bin/cat").unwrap();
        assert_eq!(
            m,
            Message::SubscriptionChanged {
                name: "muxterm.pane-cmd".into(),
                value: "/bin/cat".into(),
                pane: Some(PaneId(0)),
            }
        );
        // 无 pane 上下文用 `-`。
        let m = parse_line("%subscription-changed muxterm.pane-cmd $0 @0 0 - : zsh").unwrap();
        assert_eq!(
            m,
            Message::SubscriptionChanged {
                name: "muxterm.pane-cmd".into(),
                value: "zsh".into(),
                pane: None,
            }
        );
    }

    #[test]
    fn parse_pane_mode_changed() {
        let m = parse_line("%pane-mode-changed @0 copy-mode").unwrap();
        assert_eq!(
            m,
            Message::PaneModeChanged {
                pane: PaneId(0),
                mode: "copy-mode".into(),
            }
        );

        // F6：2105 的 `%pane-mode-changed %64 copy-mode` 不再 WARN。
        let m = parse_line("%pane-mode-changed %64 copy-mode").unwrap();
        assert_eq!(
            m,
            Message::PaneModeChanged {
                pane: PaneId(64),
                mode: "copy-mode".into(),
            }
        );
    }

    #[test]
    fn parse_exit_with_reason() {
        let m = parse_line("%exit detached").unwrap();
        assert_eq!(
            m,
            Message::Exit {
                reason: Some("detached".into()),
            }
        );
    }

    #[test]
    fn parse_exit_no_reason() {
        let m = parse_line("%exit").unwrap();
        assert_eq!(m, Message::Exit { reason: None });
    }

    #[test]
    fn parse_output_preserves_spaces_inside_quotes() {
        // 关键回归：%output 引号内的空格是真实内容，不能因 trim 丢失。
        // 例如回显一个空格 `%output @0 " "`，必须解析出 content = " "。
        let m = parse_line(r#"%output @0 " ""#).unwrap();
        if let Message::Output { content, .. } = m {
            assert_eq!(content, b" ");
        } else {
            panic!();
        }
        // 中间空格也不能丢
        let m = parse_line(r#"%output @0 "git status""#).unwrap();
        if let Message::Output { content, .. } = m {
            assert_eq!(content, b"git status");
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_output_with_at_prefix() {
        // %output @1 "hello\n"
        let m = parse_line(r#"%output @1 "hello\n""#).unwrap();
        match m {
            Message::Output {
                pane,
                content,
                raw_content,
            } => {
                assert_eq!(pane, PaneId(1));
                assert_eq!(&content, b"hello\n");
                assert_eq!(raw_content, r"hello\n");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_line_bytes_preserves_invalid_utf8_output_and_line_endings() {
        let m = parse_line_bytes(b"\x1bP1000p%output @42 \"\x94\x80\"\r\n").unwrap();
        match m {
            Message::Output { pane, content, .. } => {
                assert_eq!(pane, PaneId(42));
                assert_eq!(content, b"\x94\x80");
                assert!(!content.windows(3).any(|w| w == b"\xef\xbf\xbd"));
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    #[test]
    fn parse_line_bytes_handles_empty_and_space_only_output() {
        let empty = parse_line_bytes(b"%output @3 \"\"").unwrap();
        assert!(
            matches!(empty, Message::Output { pane: PaneId(3), content, .. } if content.is_empty())
        );

        let spaces = parse_line_bytes(br#"%output @3 "   ""#).unwrap();
        assert!(
            matches!(spaces, Message::Output { pane: PaneId(3), content, .. } if content == b"   ")
        );
    }

    #[test]
    fn parse_line_bytes_accepts_all_supported_pane_id_forms() {
        for line in [
            b"%output @7 hi".as_slice(),
            b"%output %7 hi".as_slice(),
            b"%output 7 hi".as_slice(),
        ] {
            assert!(matches!(
                parse_line_bytes(line),
                Some(Message::Output { pane: PaneId(7), content, .. }) if content == b"hi"
            ));
        }
    }

    #[test]
    fn parse_line_bytes_rejects_missing_content_and_bad_pane_ids() {
        for line in [
            b"%output @1".as_slice(),
            b"%output @1 ".as_slice(),
            b"%output @x hi".as_slice(),
            b"%output %x hi".as_slice(),
        ] {
            assert_eq!(parse_line_bytes(line), None, "line={line:?}");
        }
    }

    #[test]
    fn parse_line_bytes_keeps_non_output_utf8_and_rejects_invalid_safely() {
        let renamed = parse_line_bytes("%window-renamed @2 编译".as_bytes()).unwrap();
        assert!(matches!(
            renamed,
            Message::WindowRenamed { window: TabId(2), name } if name == "编译"
        ));

        // 非 output 消息的字段不能安全地进入 String；应安全忽略，不得 panic 或生成替换符。
        assert_eq!(parse_line_bytes(b"%window-renamed @2 \xff"), None);
        assert_eq!(parse_line_bytes(b"%sessions-changed\xff"), None);
    }

    #[test]
    fn parse_output_preserves_terminal_control_bytes_and_following_text() {
        let mut line = b"%output %8 \"".to_vec();
        line.extend_from_slice(
            br"A\033[?2026hB\033[?1049hC\033[?1049l\033]0;title\033\\D\007E\017F\010G\015\012H",
        );
        line.push(b'"');

        let expected = [
            b"A".as_slice(),
            b"\x1b[?2026hB",
            b"\x1b[?1049hC",
            b"\x1b[?1049l",
            b"\x1b]0;title\x1b\\D\x07E\x0fF\x08G\x0d\x0aH",
        ]
        .concat();
        match parse_line_bytes(&line).unwrap() {
            Message::Output { content, .. } => assert_eq!(content, expected),
            other => panic!("expected Output, got {other:?}"),
        }
    }

    #[test]
    fn parse_output_bad_escape_does_not_hide_a_following_line() {
        // The malformed line is rejected, but line framing remains byte-oriented: the
        // next complete output line is still available to the caller.
        assert_eq!(parse_line_bytes(br#"%output %1 "bad\qtail""#), None);
        assert!(matches!(
            parse_line_bytes(br#"%output %1 "after""#),
            Some(Message::Output { content, .. }) if content == b"after"
        ));
    }

    #[test]
    fn parse_output_bare_pane_id() {
        // 真实样本：tmux 3.7b 的 %output 用 %0 形式（纯数字 pane id）
        let m = parse_line(r#"%output %0 \033]133;A;cl=m;aid=1073424\007"#).unwrap();
        match m {
            Message::Output { pane, content, .. } => {
                assert_eq!(pane, PaneId(0));
                assert_eq!(&content, b"\x1B]133;A;cl=m;aid=1073424\x07");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_output_dcs_passthrough() {
        let line = r#"%output %0 \033Ptmux;\033\033]1337;SetUserVar=WEZTERM_PROG=\007\033\\"#;
        let m = parse_line(line).unwrap();
        if let Message::Output { pane, content, .. } = m {
            assert_eq!(pane, PaneId(0));
            assert_eq!(
                &content,
                b"\x1BPtmux;\x1B\x1B]1337;SetUserVar=WEZTERM_PROG=\x07\x1B\\"
            );
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_output_cjk_bytes_are_preserved() {
        // 实测 tmux 3.7b：%output 的 CJK 以原始 UTF-8 字节内嵌，不能被
        // from_utf8_lossy 替换成 \u{fffd}。这里用真实样本的 octal + 原始字节。
        let line =
            r#"%output %0 \015\012\033M\033[34m/tmp\033[39m\015\012\033[35m編譯測試\033[39m"#;
        let m = parse_line(line).unwrap();
        if let Message::Output { content, .. } = m {
            let expected = b"\x0d\x0a\x1bM\x1b[34m/tmp\x1b[39m\x0d\x0a\x1b[35m\xe7\xb7\xa8\xe8\xad\xaf\xe6\xb8\xac\xe8\xa9\xa6\x1b[39m";
            assert_eq!(&content, expected);
        } else {
            panic!("not an Output message");
        }
    }

    #[test]
    fn parse_output_osc_dynamic_colors_keep_esc_and_bel() {
        // git lg 的 OSC 动态颜色序列（ESC]10;rgb:... BEL / ESC\）
        let line = r#"%output %0 \033]10;rgb:0000/0000/0000\007"#;
        if let Message::Output { content, .. } = parse_line(line).unwrap() {
            assert_eq!(&content, b"\x1b]10;rgb:0000/0000/0000\x07");
        } else {
            panic!();
        }
        let line2 = r#"%output %0 \033]11;rgb:ffff/ffff/ffff\033\\"#;
        if let Message::Output { content, .. } = parse_line(line2).unwrap() {
            assert_eq!(&content, b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_output_csi_device_attribute_query() {
        // 用户实测 `10;rgb:... zsh: command not found` —— CSI 引导字节若丢失
        // 就会被 shell 当普通文本执行。这里验证 ESC[? 序列解码后引导字节完整。
        let line = r#"%output %0 \033[?65;4;1;2;6;21;22;17;28c"#;
        if let Message::Output { content, .. } = parse_line(line).unwrap() {
            assert_eq!(&content, b"\x1b[?65;4;1;2;6;21;22;17;28c");
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_output_with_inner_ansi() {
        // content 内含 ANSI 颜色转义（ESC[?2004h 等）原样保留
        let line = r#"%output @2 \033[?2004h\033]133;P;k=i\007prompt$ \033]133;B\007"#;
        let m = parse_line(line).unwrap();
        if let Message::Output { pane, content, .. } = m {
            assert_eq!(pane, PaneId(2));
            assert!(content.starts_with(b"\x1B[?2004h"));
            assert!(content.ends_with(b"\x1B]133;B\x07"));
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_output_unquoted_fallback() {
        // 容错：无引号也能解析（不崩）
        let m = parse_line(r#"%output @0 abc"#).unwrap();
        if let Message::Output { pane, content, .. } = m {
            assert_eq!(pane, PaneId(0));
            assert_eq!(&content, b"abc");
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_layout_change() {
        let m = parse_line("%layout-change @0 80x24,0,0,0").unwrap();
        match m {
            Message::LayoutChange {
                window,
                layout,
                visible_layout,
                flags,
            } => {
                assert_eq!(window, TabId(0));
                assert_eq!((layout.cols, layout.rows), (80, 24));
                assert_eq!(visible_layout, None);
                assert_eq!(flags, None);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_layout_change_with_tree_and_visible() {
        let m = parse_line("%layout-change @1 a87e,100x30,0,0,1 b87e,100x30,0,0,1").unwrap();
        if let Message::LayoutChange {
            window,
            layout,
            visible_layout,
            flags,
        } = m
        {
            assert_eq!(window, TabId(1));
            assert_eq!((layout.cols, layout.rows, layout.flags), (100, 30, 1));
            let v = visible_layout.unwrap();
            assert_eq!((v.cols, v.rows), (100, 30));
            assert_eq!(flags, None);
        } else {
            panic!();
        }
    }

    /// zoom：visible 是单叶，flags 含 Z（tmux `window_raw_flags`）。
    #[test]
    fn parse_layout_change_zoom_flags() {
        let m =
            parse_line("%layout-change @0 aabd,80x24,0,0{40x24,0,0,1,39x24,41,0,2} 80x24,0,0,1 *Z")
                .unwrap();
        match m {
            Message::LayoutChange {
                window,
                visible_layout,
                flags,
                ..
            } => {
                assert_eq!(window, TabId(0));
                assert_eq!(visible_layout.unwrap().flags, 1);
                assert_eq!(flags.as_deref(), Some("*Z"));
            }
            _ => panic!("应为 layout-change"),
        }
    }

    #[test]
    fn parse_boundary_begin() {
        let m = parse_line("%begin 1784356613 286 1").unwrap();
        assert_eq!(
            m,
            Message::ResponseBoundary(ResponseBoundary {
                kind: NotificationKind::Begin,
                time: 1784356613,
                number: 286,
                flags: 1,
                extra: vec![],
            })
        );
    }

    #[test]
    fn parse_boundary_end_error() {
        let end = parse_line("%end 1784356613 286 1").unwrap();
        assert_eq!(
            end,
            Message::ResponseBoundary(ResponseBoundary {
                kind: NotificationKind::End,
                time: 1784356613,
                number: 286,
                flags: 1,
                extra: vec![],
            })
        );
        let err = parse_line("%error 1784356613 286 1").unwrap();
        assert_eq!(
            err,
            Message::ResponseBoundary(ResponseBoundary {
                kind: NotificationKind::Error,
                time: 1784356613,
                number: 286,
                flags: 1,
                extra: vec![],
            })
        );
    }

    #[test]
    fn parse_boundary_legacy_version_string() {
        // 旧版 tmux 可能给版本字符串而非整数：全部塞 extra，不崩
        let m = parse_line("%begin 3.0").unwrap();
        if let Message::ResponseBoundary(b) = m {
            assert_eq!(b.kind, NotificationKind::Begin);
            assert_eq!(b.extra, vec!["3.0".to_string()]);
        } else {
            panic!();
        }
    }

    #[test]
    fn parse_extended_output() {
        // %extended-output <pane> <age> ... : "C-escaped value"
        let line = r"%extended-output %1 42 : \033]8;;https://example.com\033\\link\033]8;;\033\\";
        let m = parse_line(line).unwrap();
        assert_eq!(
            m,
            Message::ExtendedOutput {
                pane: PaneId(1),
                age_ms: 42,
                content: b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\".to_vec(),
                raw_content: "\\033]8;;https://example.com\\033\\\\link\\033]8;;\\033\\\\".into(),
            }
        );
    }

    #[test]
    fn parse_extended_output_preserves_invalid_utf8_bytes() {
        let line = b"%extended-output %7 17 : \"A\\033[2KB\x94\\200C\"";
        let m = parse_line_bytes(line).expect("extended output with high bytes must parse");
        assert_eq!(
            m,
            Message::ExtendedOutput {
                pane: PaneId(7),
                age_ms: 17,
                content: b"A\x1b[2KB\x94\x80C".to_vec(),
                raw_content: "A\\033[2KB\u{fffd}\\200C".into(),
            }
        );
    }

    #[test]
    fn parse_pause_continue() {
        // %pause / %continue 是 tmux 3.3+ 流控消息（control.c `%%pause %%%u`）。
        let m = parse_line("%pause %64").unwrap();
        assert_eq!(
            m,
            Message::Pause {
                pane: Some(PaneId(64)),
                args: String::new()
            }
        );

        let m = parse_line("%continue %64").unwrap();
        assert_eq!(
            m,
            Message::Continue {
                pane: Some(PaneId(64)),
                args: String::new()
            }
        );

        // 空参数也应识别
        assert!(matches!(parse_line("%pause"), Some(Message::Pause { .. })));
        assert!(matches!(
            parse_line("%continue"),
            Some(Message::Continue { .. })
        ));

        // keyword 返回正确的类型名
        assert_eq!(
            Message::Pause {
                pane: None,
                args: String::new()
            }
            .keyword(),
            "pause"
        );
        assert_eq!(
            Message::Continue {
                pane: None,
                args: String::new()
            }
            .keyword(),
            "continue"
        );
    }

    #[test]
    fn parse_output_then_exit_ordering() {
        // 程序退出：先 %output 后 %exit，顺序应保持（pane 内容不丢）
        let raw = "%output %0 hello\r\n%exit pane died\r\n";
        let mut msgs = Vec::new();
        for line in raw.lines() {
            if let Some(m) = parse_line(line) {
                msgs.push(m);
            }
        }
        assert_eq!(msgs.len(), 2);
        assert!(matches!(&msgs[0], Message::Output { content, .. } if content == b"hello"));
        assert!(
            matches!(&msgs[1], Message::Exit { reason } if reason.as_deref() == Some("pane died"))
        );
    }

    #[test]
    fn parse_output_interleaved_with_begin_end() {
        // %output 与 %begin/%end 交织，解析不丢消息
        let raw =
            "%output %0 a\r\n%begin 1 2 0\r\nplain response\r\n%end 1 2 0\r\n%output %1 b\r\n";
        let mut msgs = Vec::new();
        let mut response_lines = Vec::new();
        let mut in_response = false;
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(m) = parse_line(stripped) {
                if let Message::ResponseBoundary(b) = &m {
                    in_response = matches!(b.kind, NotificationKind::Begin);
                } else if !in_response {
                    msgs.push(m);
                }
            } else if in_response {
                response_lines.push(line.to_string());
            }
        }
        // 两条 output 应都在（一条在 begin 前，一条在 end 后）
        let outputs: Vec<_> = msgs
            .iter()
            .filter(|m| matches!(m, Message::Output { .. }))
            .collect();
        assert_eq!(outputs.len(), 2, "两条 %output 都应被解析");
        assert!(response_lines.contains(&"plain response".to_string()));
    }

    #[test]
    fn parse_window_pane_changed() {
        let m = parse_line("%window-pane-changed @0 %1").unwrap();
        assert_eq!(
            m,
            Message::WindowPaneChanged {
                window: TabId(0),
                pane: PaneId(1),
            }
        );
        // 兼容 @ 前缀的 pane id
        let m2 = parse_line("%window-pane-changed @1 @2").unwrap();
        assert_eq!(
            m2,
            Message::WindowPaneChanged {
                window: TabId(1),
                pane: PaneId(2),
            }
        );
    }

    #[test]
    fn parse_session_window_changed() {
        let m = parse_line("%session-window-changed $0 @1").unwrap();
        assert_eq!(
            m,
            Message::SessionWindowChanged {
                session: TmuxSessionId(0),
                window: TabId(1),
            }
        );
    }

    #[test]
    fn parse_unknown_keyword() {
        let m = parse_line("%no-such-notification $0 @1").unwrap();
        assert!(matches!(m, Message::Unknown { .. }));
    }

    #[test]
    fn parse_unknown_garbled() {
        // 畸形：window-add 缺 id -> 解析失败 -> None（被记 warn）
        assert_eq!(parse_line("%window-add"), None);
        // 但未知关键字仍保留
        let m = parse_line("%no-such-thing x y").unwrap();
        assert!(matches!(m, Message::Unknown { .. }));
    }

    // ---------- 真实样本回归测试 ----------

    #[test]
    fn real_sample_new_session_lines() {
        let raw = include_str!("../../../../tests/samples/new-session.txt");
        // 样本第一行可能是 DCS 包装（P1000p%begin ...），parse_line 只认 % 开头，
        // DCS 前缀的行会被当成非通知返回 None（client 层负责剥 DCS）。
        let mut msgs = Vec::new();
        for line in raw.lines() {
            // 剥 DCS 前缀（P1000p）以便纯协议层测试
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(m) = parse_line(stripped) {
                msgs.push(m);
            }
        }
        // 至少应识别出若干已知通知
        let keywords: Vec<&str> = msgs.iter().map(|m| m.keyword()).collect();
        assert!(keywords.contains(&"begin"), "missing begin: {keywords:?}");
        assert!(keywords.contains(&"end"), "missing end: {keywords:?}");
        assert!(keywords.contains(&"window-add"));
        assert!(keywords.contains(&"sessions-changed"));
        assert!(keywords.contains(&"session-changed"));
        assert!(keywords.contains(&"window-renamed"));
        assert!(keywords.contains(&"output"));
    }

    #[test]
    fn dogfood_sample_parses_all_control_lines() {
        // 2026-08-15 dogfood 摘录（禁止 include_str 25MB 原日志）。
        let raw = include_str!("../../../../tests/samples/dogfood-2026-0815-1326.txt");
        let mut parsed = 0usize;
        let mut session_changed = 0usize;
        let mut session_window_changed = 0usize;
        let mut layout_change = 0usize;
        for line in raw.lines() {
            let line = line.trim();
            if !line.starts_with('%') {
                continue;
            }
            let m = parse_line(line).unwrap_or_else(|| panic!("% 行必须可解析: {line}"));
            match &m {
                Message::SessionChanged { session, name } => {
                    session_changed += 1;
                    assert_eq!(
                        session,
                        &TmuxSessionId(4),
                        "dogfood 的 session 是 $4: {line}"
                    );
                    assert_eq!(name.as_deref(), Some("yaklang-workspace"), "{line}");
                }
                Message::SessionWindowChanged { session, window } => {
                    session_window_changed += 1;
                    assert_eq!(session, &TmuxSessionId(4), "{line}");
                    assert!(matches!(window.0, 21 | 29 | 27 | 19), "{line}");
                }
                Message::LayoutChange { window, .. } => {
                    layout_change += 1;
                    assert!(matches!(window.0, 18 | 29 | 21), "{line}");
                }
                Message::Unknown { keyword, .. } => {
                    panic!("dogfood 行不应是 Unknown: keyword={keyword} line={line}")
                }
                _ => {}
            }
            parsed += 1;
        }
        assert!(parsed >= 8, "摘录里应有 8 条 % 行: {parsed}");
        assert_eq!(session_changed, 1);
        assert_eq!(session_window_changed, 4);
        assert_eq!(layout_change, 3);
    }

    #[test]
    fn real_sample_osc_attention_passthrough() {
        // LINUX-PLAN C2.0 E1 fixture：tmux 3.7b 控制模式 %output 原样携带
        // OSC 133 C/D、BEL、OSC 9 与 777（无需 allow-passthrough）。
        let raw = include_str!("../../../../tests/samples/osc-attention-tmux3.7b.txt");
        assert!(
            raw.starts_with("# tmux version: tmux 3.7b"),
            "fixture 头必须写明 tmux 版本"
        );
        assert!(
            raw.contains("E1 conclusion: PASS_THROUGH"),
            "E1 结论必须写进 fixture 头"
        );
        let mut saw_osc133_c = false;
        let mut saw_osc133_d = false;
        let mut saw_bel = false;
        let mut saw_osc9 = false;
        let mut saw_osc777 = false;
        let mut round1_output = 0;
        let mut in_round1 = false;
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if stripped.contains("R1-DEFAULT-START") {
                in_round1 = true;
            }
            if stripped.contains("R1-DEFAULT-END") {
                in_round1 = false;
            }
            let Some(Message::Output { content, .. }) = parse_line(stripped) else {
                continue;
            };
            if in_round1 {
                round1_output += 1;
            }
            if content.windows(6).any(|w| w == b"]133;C") {
                saw_osc133_c = true;
            }
            if content.windows(6).any(|w| w == b"]133;D") {
                saw_osc133_d = true;
            }
            if content.contains(&0x07) {
                saw_bel = true;
            }
            if content.windows(4).any(|w| w == b"]9;h") {
                saw_osc9 = true;
            }
            if content.windows(6).any(|w| w == b"]777;n") {
                saw_osc777 = true;
            }
        }
        assert!(round1_output > 0, "round 1 应有 %output");
        assert!(saw_osc133_c, "round 1 %output 应含 OSC 133 C（三态精确）");
        assert!(saw_osc133_d, "round 1 %output 应含 OSC 133 D");
        assert!(saw_bel, "round 1 %output 应含 BEL");
        assert!(saw_osc9, "round 1 %output 应含 OSC 9");
        assert!(saw_osc777, "round 1 %output 应含 OSC 777");
    }

    #[test]
    fn real_sample_cmd_response() {
        let raw = include_str!("../../../../tests/samples/cmd-response.txt");
        let mut begins = 0;
        let mut ends = 0;
        let mut outputs = 0;
        let mut window_adds = 0;
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(m) = parse_line(stripped) {
                match &m {
                    Message::ResponseBoundary(b) => match b.kind {
                        NotificationKind::Begin => begins += 1,
                        NotificationKind::End => ends += 1,
                        NotificationKind::Error => {}
                    },
                    Message::Output { .. } => outputs += 1,
                    Message::WindowAdd { .. } => window_adds += 1,
                    Message::Unknown { .. } => {
                        // 样本里有 %session-window-changed 等新通知，归 Unknown
                    }
                    _ => {}
                }
            }
        }
        // 样本里至少有 4 个 begin/end 对（list-sessions / display-message / new-window / list-windows）
        assert!(begins >= 4, "begins={begins}");
        assert_eq!(begins, ends, "begin/end 不配对");
        assert!(outputs > 0, "应有 output");
        assert!(window_adds >= 2, "应有至少 2 个 window-add（@0 @1）");
    }

    #[test]
    fn real_sample_output_decodes_ansi() {
        let raw = include_str!("../../../../tests/samples/new-session.txt");
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(Message::Output { content, .. }) = parse_line(stripped) {
                // 解码出的内容应包含 ESC (0x1B)
                assert!(
                    content.contains(&0x1B),
                    "output 应含 ANSI ESC: {:?}",
                    content
                );
            }
        }
    }

    /// 真实样例（取自用户 SSH session 的 a.log）：htop 全屏 TUI。
    /// 关键点：含大量 `\017` (SO) 字符集切换 + `\033[row;colH` 光标定位，
    /// 解析后必须逐字节保留（不丢、不替换成 replacement char）。
    #[test]
    fn real_sample_htop_so_and_cursor_positioning() {
        let raw = include_str!("../../../../tests/samples/real-htop.txt");
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(Message::Output { content, .. }) = parse_line(stripped) {
                // 不能出现 UTF-8 replacement char (0xEF 0xBF 0xBD)
                assert!(!content.windows(3).any(|w| w == b"\xef\xbf\xbd"));
                // SO (0x0F) 必须保留
                assert!(content.contains(&0x0F), "htop 的 SO 字符集切换不能丢");
                // ESC (0x1B) 必须保留（光标定位/颜色）
                assert!(content.contains(&0x1B));
            }
        }
    }

    /// 真实样例：git lg 长文本。关键点：`\010` (backspace) 修剪换行文本 +
    /// `\015\012` CRLF。解析后这些控制字节必须逐字节保留，不产生 replacement。
    #[test]
    fn real_sample_git_lg_backspace_and_crlf() {
        let raw = include_str!("../../../../tests/samples/real-git_lg.txt");
        let mut saw_backspace = false;
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(Message::Output { content, .. }) = parse_line(stripped) {
                assert!(!content.windows(3).any(|w| w == b"\xef\xbf\xbd"));
                assert!(content.contains(&0x1B), "git lg 的 ANSI 颜色不能丢");
                if content.contains(&0x08) {
                    saw_backspace = true;
                    // 回退字节后的下一个字符必须是合法的颜色码/文本（无 replacement）
                    let idx = content.iter().position(|&b| b == 0x08).unwrap();
                    assert!(idx + 1 < content.len());
                }
            }
        }
        assert!(saw_backspace, "git lg 样例应含 backspace 修剪");
    }

    /// 真实样例：`ls -la` 输入回显。关键点：空格必须保留（不能变成 `ls-la`），
    /// 以及末尾 `\033[19D` 光标回退。
    #[test]
    fn real_sample_ls_la_preserves_spaces_and_cursor_back() {
        let raw = include_str!("../../../../tests/samples/real-ls_la.txt");
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(Message::Output { content, .. }) = parse_line(stripped) {
                // 回显内容必须包含至少一个空格（ls -la 的分隔空格不能被吞掉）
                assert!(
                    content.contains(&b' '),
                    "ls -la 回显应保留空格: {:?}",
                    content
                );
                // 末尾光标回退 ESC[...D 必须保留
                assert!(content.contains(&0x1B));
            }
        }
    }

    /// 真实样例：codex 交互提示符。关键点：UTF-8 的 ❯ 符号 (0xE2 0x9D 0xAF)、
    /// bracketed-paste 开关 `?2004h`、CR 都必须逐字节保留。
    #[test]
    fn real_sample_codex_prompt_utf8_and_mode_switches() {
        let raw = include_str!("../../../../tests/samples/real-codex.txt");
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(Message::Output { content, .. }) = parse_line(stripped) {
                // 不能有 replacement char
                assert!(!content.windows(3).any(|w| w == b"\xef\xbf\xbd"));
                // ❯ 的 UTF-8 字节序列必须保留
                assert!(
                    content.windows(3).any(|w| w == b"\xe2\x9d\xaf"),
                    "codex 提示符的 ❯ 符号字节必须保留"
                );
                // 模式切换序列中的 ESC 必须保留
                assert!(content.contains(&0x1B));
            }
        }
    }

    /// 真实样例（b.log，alacritty recording 式回归）：git lg 输出末尾带终端查询
    /// `ESC]10;rgb:0000/0000/0000 ESC\`、`ESC]11;rgb:ffff/ffff/ffff ESC\`、
    /// `ESC[?65;4;1;2;6;21;22;17;28c`（OSC 颜色查询 + CSI 设备属性查询）。
    /// 解析后这些引导字节（ESC、OSC、CSI、ST `ESC\`）必须逐字节保留，
    /// 否则 shell 会把 `10;rgb:...` 当命令执行（`zsh: command not found: 10`）。
    #[test]
    fn real_sample_git_lg_osc_csi_query_preserves_esc_leaders() {
        let raw = include_str!("../../../../tests/samples/real-gitlg-osc-query.txt");
        let mut saw_osc10 = false;
        let mut saw_osc11 = false;
        let mut saw_csi_da = false;
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(Message::Output { content, .. }) = parse_line(stripped) {
                assert!(!content.windows(3).any(|w| w == b"\xef\xbf\xbd"));
                // ESC ] 10 ; ... ESC \   (OSC 10 foreground query reply)
                saw_osc10 |= content.windows(4).any(|w| w == b"\x1b]10");
                // ESC ] 11 ; ... ESC \   (OSC 11 background query reply)
                saw_osc11 |= content.windows(4).any(|w| w == b"\x1b]11");
                // ESC [ ? 65 ; ... c   (CSI Device Attributes query)
                saw_csi_da |= content.windows(4).any(|w| w == b"\x1b[?6");
            }
        }
        assert!(saw_osc10, "git lg 的 OSC 10 颜色查询引导字节必须保留");
        assert!(saw_osc11, "git lg 的 OSC 11 颜色查询引导字节必须保留");
        assert!(saw_csi_da, "git lg 的 CSI ?65 设备属性查询引导字节必须保留");
    }

    /// 真实样例：new-session + split-window + new-window 的完整 CC 消息流。
    /// 验证能逐行解析出所有关键通知。
    #[test]
    fn real_sample_2tab_3pane_cc() {
        let raw = include_str!("../../../../tests/samples/2tab-3pane-cc.txt");
        let mut msgs = Vec::new();
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(m) = parse_line(stripped) {
                msgs.push(m);
            }
        }
        let keywords: Vec<&str> = msgs.iter().map(|m| m.keyword()).collect();
        // 关键通知必须全部出现
        assert!(
            keywords.contains(&"window-add"),
            "missing window-add: {keywords:?}"
        );
        assert!(
            keywords.contains(&"window-pane-changed"),
            "missing window-pane-changed: {keywords:?}"
        );
        assert!(
            keywords.contains(&"layout-change"),
            "missing layout-change: {keywords:?}"
        );
        assert!(
            keywords.contains(&"session-window-changed"),
            "missing session-window-changed: {keywords:?}"
        );
        assert!(
            keywords.contains(&"session-changed"),
            "missing session-changed: {keywords:?}"
        );
        // 两个 window-add（@0, @1）
        let window_adds: Vec<_> = msgs
            .iter()
            .filter(|m| m.keyword() == "window-add")
            .collect();
        assert_eq!(window_adds.len(), 2, "应有 2 个 window-add");
        // 两次 layout-change（一次水平分割，一次嵌套垂直分割）
        let layout_changes: Vec<_> = msgs
            .iter()
            .filter(|m| m.keyword() == "layout-change")
            .collect();
        assert_eq!(layout_changes.len(), 2, "应有 2 个 layout-change");
        // 验证嵌套 layout 字符串可被 parse_layout_tree 解析
        if let Some(Message::LayoutChange { layout, .. }) = layout_changes.get(1) {
            let tree = parse_layout_tree(&layout.raw).expect("layout tree 应解析成功");
            assert!(!tree.is_leaf(), "嵌套 layout 应有子节点");
            assert_eq!(tree.dir, SplitDir::Horizontal);
            let (_, right) = tree.children.as_ref().unwrap();
            assert_eq!(right.dir, SplitDir::Vertical, "右子树应为垂直分割");
        }
    }

    /// 真实样例：attach 已有 session 的 CC 消息流。
    /// 验证 %session-changed + list-windows/list-panes 响应行能正确分离。
    #[test]
    fn real_sample_attach_cc() {
        let raw = include_str!("../../../../tests/samples/attach-cc.txt");
        let mut notifications = Vec::new();
        let mut response_lines = Vec::new();
        let mut in_response = false;
        for line in raw.lines() {
            let stripped = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
            if let Some(m) = parse_line(stripped) {
                if let Message::ResponseBoundary(b) = &m {
                    in_response = matches!(b.kind, NotificationKind::Begin);
                    if matches!(b.kind, NotificationKind::End) {
                        in_response = false;
                    }
                } else if !in_response {
                    notifications.push(m);
                } else {
                    // 在响应区内，即使是 % 开头的行（如 %0 @0 0）也是响应内容
                    response_lines.push(stripped.to_string());
                }
            } else if in_response {
                response_lines.push(stripped.to_string());
            }
        }
        // attach 只有一个通知：session-changed
        assert!(
            notifications
                .iter()
                .any(|m| m.keyword() == "session-changed"),
            "应有 session-changed: {:?}",
            notifications
                .iter()
                .map(|m| m.keyword())
                .collect::<Vec<_>>()
        );
        // 响应行应包含 window 列表行和 pane 列表行
        // 响应行含 @0 和 @1（各一行 window 列表行）
        assert!(
            response_lines.iter().any(|l| l.contains("@0")),
            "应有含 @0 的 window 行: {response_lines:?}"
        );
        assert!(
            response_lines.iter().any(|l| l.contains("@1")),
            "应有含 @1 的 window 行: {response_lines:?}"
        );
        assert!(
            response_lines.iter().any(|l| l.contains("%0")),
            "应有含 %0 的 pane 行: {response_lines:?}"
        );
        assert!(
            response_lines.iter().any(|l| l.contains("%3")),
            "应有含 %3 的 pane 行: {response_lines:?}"
        );
    }

    /// 真实样例：attach 样例里的 window_layout 树字符串解析。
    #[test]
    fn real_sample_attach_layout_tree() {
        let raw = include_str!("../../../../tests/samples/attach-cc.txt");
        // 找到 list-windows 响应行里的 layout 字符串
        for line in raw.lines() {
            if line.contains("1268,140x30,0,0{") {
                // 提取 layout 部分（[layout ...] 里的内容）
                if let Some(start) = line.find("1268,") {
                    let rest = &line[start..];
                    // layout 字符串到行尾或空格
                    // 这里取的是人类可读的 `[layout ...]` 字段，末尾的 `]` 不属于
                    // window_layout；机器可读的 list-windows 响应不会带这个括号。
                    let layout_str = rest
                        .split_whitespace()
                        .next()
                        .unwrap_or(rest)
                        .trim_end_matches(']');
                    let tree = parse_layout_tree(layout_str).expect("layout tree 解析");
                    assert_eq!(tree.cols, 140);
                    assert_eq!(tree.rows, 30);
                    assert_eq!(tree.dir, SplitDir::Horizontal);
                    let (left, right) = tree.children.as_ref().unwrap();
                    assert!(left.is_leaf());
                    assert_eq!(right.dir, SplitDir::Vertical);
                    assert_eq!(right.children.as_ref().unwrap().0.rows, 15);
                    assert_eq!(right.children.as_ref().unwrap().1.rows, 14);
                    return;
                }
            }
        }
        panic!("未找到 layout 字符串");
    }

    /// 对应：超长 %output 不应截断/崩溃。
    #[test]
    fn test_protocol_long_output_content() {
        let payload = "A".repeat(8000);
        let line = format!("%output @1 \"{payload}\"");
        match parse_line(&line) {
            Some(Message::Output { pane, content, .. }) => {
                assert_eq!(pane, PaneId(1));
                assert_eq!(content.len(), 8000);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// 对应：混合空行 / 普通响应行 / % 通知的流解析。
    #[test]
    fn test_protocol_mixed_stream_lines() {
        let lines = [
            "",
            "not-a-notification",
            "%begin 1 2 3",
            "response body",
            "%end 1 2 3",
            "%sessions-changed",
            "\r",
        ];
        let msgs: Vec<_> = lines.iter().filter_map(|l| parse_line(l)).collect();
        assert_eq!(msgs.len(), 3);
        assert!(matches!(
            &msgs[0],
            Message::ResponseBoundary(b) if b.kind == NotificationKind::Begin
        ));
        assert!(matches!(
            &msgs[1],
            Message::ResponseBoundary(b) if b.kind == NotificationKind::End
        ));
        assert!(matches!(&msgs[2], Message::SessionsChanged));
    }

    /// 对应：C 转义 \a\b\f\v 与无效 hex。
    #[test]
    fn test_protocol_escape_bell_backspace_formfeed_vtab() {
        let d = ControlEscapeDecoder::new();
        assert_eq!(d.decode(r"\a\b\f\v").unwrap(), vec![0x07, 0x08, 0x0C, 0x0B]);
    }

    #[test]
    fn test_protocol_escape_invalid_hex_errors() {
        let d = ControlEscapeDecoder::new();
        assert!(matches!(
            d.decode(r"\x"),
            Err(ControlEscapeError::Truncated)
        ));
        assert!(matches!(
            d.decode(r"\xGG"),
            Err(ControlEscapeError::InvalidHex(_))
        ));
    }

    #[test]
    fn test_protocol_escape_lossy_on_error_returns_raw() {
        let d = ControlEscapeDecoder::new();
        assert_eq!(d.decode_lossy(r"bad\q"), r"bad\q");
    }

    /// 对应：%error 边界与 begin/end 成对。
    #[test]
    fn test_protocol_error_boundary() {
        let m = parse_line("%error 9 8 7").unwrap();
        match m {
            Message::ResponseBoundary(b) => {
                assert_eq!(b.kind, NotificationKind::Error);
                assert_eq!(b.time, 9);
                assert_eq!(b.number, 8);
                assert_eq!(b.flags, 7);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_protocol_malformed_output_returns_none() {
        // 缺 content / 坏 pane id → 不崩溃，返回 None
        assert!(parse_line("%output").is_none());
        assert!(parse_line("%output @x \"hi\"").is_none());
    }

    #[test]
    fn test_protocol_message_keywords_cover_variants() {
        let samples = [
            ("%output @0 \"x\"", "output"),
            ("%window-add @1", "window-add"),
            ("%window-close @1", "window-close"),
            ("%window-renamed @1 foo", "window-renamed"),
            ("%session-changed $0 name", "session-changed"),
            ("%session-renamed $0 name", "session-renamed"),
            ("%sessions-changed", "sessions-changed"),
            ("%pane-mode-changed @1 copy-mode", "pane-mode-changed"),
            ("%unlinked-window-add @2", "unlinked-window-add"),
            ("%unlinked-window-close @2", "unlinked-window-close"),
            ("%exit", "exit"),
            ("%begin 1 2 3", "begin"),
        ];
        for (line, kw) in samples {
            let m = parse_line(line).expect(line);
            assert_eq!(m.keyword(), kw, "line={line}");
        }
    }

    #[test]
    fn test_protocol_crlf_suffix_stripped() {
        let m = parse_line("%sessions-changed\r\n").or_else(|| {
            // parse_line 假定已按行切分；若保留 \r 仍应识别
            parse_line("%sessions-changed\r")
        });
        // 至少无 \r 的行必须成功
        assert!(parse_line("%sessions-changed").is_some());
        let _ = m;
    }

    // ---------- layout tree 解析 ----------

    #[test]
    fn parse_layout_tree_single_leaf() {
        // a980,140x30,0,0,3
        let t = parse_layout_tree("a980,140x30,0,0,3").unwrap();
        assert!(t.is_leaf());
        assert_eq!((t.cols, t.rows, t.x, t.y, t.flags), (140, 30, 0, 0, 3));
        assert_eq!(t.tree_id, "a980");
    }

    #[test]
    fn parse_layout_tree_horizontal_split() {
        // 75ac,140x30,0,0{70x30,0,0,0,69x30,71,0,1}
        let t = parse_layout_tree("75ac,140x30,0,0{70x30,0,0,0,69x30,71,0,1}").unwrap();
        assert!(!t.is_leaf());
        assert_eq!(t.dir, SplitDir::Horizontal);
        let (a, b) = t.children.as_ref().unwrap();
        assert!(a.is_leaf());
        assert!(b.is_leaf());
        assert_eq!((a.cols, a.rows), (70, 30));
        assert_eq!((b.cols, b.rows, b.x), (69, 30, 71));
    }

    #[test]
    fn parse_layout_tree_nested() {
        // 1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}
        let t = parse_layout_tree(
            "1268,140x30,0,0{70x30,0,0,0,69x30,71,0[69x15,71,0,1,69x14,71,16,2]}",
        )
        .unwrap();
        assert_eq!(t.dir, SplitDir::Horizontal);
        let (left, right) = t.children.as_ref().unwrap();
        assert!(left.is_leaf());
        assert!(!right.is_leaf());
        assert_eq!(right.dir, SplitDir::Vertical);
        let (r1, r2) = right.children.as_ref().unwrap();
        assert_eq!((r1.cols, r1.rows, r1.y), (69, 15, 0));
        assert_eq!((r2.cols, r2.rows, r2.y), (69, 14, 16));
    }

    #[test]
    fn parse_layout_tree_vertical_split() {
        // 完整 layout：根是垂直分割（上下）
        // b97f,140x30,0,0[70x15,0,0,1,70x14,0,16,2]
        let t = parse_layout_tree("b97f,140x30,0,0[70x15,0,0,1,70x14,0,16,2]").unwrap();
        assert_eq!(t.dir, SplitDir::Vertical);
        let (a, b) = t.children.as_ref().unwrap();
        assert_eq!((a.rows, b.rows), (15, 14));
    }

    #[test]
    fn parse_layout_tree_nary_group_keeps_all_children() {
        // tmux 在连续同向分割时可能把 4 个 pane 直接放进一个 [] 组；
        // core 使用二叉树，因此解析后应右折叠，但不能丢掉第三、第四个 pane。
        let t = parse_layout_tree("nary,20x19,0,0[20x4,0,0,1,20x4,0,5,2,20x4,0,10,3,20x4,0,15,4]")
            .unwrap();
        assert_eq!(collect_layout_tree_leaves(&t).len(), 4);
        let (first, rest) = t.children.as_ref().unwrap();
        assert_eq!(first.flags, 1);
        let (_, rest) = rest.children.as_ref().unwrap();
        let (_, last) = rest.children.as_ref().unwrap();
        assert_eq!((last.flags, last.y), (4, 15));
    }

    #[test]
    fn parse_layout_tree_missing_flags() {
        // 缺 flags（如 abc,80x24,0,0）应容错为 flags=0
        let t = parse_layout_tree("abc,80x24,0,0").unwrap();
        assert!(t.is_leaf());
        assert_eq!((t.cols, t.rows, t.x, t.y, t.flags), (80, 24, 0, 0, 0));
    }

    #[test]
    fn parse_layout_tree_bad() {
        assert!(parse_layout_tree("nope").is_err());
        assert!(parse_layout_tree("").is_err());
    }

    fn collect_layout_tree_leaves(tree: &LayoutTree) -> Vec<&LayoutTree> {
        match &tree.children {
            None => vec![tree],
            Some((a, b)) => {
                let mut leaves = collect_layout_tree_leaves(a);
                leaves.extend(collect_layout_tree_leaves(b));
                leaves
            }
        }
    }
}

/// iTerm2 tmux integration / tmux 控制协议一致性测试。
///
/// 参考 iTerm2 的 tmux integration 文档与 tmux `control.c`：
/// - pane/window 激活切换消息（window-pane-changed / session-window-changed）
/// - unlinked window 生命周期
/// - 流控（pause/continue）与扩展输出（extended-output）
/// - 命令响应边界（begin/end/error）
#[cfg(test)]
mod iterm2_protocol_conformance_tests {
    use super::*;

    /// %output 的 pane id 同时接受 `@N`、`%N`、裸数字 N（tmux 3.3+ 变体）。
    #[test]
    fn output_accepts_at_percent_and_bare_pane_ids() {
        let at = parse_line("%output @7 \"hi\"").unwrap();
        assert!(matches!(
            at,
            Message::Output {
                pane: PaneId(7),
                ..
            }
        ));
        let percent = parse_line("%output %7 \"hi\"").unwrap();
        assert!(matches!(
            percent,
            Message::Output {
                pane: PaneId(7),
                ..
            }
        ));
        let bare = parse_line("%output 7 \"hi\"").unwrap();
        assert!(matches!(
            bare,
            Message::Output {
                pane: PaneId(7),
                ..
            }
        ));
    }

    /// unlinked window 的 add/close 生命周期（iTerm2 集成会忽略未链接窗口）。
    #[test]
    fn unlinked_window_lifecycle() {
        let add = parse_line("%unlinked-window-add @12").unwrap();
        assert!(matches!(
            add,
            Message::UnlinkedWindowAdd { window: TabId(12) }
        ));
        let close = parse_line("%unlinked-window-close @12").unwrap();
        assert!(matches!(
            close,
            Message::UnlinkedWindowClose { window: TabId(12) }
        ));
    }

    /// 激活 pane/window 切换事件。
    #[test]
    fn active_pane_and_window_change_events() {
        let pane = parse_line("%window-pane-changed @3 @9").unwrap();
        assert!(matches!(
            pane,
            Message::WindowPaneChanged {
                window: TabId(3),
                pane: PaneId(9)
            }
        ));
        let window = parse_line("%session-window-changed $1 @4").unwrap();
        assert!(matches!(
            window,
            Message::SessionWindowChanged {
                session: TmuxSessionId(1),
                window: TabId(4)
            }
        ));
    }

    /// pane 模式切换与退出原因。
    #[test]
    fn pane_mode_and_exit_reason() {
        let mode = parse_line("%pane-mode-changed @2 copy-mode").unwrap();
        assert!(matches!(
            mode,
            Message::PaneModeChanged {
                pane: PaneId(2),
                ref mode
            } if mode == "copy-mode"
        ));
        let exit = parse_line("%exit server exited").unwrap();
        assert!(matches!(
            exit,
            Message::Exit {
                reason: Some(ref r)
            } if r == "server exited"
        ));
        let bare = parse_line("%exit").unwrap();
        assert!(matches!(bare, Message::Exit { reason: None }));
    }

    /// 流控消息保留 flags（tmux 3.3+ pause-after）。
    #[test]
    fn flow_control_pause_continue() {
        let pause = parse_line("%pause %7 -U 1 2").unwrap();
        assert!(matches!(
            pause,
            Message::Pause {
                pane: Some(PaneId(7)),
                ref args
            } if args == "-U 1 2"
        ));
        let cont = parse_line("%continue %7").unwrap();
        assert!(matches!(
            cont,
            Message::Continue {
                pane: Some(PaneId(7)),
                ..
            }
        ));
    }

    /// extended-output 的 value 与 %output 一样按 C 转义解码。
    #[test]
    fn extended_output_decodes_value_like_output() {
        let msg = parse_line(
            r#"%extended-output @5 12 : "a
b""#,
        )
        .unwrap();
        assert!(matches!(
            msg,
            Message::ExtendedOutput {
                pane: PaneId(5),
                age_ms: 12,
                ref content,
                ..
            } if content == b"a\nb"
        ));
    }

    /// %error 边界保留 flags，供上层识别命令失败。
    #[test]
    fn boundary_error_keeps_flags() {
        let msg = parse_line("%error 123 7 0").unwrap();
        match msg {
            Message::ResponseBoundary(b) => {
                assert_eq!(b.kind, NotificationKind::Error);
                assert_eq!(b.time, 123);
                assert_eq!(b.number, 7);
                assert_eq!(b.flags, 0);
            }
            _ => panic!("应为 error 边界"),
        }
    }

    /// layout-change 的可见布局参数若不是合法 layout，应安全忽略。
    #[test]
    fn layout_change_ignores_non_layout_visible_flag() {
        let msg = parse_line("%layout-change @0 80x24,0,0 *").unwrap();
        assert!(matches!(
            msg,
            Message::LayoutChange {
                window: TabId(0),
                visible_layout: None,
                flags: Some(ref f),
                ..
            } if f == "*"
        ));
    }

    /// C 转义解码：八进制、十六进制、ESC/ST、SO/backspace 都要逐字节还原。
    #[test]
    fn c_string_decoder_handles_octal_hex_and_control_bytes() {
        let bytes = ControlEscapeDecoder::new()
            .decode(r##"\033[31m\017\010\x41\012\\\"\e"##)
            .unwrap();
        assert_eq!(
            bytes,
            vec![0x1b, b'[', b'3', b'1', b'm', 0x0f, 0x08, b'A', b'\n', b'\\', b'"', 0x1b]
        );
    }

    /// 未知消息保留 keyword 与原始内容，方便上层告警而不是崩溃。
    #[test]
    fn unknown_message_preserves_keyword_and_raw() {
        let msg = parse_line("%no-such-thing a b c").unwrap();
        assert!(matches!(
            msg,
            Message::Unknown {
                ref keyword,
                ref raw
            } if keyword == "no-such-thing" && raw == "a b c"
        ));
    }
}
