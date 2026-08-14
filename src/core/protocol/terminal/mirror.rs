//! tmux 镜像下的终端查询应答策略（与 macOS `TerminalMirrorPolicy` /
//! `TerminalQueryDetector` 对齐）。
//!
//! tmux 控制模式拥有 pane 的 PTY：OSC 10/11/12、CSI DA/DSR 由 tmux 用
//! `refresh-client -r` 代答。前端在 feed 远端输出时若把解析器应答经
//! `send-keys -l` 写回，会被 pane 回显并当命令执行，表现为 `git lg` 后
//! `zsh: command not found: 10` / `65;...c`。

/// 一段 pane 输出里检测到的终端查询类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// OSC 10/11/12 `?` 动态颜色查询。
    OscDynamicColor(u8),
    /// CSI DA：`ESC [ c` / `ESC [ ? ... c`。
    CsiDeviceAttributes,
    /// CSI DSR：`ESC [ n` / `ESC [ 5 n` / `ESC [ 6 n`。
    CsiDeviceStatus,
    /// kitty keyboard：`CSI ? u` / `CSI > 4;... u`。
    KittyKeyboard,
}

/// 是否把解析器应答回写给后端。
///
/// 对齐 macOS `TerminalMirrorPolicy.shouldForwardParserResponse`：
/// tmux 镜像在 feed 远端输出期间一律丢弃；本地 PTY 始终转发。
pub fn should_forward_parser_response(
    during_remote_output_feed: bool,
    is_tmux_mirror: bool,
) -> bool {
    !is_tmux_mirror || !during_remote_output_feed
}

/// VTE `commit` 把用户输入和解析器应答混在同一信号里。
///
/// tmux 镜像：feed 期间全部丢弃；feed 之外若数据是 OSC/CSI 应答
/// （VTE 可能在 `feed()` 返回后才 emit）也丢弃。
pub fn should_forward_mixed_input(
    during_remote_output_feed: bool,
    is_tmux_mirror: bool,
    data: &[u8],
) -> bool {
    if data.is_empty() {
        return false;
    }
    if !should_forward_parser_response(during_remote_output_feed, is_tmux_mirror) {
        return false;
    }
    if is_tmux_mirror && looks_like_parser_reply(data) {
        return false;
    }
    true
}

/// tmux 镜像：关掉 VTE 鼠标跟踪，让拖选用 GTK 选择（对齐 macOS
/// `allowMouseReporting = false`）。htop/codex 的 `CSI ? 1000h` 否则会
/// 把点击送给应用，Ctrl+Shift+C 复制不到字。
pub const DISABLE_MOUSE_TRACKING: &[u8] =
    b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l";

/// 把剪贴板文本编码成发给 pane 的字节。空内容不得发空的 bracketed paste
/// 包装（2310.log 的 `\\e[200~\\e[201~`）。
pub fn encode_clipboard_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

/// 扫描字节流中的终端查询（去重、保序）。
pub fn queries_in(bytes: &[u8]) -> Vec<QueryKind> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        match bytes[i + 1] {
            b']' => {
                if let Some((kind, consumed)) = parse_osc_query(bytes, i) {
                    push_unique(&mut found, kind);
                    i += consumed;
                    continue;
                }
            }
            b'[' => {
                if let Some((kind, consumed)) = parse_csi_query(bytes, i) {
                    push_unique(&mut found, kind);
                    i += consumed;
                    continue;
                }
            }
            _ => {}
        }
        i += 2;
    }
    found
}

pub fn contains_query(bytes: &[u8]) -> bool {
    !queries_in(bytes).is_empty()
}

/// OSC 颜色应答 / CSI DA 应答（git lg 泄漏进 shell 的那种字节）。
pub fn looks_like_parser_reply(data: &[u8]) -> bool {
    if data.len() < 3 || data[0] != 0x1b {
        return false;
    }
    match data[1] {
        b']' => osc_is_color_report(&data[2..]),
        b'[' => csi_is_device_reply(&data[2..]),
        _ => false,
    }
}

