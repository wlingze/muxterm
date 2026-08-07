//! tmux 命令构造器（强类型）。
//!
//! 客户端通过 tmux stdin 发送命令。控制模式下命令**不带 `%` 前缀**（`%` 是 tmux
//! → 客户端的通知），每条命令以换行结尾。
//!
//! 本模块提供强类型构造器，避免直接拼裸字符串导致的转义/注入问题：
//! - pane/window/session id 用 newtype 包装（`PaneId` / `WindowId` / `SessionId`）
//! - 按键用 `Key` enum 区分「逐字文本」与「特殊键」
//! - 文本参数统一 C 转义后用双引号包裹（与 tmux 解析一致）
//!
//! 所有构造器返回 [`TmuxCommand`]，调用 `.to_string()` 得到带换行的完整命令行。

use super::protocol::ControlEscapeDecoder;
use crate::core::types::PaneId as ProtoPaneId;
use std::fmt::Write;

// 复用 `crate::types` 里的 ID 类型，保持单一来源。
pub use crate::core::types::{PaneId, SessionId, WindowId};

/// 一个已构造好的 tmux 命令。
///
/// `raw` 是不含末尾换行的命令文本；[`TmuxCommand::to_string`] 会补上 `\n`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxCommand {
    raw: String,
}

impl TmuxCommand {
    /// 直接包装一段已构造好的命令文本（不含末尾换行）。
    pub(crate) fn from_raw(raw: String) -> Self {
        Self { raw }
    }

    /// 命令文本（不含末尾换行）。
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// 返回带末尾换行的命令字符串，可直接写入 tmux stdin。
    pub fn to_line(&self) -> String {
        let mut s = self.raw.clone();
        s.push('\n');
        s
    }
}

impl std::fmt::Display for TmuxCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)?;
        f.write_str("\n")
    }
}

// ============================================================================
// Key：按键类型
// ============================================================================

/// 发送给 pane 的按键。
///
/// - [`Key::Literal`]：逐字文本（`send-keys -l`），不解释 tmux 特殊键名，用于
///   粘贴任意字符串。会被 C 转义 + 双引号包裹。
/// - [`Key::Special`]：tmux 特殊键名（如 `Enter` / `C-c` / `Up`），原样发送（不
///   加引号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// 逐字文本，用 `-l` 模式发送（不解释转义/特殊键）。
    Literal(String),
    /// tmux 特殊键名，如 "Enter"、"C-c"、"Up"、"BSpace"。
    Special(&'static str),
}

impl Key {
    pub fn enter() -> Self {
        Key::Special("Enter")
    }
    pub fn ctrl(c: char) -> Self {
        // tmux 用 C-x 表示 Ctrl+x
        let name = match c {
            'c' => "C-c",
            'd' => "C-d",
            'z' => "C-z",
            'a' => "C-a",
            'e' => "C-e",
            'l' => "C-l",
            'u' => "C-u",
            'w' => "C-w",
            'k' => "C-k",
            _ => {
                // 通用：构造 "C-<lower>"
                // 用 Box::leak 得到 'static str
                let s: String = format!("C-{}", c.to_ascii_lowercase());
                let leaked: &'static str = Box::leak(s.into_boxed_str());
                return Key::Special(leaked);
            }
        };
        Key::Special(name)
    }
    pub fn tab() -> Self {
        Key::Special("Tab")
    }
    pub fn bspace() -> Self {
        Key::Special("BSpace")
    }
    pub fn escape() -> Self {
        Key::Special("Escape")
    }
    pub fn up() -> Self {
        Key::Special("Up")
    }
    pub fn down() -> Self {
        Key::Special("Down")
    }
    pub fn left() -> Self {
        Key::Special("Left")
    }
    pub fn right() -> Self {
        Key::Special("Right")
    }
    pub fn literal<S: Into<String>>(s: S) -> Self {
        Key::Literal(s.into())
    }
}

// ============================================================================
// 内部辅助：C 转义 + 引号包裹
// ============================================================================

