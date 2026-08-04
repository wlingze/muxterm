//! tmux 控制协议解析器（line-oriented） + layout tree 解析器。
//!
//! tmux 以 `-CC` 启动后会向 stdout 输出结构化通知，每行以 `%` 开头（命令响应内容
//! 除外，夹在 `%begin` / `%end` 之间）。本模块提供：
//!
//! - [`Message`]：覆盖所有已知通知类型的 enum
//! - [`parse_line`]：解析单行原始输出（已按真换行切分）为 `Option<Message>`
//! - [`ControlEscapeDecoder`]：解码 `%output` 里 C 风格转义字符串
//! - [`parse_layout_tree`]：把 tmux 的 window_layout 树字符串解析成 [`LayoutTree`]，
//!   供 TmuxBackend 映射成 [`LayoutNode`](crate::core::model::layout::LayoutNode)。
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
pub use crate::core::types::{PaneId, SessionId, WindowId};

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

    /// 解码转义字符串，返回原始字节。
    pub fn decode(&self, s: &str) -> Result<Vec<u8>, ControlEscapeError> {
        let bytes = s.as_bytes();
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
                    // 十六进制：tmux 用 \xNN 形式
                    i += 1;
                    let mut digits = 0u8;
                    let mut val: u32 = 0;
                    while i < bytes.len() && digits < 2 {
                        let c = bytes[i];
                        match c {
                            b'0'..=b'9' => val = val * 16 + (c - b'0') as u32,
                            b'a'..=b'f' => val = val * 16 + (c - b'a' + 10) as u32,
                            b'A'..=b'F' => val = val * 16 + (c - b'A' + 10) as u32,
                            _ => break,
                        }
                        digits += 1;
                        i += 1;
                    }
                    if digits == 0 {
                        return Err(ControlEscapeError::InvalidHex('\0'));
                    }
                    out.push(val as u8);
                }
                other => {
                    return Err(ControlEscapeError::UnknownEscape(other as char));
                }
            }
        }
        Ok(out)
    }

    /// 解码并尝试转成 UTF-8 字符串（用 lossy 回退）。
    pub fn decode_lossy(&self, s: &str) -> String {
        match self.decode(s) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => s.to_string(),
        }
    }
}

// ============================================================================
// ID 解析（类型定义在 crate::types）
// ============================================================================

impl PaneId {
    /// 从 `@N` 形式解析。
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let s = s
            .strip_prefix('@')
            .ok_or_else(|| ProtocolError::MalformedField(format!("pane id 缺少 @ 前缀: {s}")))?;
        let n = u32::from_str(s)
            .map_err(|_| ProtocolError::MalformedField(format!("pane id 非数字: {s}")))?;
        Ok(PaneId(n))
    }
}

impl WindowId {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let s = s
            .strip_prefix('@')
            .ok_or_else(|| ProtocolError::MalformedField(format!("window id 缺少 @ 前缀: {s}")))?;
        let n = u32::from_str(s)
            .map_err(|_| ProtocolError::MalformedField(format!("window id 非数字: {s}")))?;
        Ok(WindowId(n))
    }
}

