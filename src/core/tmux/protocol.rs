//! tmux 控制协议解析器（line-oriented）。
//!
//! tmux 以 `-CC` 启动后会向 stdout 输出结构化通知，每行以 `%` 开头（命令响应内容
//! 除外，夹在 `%begin` / `%end` 之间）。本模块提供：
//!
//! - [`Message`]：覆盖所有已知通知类型的 enum
//! - [`parse_line`]：解析单行原始输出（已按真换行切分）为 `Option<Message>`
//! - [`ControlEscapeDecoder`]：解码 `%output` 里 C 风格转义字符串
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
// ID 解析（类型定义在 crate::core::types）
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

impl LayoutChange {
    /// 解析形如 `<cols>x<rows>,<x>,<y>,<flags>` 的布局几何字符串。
    ///
    /// 注意 tmux 的完整 window_layout 可能带树前缀（如 `aabd,100x30,0,0,0`），
    /// 这里只取最后一段 `<cols>x<rows>,<x>,<y>,<flags>`。
    pub fn parse(layout: &str) -> Result<Self, ProtocolError> {
        // 取最后一个逗号段里含 'x' 的部分（几何段）
        let parts: Vec<&str> = layout.split(',').collect();
        // 找到包含 'x' 的段（几何段）
        let geo_idx = parts.iter().position(|p| p.contains('x')).ok_or_else(|| {
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
        // 几何段之后的若干个数字字段：x, y, flags（至少 flags）
        // 标准格式：<tree>,<cols>x<rows>,<x>,<y>,<flags>
        // 我们从 geo_idx 之后取 3 个数字作为 x,y,flags；不足则补 0。
        let after = &parts[geo_idx + 1..];
        let num_after = after.len();
        let x = if num_after >= 1 {
            u32::from_str(after[0]).map_err(|_| {
                ProtocolError::MalformedField(format!("layout x 非数字: {}", after[0]))
            })?
        } else {
            0
        };
        let y = if num_after >= 2 {
            u32::from_str(after[1]).map_err(|_| {
                ProtocolError::MalformedField(format!("layout y 非数字: {}", after[1]))
            })?
        } else {
            0
        };
        let flags = if num_after >= 3 {
            u32::from_str(after[2]).map_err(|_| {
                ProtocolError::MalformedField(format!("layout flags 非数字: {}", after[2]))
            })?
        } else {
            0
        };
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
    /// `%extended-output <pane_id> <type> <args>`（tmux 3.3+，如 hyperlink）
    ExtendedOutput {
        pane: PaneId,
        output_type: String,
        args: String,
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
            Message::Exit { .. } => "exit",
            Message::ExtendedOutput { .. } => "extended-output",
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
        Ok(&s[1..])
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
    let visible = parts.next().map(LayoutChange::parse).transpose()?;
    Ok(Message::LayoutChange {
        window,
        layout,
        visible_layout: visible,
    })
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
        assert_eq!(WindowId(12).as_str(), "@12");
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
    fn parse_unknown_keyword() {
        let m = parse_line("%session-window-changed $0 @1").unwrap();
        assert_eq!(
            m,
            Message::Unknown {
                keyword: "session-window-changed".into(),
                raw: "$0 @1".into(),
            }
        );
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
        let raw = include_str!("../../../tests/samples/new-session.txt");
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
        let raw = include_str!("../../../tests/samples/cmd-response.txt");
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
        let raw = include_str!("../../../tests/samples/new-session.txt");
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
}