fn push_unique(found: &mut Vec<QueryKind>, kind: QueryKind) {
    let label = query_label(kind);
    if !found.iter().any(|k| query_label(*k) == label) {
        found.push(kind);
    }
}

fn query_label(kind: QueryKind) -> String {
    match kind {
        QueryKind::OscDynamicColor(code) => format!("osc{code}"),
        QueryKind::CsiDeviceAttributes => "da".into(),
        QueryKind::CsiDeviceStatus => "dsr".into(),
        QueryKind::KittyKeyboard => "kitty".into(),
    }
}

fn parse_osc_query(bytes: &[u8], start: usize) -> Option<(QueryKind, usize)> {
    let mut i = start + 2;
    let code_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == code_start || i >= bytes.len() {
        return None;
    }
    let code: u8 = std::str::from_utf8(&bytes[code_start..i])
        .ok()?
        .parse()
        .ok()?;
    if !(10..=12).contains(&code) {
        return None;
    }
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b';' {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'?' {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i] != 0x07 {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
            i += 2;
            return Some((QueryKind::OscDynamicColor(code), i - start));
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    Some((QueryKind::OscDynamicColor(code), i - start + 1))
}

fn parse_csi_query(bytes: &[u8], start: usize) -> Option<(QueryKind, usize)> {
    let mut i = start + 2;
    let mut saw_question = false;
    if i < bytes.len() && bytes[i] == b'?' {
        saw_question = true;
        i += 1;
    }
    let mut saw_greater = false;
    if i < bytes.len() && bytes[i] == b'>' {
        saw_greater = true;
        i += 1;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let kind = match bytes[i] {
        b'c' => QueryKind::CsiDeviceAttributes,
        b'n' => QueryKind::CsiDeviceStatus,
        b'u' if saw_question || saw_greater => QueryKind::KittyKeyboard,
        _ => return None,
    };
    Some((kind, i - start + 1))
}

fn osc_is_color_report(rest: &[u8]) -> bool {
    // `10;rgb:...` / `11;#rrggbb`（VTE 应答）；查询 `10;?` 不是应答。
    let s = std::str::from_utf8(rest).unwrap_or("");
    let s = s.trim_start();
    let Some((_, body)) = s.split_once(';') else {
        return s.contains("rgb:");
    };
    let body = body.trim_start();
    body.starts_with("rgb:") || body.starts_with('#')
}

fn csi_is_device_reply(rest: &[u8]) -> bool {
    // DA：`?65;...c`  DSR：`0n` / `3;5R`
    if rest.is_empty() {
        return false;
    }
    if rest[0] == b'?' {
        return rest.contains(&b'c');
    }
    rest.ends_with(b"c") || rest.ends_with(b"n") || rest.ends_with(b"R")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::terminal::emulate::TerminalState;

    #[test]
    fn tmux_mirror_drops_parser_response_during_feed() {
        assert!(!should_forward_parser_response(true, true));
    }

    #[test]
    fn tmux_mirror_drops_even_when_feed_contains_query() {
        assert!(!should_forward_parser_response(true, true));
    }

    #[test]
    fn tmux_mirror_forwards_outside_feed() {
        assert!(should_forward_parser_response(false, true));
    }

    #[test]
    fn local_terminal_always_forwards_responses() {
        assert!(should_forward_parser_response(true, false));
        assert!(should_forward_parser_response(false, false));
    }

    #[test]
    fn detects_osc_dynamic_color_queries() {
        let raw = b"\x1b]10;?\x07\x1b]11;?\x07";
        assert_eq!(
            queries_in(raw),
            vec![
                QueryKind::OscDynamicColor(10),
                QueryKind::OscDynamicColor(11)
            ]
        );
        assert!(contains_query(raw));
    }

    #[test]
    fn detects_osc_query_with_st_terminator() {
        assert_eq!(
            queries_in(b"\x1b]12;?\x1b\\"),
            vec![QueryKind::OscDynamicColor(12)]
        );
    }

    #[test]
    fn detects_csi_device_attributes() {
        let kinds = queries_in(b"\x1b[c\x1b[?65;4;1;2;6;21;22;17;28c");
        assert!(kinds.contains(&QueryKind::CsiDeviceAttributes));
    }

    #[test]
    fn detects_csi_device_status() {
        assert_eq!(queries_in(b"\x1b[6n"), vec![QueryKind::CsiDeviceStatus]);
    }

    #[test]
    fn detects_kitty_keyboard_queries() {
        assert!(contains_query(b"\x1b[?u"));
        assert!(contains_query(b"\x1b[>4;0u"));
    }

    #[test]
    fn ignores_plain_output_without_queries() {
        assert!(!contains_query(b"hello world\r\n\x1b[31mred\x1b[0m"));
        assert!(queries_in(b"hello world\r\n\x1b[31mred\x1b[0m").is_empty());
    }

    #[test]
    fn ignores_osc_color_set_not_query() {
        assert!(!contains_query(b"\x1b]10;#ffffff\x07\x1b]11;#000000\x07"));
    }

    /// 用户实测：VTE commit 把主题色 OSC 应答当按键 send-keys 进 pane。
    #[test]
    fn tmux_drops_vte_osc_color_reply_even_after_feed() {
        let leaked = b"\x1b]10;rgb:4c4c/4f4f/6969\x07";
        assert!(looks_like_parser_reply(leaked));
        assert!(looks_like_parser_reply(b"\x1b]11;#eff1f5\x07"));
        assert!(!looks_like_parser_reply(b"\x1b]10;?\x07"));
        assert!(!should_forward_mixed_input(false, true, leaked));
        assert!(!should_forward_mixed_input(true, true, leaked));
        assert!(should_forward_mixed_input(false, false, leaked));
    }

    #[test]
    fn tmux_forwards_normal_keystrokes_outside_feed() {
        assert!(should_forward_mixed_input(false, true, b"git lg\n"));
        assert!(!should_forward_mixed_input(true, true, b"git lg\n"));
    }

    /// `git lg` 输出里的 OSC 10/11 查询：解析器会生成应答，tmux 镜像必须丢弃。
    #[test]
    fn gitlg_osc_query_replies_are_dropped_in_tmux_mirror() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]10;?\x07\x1b]11;?\x07\x1b[c");
        let replies = t.take_reply();
        assert!(
            replies.windows(7).any(|w| w == b"]10;rgb") || replies.windows(3).any(|w| w == b"]10"),
            "解析器应对 OSC 10 查询生成应答: {replies:?}"
        );
        assert!(
            !should_forward_parser_response(true, true),
            "tmux feed 期间不得把应答 send-keys 回 pane"
        );
        assert!(!should_forward_mixed_input(true, true, &replies));
    }

    #[test]
    fn real_gitlg_sample_preserves_osc_escapes_and_is_not_forwarded() {
        let raw = include_str!("../../../../tests/samples/real-gitlg-osc-query.txt");
        // 样例是 tmux %output 行；核心仍是 ESC]10;rgb 不得当命令。
        assert!(raw.contains(r"\e]10;rgb:") || raw.contains("]10;rgb:"));
        let leaked = b"\x1b]10;rgb:0000/0000/0000\x1b\\";
        assert!(looks_like_parser_reply(leaked));
        assert!(!should_forward_mixed_input(true, true, leaked));
    }

    #[test]
    fn empty_clipboard_does_not_emit_bracketed_paste_wrappers() {
        assert!(encode_clipboard_paste("", true).is_empty());
        assert!(encode_clipboard_paste("", false).is_empty());
    }

    #[test]
    fn clipboard_paste_wraps_only_when_bracketed_mode_on() {
        assert_eq!(encode_clipboard_paste("hi", false), b"hi");
        assert_eq!(encode_clipboard_paste("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn disable_mouse_tracking_clears_htop_sgr_mouse() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
        assert!(t.mouse_reporting);
        t.feed(DISABLE_MOUSE_TRACKING);
        assert!(!t.mouse_reporting, "镜像必须关掉鼠标跟踪，否则无法拖选复制");
    }
}
