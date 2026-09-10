//! Frontend policy for terminal mirrors and parser-generated replies.
//!
//! A frontend that renders a remote pane is not the owner of that pane's PTY.
//! This module keeps the input/reply policy next to the surface consumers and
//! outside Core's terminal Index model.

/// A terminal query detected in a byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    OscDynamicColor(u8),
    CsiDeviceAttributes,
    CsiDeviceStatus,
    KittyKeyboard,
}

/// Whether parser-generated terminal replies should be sent to the backend.
pub fn should_forward_parser_response(
    during_remote_output_feed: bool,
    is_tmux_mirror: bool,
) -> bool {
    !is_tmux_mirror || !during_remote_output_feed
}

/// Filter input emitted by a VTE/parser commit signal.
pub fn should_forward_mixed_input(
    during_remote_output_feed: bool,
    is_tmux_mirror: bool,
    data: &[u8],
) -> bool {
    if data.is_empty() || !should_forward_parser_response(during_remote_output_feed, is_tmux_mirror)
    {
        return false;
    }
    if is_tmux_mirror && looks_like_parser_reply(data) {
        return false;
    }
    true
}

/// Disable mouse reporting while a remote mirror is displayed.
pub const DISABLE_MOUSE_TRACKING: &[u8] =
    b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l";

/// Remove control characters unsafe to send as paste input.
pub fn sanitize_paste(text: &str, _bracketed: bool) -> String {
    text.chars()
        .filter(|ch| *ch == '\n' || *ch == '\r' || *ch == '\t' || *ch >= ' ')
        .collect()
}

/// Encode sanitized clipboard text for the target's bracketed-paste mode.
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

/// Detect terminal queries in order, de-duplicated by query family.
pub fn queries_in(bytes: &[u8]) -> Vec<QueryKind> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            index += 1;
            continue;
        }
        if index + 1 >= bytes.len() {
            break;
        }
        match bytes[index + 1] {
            b']' => {
                if let Some((kind, consumed)) = parse_osc_query(bytes, index) {
                    push_unique(&mut found, kind);
                    index += consumed;
                    continue;
                }
            }
            b'[' => {
                if let Some((kind, consumed)) = parse_csi_query(bytes, index) {
                    push_unique(&mut found, kind);
                    index += consumed;
                    continue;
                }
            }
            _ => {}
        }
        index += 2;
    }
    found
}

pub fn contains_query(bytes: &[u8]) -> bool {
    !queries_in(bytes).is_empty()
}

/// Detect OSC/CSI replies that must not become shell input in mirror mode.
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
    if !found
        .iter()
        .any(|existing| query_label(*existing) == query_label(kind))
    {
        found.push(kind);
    }
}

fn query_label(kind: QueryKind) -> &'static str {
    match kind {
        QueryKind::OscDynamicColor(10) => "osc10",
        QueryKind::OscDynamicColor(11) => "osc11",
        QueryKind::OscDynamicColor(12) => "osc12",
        QueryKind::OscDynamicColor(_) => "osc",
        QueryKind::CsiDeviceAttributes => "da",
        QueryKind::CsiDeviceStatus => "dsr",
        QueryKind::KittyKeyboard => "kitty",
    }
}

fn parse_osc_query(bytes: &[u8], start: usize) -> Option<(QueryKind, usize)> {
    let mut index = start + 2;
    let code_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == code_start || index >= bytes.len() {
        return None;
    }
    let code: u8 = std::str::from_utf8(&bytes[code_start..index])
        .ok()?
        .parse()
        .ok()?;
    if !(10..=12).contains(&code) {
        return None;
    }
    while index < bytes.len() && bytes[index] == b' ' {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b';' {
        return None;
    }
    index += 1;
    while index < bytes.len() && bytes[index] == b' ' {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'?' {
        return None;
    }
    index += 1;
    while index < bytes.len() && bytes[index] != 0x07 {
        if bytes[index] == 0x1b && index + 1 < bytes.len() && bytes[index + 1] == b'\\' {
            index += 2;
            return Some((QueryKind::OscDynamicColor(code), index - start));
        }
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    Some((QueryKind::OscDynamicColor(code), index - start + 1))
}

fn parse_csi_query(bytes: &[u8], start: usize) -> Option<(QueryKind, usize)> {
    let mut index = start + 2;
    let mut saw_question = false;
    if bytes.get(index) == Some(&b'?') {
        saw_question = true;
        index += 1;
    }
    let mut saw_greater = false;
    if bytes.get(index) == Some(&b'>') {
        saw_greater = true;
        index += 1;
    }
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b';') {
        index += 1;
    }
    let kind = match *bytes.get(index)? {
        b'c' => QueryKind::CsiDeviceAttributes,
        b'n' => QueryKind::CsiDeviceStatus,
        b'u' if saw_question || saw_greater => QueryKind::KittyKeyboard,
        _ => return None,
    };
    Some((kind, index - start + 1))
}

fn osc_is_color_report(rest: &[u8]) -> bool {
    let value = std::str::from_utf8(rest).unwrap_or("").trim_start();
    let Some((_, body)) = value.split_once(';') else {
        return value.contains("rgb:");
    };
    let body = body.trim_start();
    body.starts_with("rgb:") || body.starts_with('#')
}

fn csi_is_device_reply(rest: &[u8]) -> bool {
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

    #[test]
    fn mirror_policy_drops_only_remote_feed_replies() {
        assert!(!should_forward_parser_response(true, true));
        assert!(should_forward_parser_response(false, true));
        assert!(should_forward_parser_response(true, false));
        assert!(should_forward_parser_response(false, false));
    }

    #[test]
    fn detects_terminal_queries_and_parser_replies() {
        assert_eq!(
            queries_in(b"\x1b]10;?\x07\x1b]11;?\x07\x1b[c\x1b[6n"),
            vec![
                QueryKind::OscDynamicColor(10),
                QueryKind::OscDynamicColor(11),
                QueryKind::CsiDeviceAttributes,
                QueryKind::CsiDeviceStatus,
            ]
        );
        assert!(looks_like_parser_reply(b"\x1b]10;rgb:4c4c/4f4f/6969\x07"));
        assert!(looks_like_parser_reply(b"\x1b[?65;4;1c"));
        assert!(!looks_like_parser_reply(b"\x1b]10;?\x07"));
    }

    #[test]
    fn mixed_input_filters_empty_and_mirror_replies() {
        assert!(!should_forward_mixed_input(false, true, b""));
        assert!(!should_forward_mixed_input(
            false,
            true,
            b"\x1b]10;rgb:0000/0000/0000\x1b\\"
        ));
        assert!(should_forward_mixed_input(false, true, b"git lg\n"));
        assert!(!should_forward_mixed_input(true, true, b"git lg\n"));
    }

    #[test]
    fn paste_filter_and_bracketed_encoding_are_stable() {
        assert_eq!(sanitize_paste("a\x03b\n\tc", true), "ab\n\tc");
        assert!(encode_clipboard_paste("", true).is_empty());
        assert_eq!(encode_clipboard_paste("hi", false), b"hi");
        assert_eq!(encode_clipboard_paste("hi", true), b"\x1b[200~hi\x1b[201~");
    }
}