/// 把任意字节编码为 tmux 可接受的 C 转义双引号字符串。
///
/// 规则（与 tmux `cmd_queue`/`format` 解析一致）：
/// - `\` → `\\`
/// - `"` → `\"`
/// - 0x1B (ESC) → `\e`
/// - `\n` → `\n`, `\r` → `\r`, `\t` → `\t`
/// - 其他非打印字节 → 三位八进制 `\ooo`
/// - 普通可打印字符原样
fn quote_c_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'\\' => out.push_str(r"\\"),
            b'"' => out.push_str(r#"\""#),
            0x1B => out.push_str(r"\e"),
            b'\n' => out.push_str(r"\n"),
            b'\r' => out.push_str(r"\r"),
            b'\t' => out.push_str(r"\t"),
            0x20..=0x7E => out.push(b as char),
            other => {
                // tmux 控制模式按反斜杠后的最多三位八进制解码。
                let _ = write!(out, r"\{:03o}", other);
            }
        }
    }
    out.push('"');
    out
}

fn quote_c_string(s: &str) -> String {
    quote_c_bytes(s.as_bytes())
}

/// 把一个 key 渲染为 send-keys 的单个 token。
fn render_key(key: &Key) -> String {
    match key {
        Key::Literal(s) => quote_c_string(s),
        Key::Special(name) => (*name).to_string(),
    }
}

/// 构造 `-t <target>` 参数（target 是 pane/window/session id 或名字）。
fn target_arg(target: &str) -> String {
    format!("-t {target}")
}

fn pane_target(p: PaneId) -> String {
    // tmux 命令里 pane id 是 %N（-CC 通知里也是 %output %N）；
    // @N 是 window id，不能混用。
    target_arg(&format!("%{}", p.0))
}
fn window_target(w: WindowId) -> String {
    // tmux 协议用 @N 格式的 window id（与 muxterm 的 wN 显示格式不同）
    target_arg(&format!("@{}", w.0))
}
fn session_target(s: SessionId) -> String {
    target_arg(&s.as_str())
}

// ============================================================================
// 命令构造器
// ============================================================================