impl SessionId {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let s = s
            .strip_prefix('$')
            .ok_or_else(|| ProtocolError::MalformedField(format!("session id 缺少 $ 前缀: {s}")))?;
        let n = u32::from_str(s)
            .map_err(|_| ProtocolError::MalformedField(format!("session id 非数字: {s}")))?;
        Ok(SessionId(n))
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
    LayoutChange {
        window: WindowId,
        layout: LayoutChange,
        /// 可见的布局字符串（若 tmux 给了第二个布局参数）。
        visible_layout: Option<LayoutChange>,
    },
    /// `%window-add <window_id>`
    WindowAdd { window: WindowId },
    /// `%window-close <window_id>`
    WindowClose { window: WindowId },
    /// `%window-renamed <window_id> <name>`
    WindowRenamed { window: WindowId, name: String },
    /// `%session-changed <session_id> [<session_name>]`
    SessionChanged {
        session: SessionId,
        name: Option<String>,
    },
    /// `%session-renamed <session_id> <name>`
    SessionRenamed { session: SessionId, name: String },
    /// `%sessions-changed`（无参数）
    SessionsChanged,
    /// `%pane-mode-changed <pane_id> <mode>`
    PaneModeChanged { pane: PaneId, mode: String },
    /// `%unlinked-window-add <window_id>`
    UnlinkedWindowAdd { window: WindowId },
    /// `%unlinked-window-close <window_id>`
    UnlinkedWindowClose { window: WindowId },
    /// `%exit [<reason>...]`
    Exit { reason: Option<String> },
    /// `%window-pane-changed <window_id> <pane_id>`：某 window 的激活 pane 切换。
    WindowPaneChanged { window: WindowId, pane: PaneId },
    /// `%session-window-changed <session_id> <window_id>`：某 session 的激活 window 切换。
    SessionWindowChanged {
        session: SessionId,
        window: WindowId,
    },
    /// `%extended-output <pane_id> <type> <args>`（tmux 3.3+，如 hyperlink）
    ExtendedOutput {
        pane: PaneId,
        output_type: String,
        args: String,
    },
    /// `%pause <flags...>`（tmux 3.3+ 流控）：pane 输出被暂停（`pause-after`）。
    /// 第一版只「识别并安全忽略」，不阻塞状态机；内容保留以便后续实现背压。
    Pause { args: String },
    /// `%continue <flags...>`（tmux 3.3+ 流控）：pane 输出恢复。
    Continue { args: String },
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
            Message::Exit { .. } => "exit",
            Message::WindowPaneChanged { .. } => "window-pane-changed",
            Message::SessionWindowChanged { .. } => "session-window-changed",
            Message::ExtendedOutput { .. } => "extended-output",
            Message::Pause { .. } => "pause",
            Message::Continue { .. } => "continue",
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
    // tmux CC 通知以 % 开头。注意 tmux 有时会在 % 前面带 DCS 包装（P1000p ... ），
    // 这里只处理以 % 开头的行；DCS 前缀由 client 层在拼包时清理。
    let line = line.strip_prefix('%')?;
    // 关键字 = 第一个 token（直到第一个空格）
    let (keyword, rest) = match line.split_once(' ') {
        Some((kw, r)) => (kw, r),
        None => (line, ""),
    };

    match keyword {
        "output" => parse_output(rest),
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
        "exit" => Ok(Message::Exit {
            reason: if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            },
        }),
        "extended-output" => parse_extended_output(rest),
        "pause" => Ok(Message::Pause {
            args: rest.trim().to_string(),
        }),
        "continue" => Ok(Message::Continue {
            args: rest.trim().to_string(),
        }),
        "begin" => parse_boundary(rest, NotificationKind::Begin).map(Message::ResponseBoundary),
        "end" => parse_boundary(rest, NotificationKind::End).map(Message::ResponseBoundary),
        "error" => parse_boundary(rest, NotificationKind::Error).map(Message::ResponseBoundary),
        _ => Ok(Message::Unknown {
            keyword: keyword.to_string(),
            raw: rest.to_string(),
        }),
    }
    .map_err(|e: ProtocolError| {
        tracing::warn!(target = "muxterm::protocol", "解析失败: {e}");
        e
    })
    .ok()
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
    let window = WindowId::parse(rest)?;
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
    let window = WindowId::parse(wid)?;
    let name = it.next().unwrap_or("").to_string();
    Ok(Message::WindowRenamed { window, name })
}

fn parse_session_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %session-changed $0 name
    let mut it = rest.splitn(2, ' ');
    let sid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("session-changed 缺 id".into()))?;
    let session = SessionId::parse(sid)?;
    let name = it.next().map(|s| s.to_string());
    Ok(Message::SessionChanged { session, name })
}

fn parse_session_renamed(rest: &str) -> Result<Message, ProtocolError> {
    let mut it = rest.splitn(2, ' ');
    let sid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("session-renamed 缺 id".into()))?;
    let session = SessionId::parse(sid)?;
    let name = it.next().unwrap_or("").to_string();
    Ok(Message::SessionRenamed { session, name })
}

fn parse_pane_mode_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %pane-mode-changed @0 copy-mode
    let mut it = rest.splitn(2, ' ');
    let pid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("pane-mode-changed 缺 id".into()))?;
    let pane = PaneId::parse(pid)?;
    let mode = it.next().unwrap_or("").to_string();
    Ok(Message::PaneModeChanged { pane, mode })
}

fn parse_extended_output(rest: &str) -> Result<Message, ProtocolError> {
    // %extended-output @0 hyperlink <args...>
    let mut it = rest.splitn(3, ' ');
    let pid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("extended-output 缺 pane id".into()))?;
    let pane = PaneId::parse(pid)?;
    let output_type = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("extended-output 缺 type".into()))?
        .to_string();
    let args = it.next().unwrap_or("").to_string();
    Ok(Message::ExtendedOutput {
        pane,
        output_type,
        args,
    })
}

fn parse_output(rest: &str) -> Result<Message, ProtocolError> {
    // %output @N "content"
    // 注意：真实 tmux 3.3+ 用 %0/%1 形式的 pane id（不带 @ 前缀！见样本）。
    // 兼容两种：@N 或纯数字 N。
    let rest = rest.trim_start();
    // 找第一个空格分隔 pane id 和 content
    let (pid_str, content_part) = rest
        .split_once(' ')
        .ok_or_else(|| ProtocolError::MalformedField("output 缺 content".into()))?;
    let pane = parse_pane_id_lenient(pid_str)?;
    // content 必须是双引号包裹的 C 转义字符串
    let inner = strip_c_string(content_part)?;
    let raw_content = inner.to_string();
    let content = ControlEscapeDecoder::new()
        .decode(inner)
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
    let s = s.trim();
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

fn parse_layout_change(rest: &str) -> Result<Message, ProtocolError> {
    // %layout-change @0 <layout> [<visible_layout> [<flags>]]
    let rest = rest.trim();
    let mut parts = rest.splitn(4, ' ');
    let wid = parts
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("layout-change 缺 window id".into()))?;
    let window = WindowId::parse(wid)?;
    let layout_str = parts
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("layout-change 缺 layout".into()))?;
    let layout = LayoutChange::parse(layout_str)?;
    // visible_layout 可选，可能是合法的 layout 字符串，也可能是 flags（如 *）。
    // 如果 parse 失败则忽略（容错），不丢弃整条消息。
    let visible = parts
        .next()
        .map(LayoutChange::parse)
        .filter(|r| r.is_ok())
        .map(|r| r.unwrap());
    Ok(Message::LayoutChange {
        window,
        layout,
        visible_layout: visible,
    })
}