/// 发送按键到 pane。
///
/// - 单个 `Key::Literal` 用 `send-keys -l`（逐字，不解释特殊键）。
/// - 单个 `Key::Special` 用普通 `send-keys`。
/// - 混合多个 key 时：若有任意 Literal，则全部按逐字模式拼接成一个 `-l` 字符串
///   （特殊键按 tmux 键名字面拼接，这在纯特殊键场景下不该走这条路径）；否则
///   用普通 `send-keys` 逐个发特殊键。
pub fn send_keys(pane: PaneId, keys: &[Key]) -> TmuxCommand {
    if keys.is_empty() {
        // 空发送：仍发一条 send-keys -t（tmux 会忽略）
        return build(&[pane_target(pane).to_string()], "send-keys");
    }
    // 分离 Literal 和 Special 键：Literal 用 -l 逐字发送，Special 用 tmux 键名发送。
    // 混合时先发 -l 文本，再发特殊键（两条命令用换行连接）。
    let literals: Vec<&str> = keys
        .iter()
        .filter_map(|k| match k {
            Key::Literal(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let specials: Vec<&Key> = keys
        .iter()
        .filter(|k| matches!(k, Key::Special(_)))
        .collect();

    if !literals.is_empty() && specials.is_empty() {
        // 纯逐字模式
        let text: String = literals.concat();
        build(
            &[pane_target(pane), "-l".to_string(), quote_c_string(&text)],
            "send-keys",
        )
    } else if literals.is_empty() && !specials.is_empty() {
        // 纯特殊键模式
        let mut args = vec![pane_target(pane)];
        for k in &specials {
            args.push(render_key(k));
        }
        build(&args, "send-keys")
    } else {
        // 混合模式：先 -l 文本，再特殊键（用换行连接两条命令）
        let text: String = literals.concat();
        let lit_cmd = build(
            &[pane_target(pane), "-l".to_string(), quote_c_string(&text)],
            "send-keys",
        );
        let mut special_args = vec![pane_target(pane)];
        for k in &specials {
            special_args.push(render_key(k));
        }
        let special_cmd = build(&special_args, "send-keys");
        // 两条命令用换行连接（TmuxCommand::to_line 已自带末尾换行）
        TmuxCommand::from_raw(format!("{}\n{}", lit_cmd.as_str(), special_cmd.as_str()))
    }
}

/// 发送原始字节，不经过 UTF-8 转换；用于终端控制字节和任意粘贴数据。
pub fn send_keys_bytes(pane: PaneId, bytes: &[u8]) -> TmuxCommand {
    build(
        &[pane_target(pane), "-l".to_string(), quote_c_bytes(bytes)],
        "send-keys",
    )
}

/// 发送前缀键（`prefix` 表里的键）。
pub fn send_prefix(pane: PaneId) -> TmuxCommand {
    build(&[pane_target(pane)], "send-prefix")
}

/// resize-pane。
pub fn resize_pane(pane: PaneId, width: Option<u32>, height: Option<u32>) -> TmuxCommand {
    let mut args = vec![pane_target(pane)];
    if let Some(w) = width {
        args.push(format!("-x {w}"));
    }
    if let Some(h) = height {
        args.push(format!("-y {h}"));
    }
    build(&args, "resize-pane")
}

/// 调整 tmux 控制模式 client 的字符格尺寸。
pub fn refresh_client_size(cols: u32, rows: u32) -> TmuxCommand {
    build(&[format!("-C {cols}x{rows}")], "refresh-client")
}

/// list-windows -t <session>。
pub fn list_windows(session: SessionId) -> TmuxCommand {
    build(&[session_target(session)], "list-windows")
}

/// list-panes -t <window>。
pub fn list_panes(window: WindowId) -> TmuxCommand {
    build(&[window_target(window)], "list-panes")
}

/// display-message -p -t <pane> '<format>'。
pub fn display_message(target: PaneId, format: &str) -> TmuxCommand {
    build(
        &[
            "-p".to_string(),
            pane_target(target),
            quote_c_string(format),
        ],
        "display-message",
    )
}

/// new-window -t <session> -n <name>。
pub fn new_window(session: SessionId, name: Option<&str>) -> TmuxCommand {
    let mut args = vec![session_target(session)];
    if let Some(n) = name {
        args.push(format!("-n {}", quote_c_string(n)));
    }
    build(&args, "new-window")
}

/// kill-window -t <window>。
pub fn kill_window(window: WindowId) -> TmuxCommand {
    build(&[window_target(window)], "kill-window")
}

/// kill-pane -t <pane>。
pub fn kill_pane(pane: PaneId) -> TmuxCommand {
    build(&[pane_target(pane)], "kill-pane")
}

/// split-window -t <window> [-h|-v] [-n name]。
pub fn split_window(
    window: WindowId,
    direction: SplitDirection,
    name: Option<&str>,
) -> TmuxCommand {
    let mut args = vec![window_target(window)];
    match direction {
        SplitDirection::Horizontal => args.push("-h".to_string()),
        SplitDirection::Vertical => args.push("-v".to_string()),
    }
    if let Some(n) = name {
        args.push(format!("-n {}", quote_c_string(n)));
    }
    build(&args, "split-window")
}

/// split 方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// 水平分割（`-h`，左右分）。
    Horizontal,
    /// 垂直分割（`-v`，上下分）。
    Vertical,
}

/// select-pane -t <pane>。
pub fn select_pane(pane: PaneId) -> TmuxCommand {
    build(&[pane_target(pane)], "select-pane")
}

/// select-window -t <window>。
pub fn select_window(window: WindowId) -> TmuxCommand {
    build(&[window_target(window)], "select-window")
}

/// rename-window -t <window> <new_name>。
pub fn rename_window(window: WindowId, new_name: &str) -> TmuxCommand {
    build(
        &[window_target(window), quote_c_string(new_name)],
        "rename-window",
    )
}

/// rename-session -t <session> <new_name>。
pub fn rename_session(session: SessionId, new_name: &str) -> TmuxCommand {
    build(
        &[session_target(session), quote_c_string(new_name)],
        "rename-session",
    )
}

/// detach-client -t <session>。
pub fn detach_client(session: SessionId) -> TmuxCommand {
    build(&[session_target(session)], "detach-client")
}

/// refresh-client。
pub fn refresh_client() -> TmuxCommand {
    build(&[], "refresh-client")
}

// ============================================================================
// 内部 build 辅助
// ============================================================================

/// 把命令名 + 参数列表拼成单行（参数用空格分隔），返回 TmuxCommand。
fn build(args: &[String], name: &str) -> TmuxCommand {
    let mut raw = String::from(name);
    for a in args {
        raw.push(' ');
        raw.push_str(a);
    }
    TmuxCommand::from_raw(raw)
}

// 静默 protocol 的导入（仅用于类型一致性的文档引用，实际未直接用 ControlEscapeDecoder）。
#[allow(unused_imports)]
use ControlEscapeDecoder as _ControlEscapeDecoder;
#[allow(unused_imports)]
use ProtoPaneId as _ProtoPaneId;

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn send_keys_special_enter() {
        let c = send_keys(PaneId(1), &[Key::enter()]);
        assert_eq!(c.as_str(), "send-keys -t %1 Enter");
        assert_eq!(c.to_line(), "send-keys -t %1 Enter\n");
    }

    #[test]
    fn send_keys_ctrl_c() {
        let c = send_keys(PaneId(2), &[Key::ctrl('c')]);
        assert_eq!(c.as_str(), "send-keys -t %2 C-c");
    }

    #[test]
    fn send_keys_multiple_special() {
        let c = send_keys(PaneId(0), &[Key::up(), Key::up(), Key::down()]);
        assert_eq!(c.as_str(), "send-keys -t %0 Up Up Down");
    }

    #[test]
    fn send_keys_literal() {
        let c = send_keys(PaneId(3), &[Key::literal("echo hi")]);
        assert_eq!(c.as_str(), r#"send-keys -t %3 -l "echo hi""#);
    }

    #[test]
    fn send_keys_literal_escapes_quote_and_backslash() {
        let c = send_keys(PaneId(3), &[Key::literal(r#"a"b\c"#)]);
        assert_eq!(c.as_str(), r#"send-keys -t %3 -l "a\"b\\c""#);
    }

    #[test]
    fn send_keys_literal_escapes_newline_and_esc() {
        let c = send_keys(PaneId(0), &[Key::literal("a\nb\x1B")]);
        assert_eq!(c.as_str(), r#"send-keys -t %0 -l "a\nb\e""#);
    }

    #[test]
    fn send_keys_mixed_literal_and_special() {
        // 混合：Literal 用 -l，Special 用 tmux 键名，两条命令换行分隔
        let c = send_keys(PaneId(1), &[Key::literal("ls "), Key::enter()]);
        let line = c.to_line();
        // 应含 -l "ls "（逐字文本）
        assert!(line.contains(r#"-l "ls ""#), "应含逐字文本: {line}");
        // 应含 Enter 作为特殊键（单独命令）
        assert!(line.contains("Enter"), "应含 Enter 特殊键: {line}");
        // 不应把 Enter 拼进 -l 文本
        assert!(!line.contains("ls Enter"), "Enter 不应被拼进 -l: {line}");
        // 应有换行分隔两条命令
        assert!(line.contains("\n"), "应有换行分隔: {line}");
    }

    #[test]
    fn send_keys_tab_bspace_escape_arrows() {
        assert_eq!(
            send_keys(PaneId(1), &[Key::tab()]).as_str(),
            "send-keys -t %1 Tab"
        );
        assert_eq!(
            send_keys(PaneId(1), &[Key::bspace()]).as_str(),
            "send-keys -t %1 BSpace"
        );
        assert_eq!(
            send_keys(PaneId(1), &[Key::escape()]).as_str(),
            "send-keys -t %1 Escape"
        );
        assert_eq!(
            send_keys(PaneId(1), &[Key::left(), Key::right()]).as_str(),
            "send-keys -t %1 Left Right"
        );
    }

    #[test]
    fn send_keys_ctrl_generic() {
        assert_eq!(
            send_keys(PaneId(0), &[Key::ctrl('z')]).as_str(),
            "send-keys -t %0 C-z"
        );
    }

    #[test]
    fn send_keys_empty() {
        let c = send_keys(PaneId(1), &[]);
        assert_eq!(c.as_str(), "send-keys -t %1");
    }

    #[test]
    fn send_prefix_cmd() {
        assert_eq!(send_prefix(PaneId(1)).as_str(), "send-prefix -t %1");
    }

    #[test]
    fn resize_pane_both() {
        let c = resize_pane(PaneId(1), Some(80), Some(24));
        assert_eq!(c.as_str(), "resize-pane -t %1 -x 80 -y 24");
    }

    #[test]
    fn resize_pane_only_width() {
        let c = resize_pane(PaneId(1), Some(80), None);
        assert_eq!(c.as_str(), "resize-pane -t %1 -x 80");
    }

    #[test]
    fn resize_pane_only_height() {
        let c = resize_pane(PaneId(2), None, Some(40));
        assert_eq!(c.as_str(), "resize-pane -t %2 -y 40");
    }

    #[test]
    fn resize_pane_none() {
        let c = resize_pane(PaneId(2), None, None);
        assert_eq!(c.as_str(), "resize-pane -t %2");
    }

    #[test]
    fn refresh_client_with_size() {
        assert_eq!(
            refresh_client_size(120, 40).as_str(),
            "refresh-client -C 120x40"
        );
    }

    #[test]
    fn list_windows_cmd() {
        assert_eq!(list_windows(SessionId(0)).as_str(), "list-windows -t $0");
    }

    #[test]
    fn list_panes_cmd() {
        assert_eq!(list_panes(WindowId(1)).as_str(), "list-panes -t @1");
    }

    #[test]
    fn display_message_cmd() {
        let c = display_message(PaneId(0), "#{pane_current_command}");
        assert_eq!(
            c.as_str(),
            r##"display-message -p -t %0 "#{pane_current_command}""##
        );
    }

    #[test]
    fn display_message_escapes_quote() {
        let c = display_message(PaneId(0), r#"a"b"#);
        assert_eq!(c.as_str(), r##"display-message -p -t %0 "a\"b""##);
    }

    #[test]
    fn new_window_with_name() {
        let c = new_window(SessionId(0), Some("second"));
        assert_eq!(c.as_str(), r#"new-window -t $0 -n "second""#);
    }

    #[test]
    fn new_window_no_name() {
        assert_eq!(new_window(SessionId(0), None).as_str(), "new-window -t $0");
    }

    #[test]
    fn kill_window_cmd() {
        assert_eq!(kill_window(WindowId(1)).as_str(), "kill-window -t @1");
    }

    #[test]
    fn kill_pane_cmd() {
        assert_eq!(kill_pane(PaneId(3)).as_str(), "kill-pane -t %3");
    }

    #[test]
    fn split_window_horizontal() {
        let c = split_window(WindowId(0), SplitDirection::Horizontal, Some("log"));
        assert_eq!(c.as_str(), r#"split-window -t @0 -h -n "log""#);
    }

    #[test]
    fn split_window_vertical_no_name() {
        let c = split_window(WindowId(1), SplitDirection::Vertical, None);
        assert_eq!(c.as_str(), "split-window -t @1 -v");
    }

    #[test]
    fn select_pane_cmd() {
        assert_eq!(select_pane(PaneId(2)).as_str(), "select-pane -t %2");
    }

    #[test]
    fn select_window_cmd() {
        assert_eq!(select_window(WindowId(1)).as_str(), "select-window -t @1");
    }

    #[test]
    fn rename_window_cmd() {
        let c = rename_window(WindowId(0), "main");
        assert_eq!(c.as_str(), r#"rename-window -t @0 "main""#);
    }

    #[test]
    fn rename_window_escapes() {
        let c = rename_window(WindowId(0), "a\\b\"c");
        assert_eq!(c.as_str(), r#"rename-window -t @0 "a\\b\"c""#);
    }

    #[test]
    fn rename_session_cmd() {
        let c = rename_session(SessionId(1), "work");
        assert_eq!(c.as_str(), r#"rename-session -t $1 "work""#);
    }

    #[test]
    fn detach_client_cmd() {
        assert_eq!(detach_client(SessionId(0)).as_str(), "detach-client -t $0");
    }

    #[test]
    fn refresh_client_cmd() {
        assert_eq!(refresh_client().as_str(), "refresh-client");
    }

    #[test]
    fn display_implements_with_newline() {
        let c = send_keys(PaneId(1), &[Key::enter()]);
        let s = format!("{c}");
        assert_eq!(s, "send-keys -t %1 Enter\n");
    }

    #[test]
    fn quote_c_string_control_chars() {
        // tmux 控制模式要求使用八进制；固定三位避免和后续数字粘连。
        let q = quote_c_string("\u{1}\u{5}\u{10}\u{0e}\u{12}\u{7F}");
        assert_eq!(q, r#""\001\005\020\016\022\177""#);
    }

    #[test]
    fn send_keys_bytes_preserves_control_and_non_utf8_bytes() {
        let command = send_keys_bytes(PaneId(1), &[0x03, 0x0c, 0xff]);
        assert_eq!(command.as_str(), "send-keys -t %1 -l \"\\003\\014\\377\"");
    }

    /// send-keys -l 走 C 转义而不是 shell，因此 $、`、; 都不能被展开。
    #[test]
    fn send_keys_literal_does_not_shell_expand() {
        let c = send_keys(PaneId(1), &[Key::literal("$HOME `id` ; echo hi")]);
        assert_eq!(
            c.as_str(),
            r#"send-keys -t %1 -l "$HOME `id` ; echo hi""#
        );
    }

    /// 窗口名里的换行/ESC/控制字节必须编码成 C 转义。
    #[test]
    fn rename_window_escapes_control_and_newline() {
        let c = rename_window(WindowId(0), "a\nb\x1b");
        assert_eq!(c.as_str(), r#"rename-window -t @0 "a\nb\e""#);
    }

    /// display-message 的 format 里的反斜杠/引号/换行都要转义。
    #[test]
    fn display_message_escapes_backslash_quote_newline() {
        let c = display_message(PaneId(0), "a\\b\"c\nd");
        assert_eq!(c.as_str(), r##"display-message -p -t %0 "a\\b\"c\nd""##);
    }

    /// 分割窗口的名字带引号/反斜杠时也要转义。
    #[test]
    fn split_window_escapes_name() {
        let c = split_window(WindowId(0), SplitDirection::Horizontal, Some("a\"b\\c"));
        assert_eq!(c.as_str(), r#"split-window -t @0 -h -n "a\"b\\c""#);
    }
}

// ── Phase 5：输入边界 ──────────────────────────────────────────
// 验证原始终端输入序列（bracketed paste / mouse reporting / focus /
// 方向键 / 特殊控制）经 quote_c_string 编码后，能用 ControlEscapeDecoder
// 无损还原。这是 send-keys -l 写入 PTY 的字节保真核心。

fn roundtrip_bytes(bytes: &[u8]) -> Vec<u8> {
    let quoted = quote_c_bytes(bytes);
    // 去掉两端引号，交给 ControlEscapeDecoder 解码
    let inner = quoted.trim_start_matches('"').trim_end_matches('"');
    ControlEscapeDecoder::new().decode(inner).expect("应能解码")
}

#[test]
fn input_roundtrip_bracketed_paste() {
    // bracketed paste 序列：ESC[200~ ... ESC[201~
    let seq = b"\x1b[200~pasted text\x1b[201~";
    assert_eq!(roundtrip_bytes(seq), seq);
}

#[test]
fn input_roundtrip_mouse_reporting() {
    // SGR mouse 上报序列
    let seq = b"\x1b[<0;12;34M";
    assert_eq!(roundtrip_bytes(seq), seq);
    let seq2 = b"\x1b[<64;5;6m";
    assert_eq!(roundtrip_bytes(seq2), seq2);
}

#[test]
fn input_roundtrip_focus_in_out() {
    assert_eq!(roundtrip_bytes(b"\x1b[I"), b"\x1b[I");
    assert_eq!(roundtrip_bytes(b"\x1b[O"), b"\x1b[O");
}

#[test]
fn input_roundtrip_arrow_and_modifier_keys() {
    assert_eq!(roundtrip_bytes(b"\x1b[A"), b"\x1b[A");
    assert_eq!(roundtrip_bytes(b"\x1b[1;5A"), b"\x1b[1;5A");
    assert_eq!(roundtrip_bytes(b"\x1b[15~"), b"\x1b[15~");
}

#[test]
fn input_roundtrip_ctrl_and_alt() {
    for byte in [0x01, 0x03, 0x05, 0x0c, 0x0e, 0x10, 0x12] {
        assert_eq!(roundtrip_bytes(&[byte]), [byte]);
    }
    assert_eq!(roundtrip_bytes(b"\x1bn"), b"\x1bn"); // Alt-n
    assert_eq!(roundtrip_bytes(b"\x1a"), b"\x1a"); // Ctrl-Z
    assert_eq!(roundtrip_bytes(b"\x7f"), b"\x7f"); // DEL
}

#[test]
fn input_roundtrip_osc_dynamic_colors_and_csi_query() {
    // 用户实测：git lg 会输出 OSC 动态颜色 + CSI 查询序列，若引导字节丢失会被
    // shell 当普通文本执行（"10;rgb:... zsh: command not found"）。
    // OSC 10 前景色、OSC 11 背景色（以 BEL/ST 结尾）
    let osc_fg = b"\x1b]10;rgb:0000/0000/0000\x07";
    assert_eq!(roundtrip_bytes(osc_fg), osc_fg);
    let osc_bg = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
    assert_eq!(roundtrip_bytes(osc_bg), osc_bg);
    // CSI 设备属性/光标查询
    let csi = b"\x1b[?65;4;1;2;6;21;22;17;28c";
    assert_eq!(roundtrip_bytes(csi), csi);
    let csi_b = b"\x1b[65;1;2c";
    assert_eq!(roundtrip_bytes(csi_b), csi_b);
}

#[test]
fn input_roundtrip_cjk_bytes_are_not_mangled() {
    // 中文 + 全角标点：必须逐字节还原，不能被 from_utf8_lossy 替换成 �
    let cjk = "编译测试 单测 lint".as_bytes();
    assert_eq!(roundtrip_bytes(cjk), cjk);
    let mixed = "T1(编译/单测/lint)".as_bytes();
    assert_eq!(roundtrip_bytes(mixed), mixed);
    // 混合：ESC + CJK + OSC
    let mut combined = vec![0x1b, b'['];
    combined.extend_from_slice("编译".as_bytes());
    combined.extend_from_slice(b"]10;rgb:0000/0000/0000\x07");
    combined.extend_from_slice("测试".as_bytes());
    assert_eq!(roundtrip_bytes(&combined), combined);
}

#[test]
fn input_roundtrip_tab_split_key_sequence() {
    // Alt-d 创建新 pane 等组合：ESC d、ESC w 等
    assert_eq!(roundtrip_bytes(b"\x1bd"), b"\x1bd");
    assert_eq!(roundtrip_bytes(b"\x1bw"), b"\x1bw");
    assert_eq!(roundtrip_bytes(b"\x1bt"), b"\x1bt");
}

#[test]
fn input_roundtrip_utf8_survives() {
    // 多字节 UTF-8 文本在 send-keys -l 路径应保持字节不变
    let s = "中文 emoji😀";
    let bytes = s.as_bytes();
    assert_eq!(roundtrip_bytes(bytes), bytes);
}

#[test]
fn input_roundtrip_dcs_passthrough() {
    // tmux passthrough / kitty keyboard：ESC P ... ESC \ 不应被破坏
    let seq = b"\x1bPtmux;\x1b\x1b]1337;SetUserVar=X=\x07\x1b\\";
    assert_eq!(roundtrip_bytes(seq), seq);
}