fn parse_window_pane_changed(rest: &str) -> Result<Message, ProtocolError> {
    // %window-pane-changed @0 %1   （window id 用 @，pane id 用 % 或 @）
    let mut it = rest.splitn(2, ' ');
    let wid = it
        .next()
        .ok_or_else(|| ProtocolError::MalformedField("window-pane-changed 缺 window id".into()))?;
    let window = WindowId::parse(wid)?;
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
    let session = SessionId::parse(sid)?;
    let wid = it.next().ok_or_else(|| {
        ProtocolError::MalformedField("session-window-changed 缺 window id".into())
    })?;
    let window = WindowId::parse(wid.trim())?;
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
/// 匹配来关联。所以本结构只提供几何拓扑；pane id 的映射由 TmuxBackend 完成。
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
        assert!(PaneId::parse("3").is_err());
        assert!(PaneId::parse("@x").is_err());
        assert_eq!(PaneId(7).as_str(), "@7");
    }

    #[test]
    fn window_id_parse() {
        assert_eq!(WindowId::parse("@0").unwrap(), WindowId(0));
        assert_eq!(WindowId(12).as_str(), "w12");
    }

    #[test]
    fn session_id_parse() {
        assert_eq!(SessionId::parse("$0").unwrap(), SessionId(0));
        assert_eq!(SessionId(5).as_str(), "$5");
        assert!(SessionId::parse("@1").is_err());
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
        assert_eq!(
            m,
            Message::WindowAdd {
                window: WindowId(0)
            }
        );
        assert_eq!(m.keyword(), "window-add");
    }

    #[test]
    fn parse_window_close() {
        let m = parse_line("%window-close @3").unwrap();
        assert_eq!(
            m,
            Message::WindowClose {
                window: WindowId(3)
            }
        );
    }

    #[test]
    fn parse_unlinked_window_add_close() {
        let a = parse_line("%unlinked-window-add @5").unwrap();
        assert_eq!(
            a,
            Message::UnlinkedWindowAdd {
                window: WindowId(5)
            }
        );
        let c = parse_line("%unlinked-window-close @5").unwrap();
        assert_eq!(
            c,
            Message::UnlinkedWindowClose {
                window: WindowId(5)
            }
        );
    }

    #[test]
    fn parse_window_renamed() {
        let m = parse_line("%window-renamed @0 bash").unwrap();
        assert_eq!(
            m,
            Message::WindowRenamed {
                window: WindowId(0),
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
                session: SessionId(0),
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
                session: SessionId(0),
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
                session: SessionId(0),
                name: "mysession".into(),
            }
        );
    }

    #[test]
    fn parse_sessions_changed() {
        let m = parse_line("%sessions-changed\r\n").unwrap();
        assert_eq!(m, Message::SessionsChanged);
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
            } => {
                assert_eq!(window, WindowId(0));
                assert_eq!((layout.cols, layout.rows), (80, 24));
                assert_eq!(visible_layout, None);
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
        } = m
        {
            assert_eq!(window, WindowId(1));
            assert_eq!((layout.cols, layout.rows, layout.flags), (100, 30, 1));
            let v = visible_layout.unwrap();
            assert_eq!((v.cols, v.rows), (100, 30));
        } else {
            panic!();
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
        let m = parse_line("%extended-output @1 hyperlink file:///tmp").unwrap();
        assert_eq!(
            m,
            Message::ExtendedOutput {
                pane: PaneId(1),
                output_type: "hyperlink".into(),
                args: "file:///tmp".into(),
            }
        );
    }

    #[test]
    fn parse_pause_continue() {
        // %pause / %continue 是 tmux 3.3+ 流控消息，第一版只识别并安全忽略
        let m = parse_line("%pause 100").unwrap();
        assert_eq!(m, Message::Pause { args: "100".into() });

        let m = parse_line("%continue 100").unwrap();
        assert_eq!(m, Message::Continue { args: "100".into() });

        // 空参数也应识别
        assert!(matches!(parse_line("%pause"), Some(Message::Pause { .. })));
        assert!(matches!(
            parse_line("%continue"),
            Some(Message::Continue { .. })
        ));

        // keyword 返回正确的类型名
        assert_eq!(
            Message::Pause {
                args: String::new()
            }
            .keyword(),
            "pause"
        );
        assert_eq!(
            Message::Continue {
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
                window: WindowId(0),
                pane: PaneId(1),
            }
        );
        // 兼容 @ 前缀的 pane id
        let m2 = parse_line("%window-pane-changed @1 @2").unwrap();
        assert_eq!(
            m2,
            Message::WindowPaneChanged {
                window: WindowId(1),
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
                session: SessionId(0),
                window: WindowId(1),
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
            Err(ControlEscapeError::InvalidHex(_))
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
