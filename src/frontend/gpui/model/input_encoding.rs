//! macOS NSEvent 的 Ctrl 组合到终端控制字节的纯映射，以及 Cmd-Enter 分流。
//!
//! 移植自 `chrome/TerminalInputEncoding.swift` 与 `chrome/CmdEnterRouting.swift`。
//!
//! gpui 拿到的按键已经是「键名 + 修饰键」，不是 NSEvent，所以这里的入参是
//! 字符而不是 `NSEvent`。映射表本身与平台无关，必须逐字节一致，否则
//! Ctrl-C/Ctrl-D 之类的信号键会失效。

/// Backspace 发出的字节。
pub const BACKSPACE_BYTE: u8 = 0x7f;

/// GUI 输入里孤立的 `0xFF` 进 zsh 会显示为 `<ffffffff>`。
///
/// 多字节的鼠标/终端协议保持原样，避免破坏原始输入通道。
pub fn is_unexpected_single_byte(data: &[u8]) -> bool {
    data == [0xff]
}

/// 单字符按键在 Ctrl 按下时对应的控制字节。
///
/// 已经编码过的控制码（`0x01..=0x1a`）原样返回，避免二次转换；无法映射的
/// 按键返回 `None`，调用方退回普通文本路径。
pub fn control_byte(key: &str) -> Option<u8> {
    let bytes = key.as_bytes();
    if bytes.len() != 1 {
        return None;
    }
    let byte = bytes[0];
    match byte {
        // 某些输入法/测试工具已经把 charactersIgnoringModifiers 变成控制码。
        0x01..=0x1a => Some(byte),
        0x41..=0x5a => Some(byte - 0x40),
        0x61..=0x7a => Some(byte - 0x60),
        // NUL / Ctrl+2 / Ctrl+`
        0x00 | 0x20 | 0x32 | 0x60 => Some(0x00),
        // ESC / Ctrl+3
        0x5b | 0x33 => Some(0x1b),
        // FS / Ctrl+4
        0x5c | 0x34 => Some(0x1c),
        // GS / Ctrl+5
        0x5d | 0x35 => Some(0x1d),
        // RS / Ctrl+6 / Ctrl+~
        0x5e | 0x7e | 0x36 => Some(0x1e),
        // US / Ctrl+/ / Ctrl+7
        0x5f | 0x2f | 0x37 => Some(0x1f),
        // DEL / Ctrl+?
        0x3f | 0x38 => Some(BACKSPACE_BYTE),
        _ => None,
    }
}

/// Cmd-Enter 按当前表面分流，避免注意力缩放覆盖主窗口 tmux zoom。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    MainWindow,
    AttentionPanel,
    ReplyOverlay,
}

/// Cmd-Enter 要执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdEnterAction {
    /// 主窗口：tmux `resize-pane -Z`。
    ToggleTmuxZoom,
    /// 注意力面板：打开独立 replica overlay。
    OpenReplyOverlay,
    /// overlay 已打开：再按一次关掉。
    CloseReplyOverlay,
}

/// overlay 的标识，用于窗口/视图查找。
pub const OVERLAY_IDENTIFIER: &str = "muxterm.replyOverlay";

/// 按表面决定 Cmd-Enter 的动作。
pub fn cmd_enter_action(on: Surface) -> CmdEnterAction {
    match on {
        Surface::MainWindow => CmdEnterAction::ToggleTmuxZoom,
        Surface::AttentionPanel => CmdEnterAction::OpenReplyOverlay,
        Surface::ReplyOverlay => CmdEnterAction::CloseReplyOverlay,
    }
}

/// 括号粘贴开始标记。取自 SwiftTerm 1.15.0 的
/// `EscapeSequences.bracketedPasteStart`（`Package.resolved` 锁定的版本）。
pub const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
/// 括号粘贴结束标记，同上 `bracketedPasteEnd`。
pub const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

/// 把一段粘贴文本编码成要写进 pane 的字节。
///
/// 与应用处于括号粘贴模式时，整段（包括大段粘贴的每一批）只包一层；分批
/// 只是传输切片，不改变语义。空文本返回空，不发出空的粘贴标记。
pub fn paste_payload(text: &str, bracketed: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut payload = Vec::with_capacity(text.len() + 12);
    if bracketed {
        payload.extend_from_slice(BRACKETED_PASTE_START);
    }
    payload.extend_from_slice(text.as_bytes());
    if bracketed {
        payload.extend_from_slice(BRACKETED_PASTE_END);
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `TerminalView.insertExternalText` 与 `pastePayload` 的括号行为。
    #[test]
    fn paste_payload_wraps_only_when_bracketed() {
        assert_eq!(paste_payload("hi", false), b"hi".to_vec());
        assert_eq!(paste_payload("hi", true), b"\x1b[200~hi\x1b[201~".to_vec());
        assert!(paste_payload("", true).is_empty(), "空粘贴不发标记");
    }

    /// 多行文本不追加回车，也不改写内容。
    #[test]
    fn paste_payload_preserves_newlines_and_utf8() {
        let text = "第一行\n第二行\n";
        let payload = paste_payload(text, true);
        assert_eq!(
            payload,
            [BRACKETED_PASTE_START, text.as_bytes(), BRACKETED_PASTE_END].concat()
        );
        assert_eq!(
            payload.iter().filter(|b| **b == b'\n').count(),
            2,
            "粘贴不得偷偷补回车"
        );
    }

    /// 对照 `TerminalInputEncodingTests.testUnexpectedSingleFFIsRejected...`。
    #[test]
    fn unexpected_single_ff_is_rejected_without_touching_other_bytes() {
        assert!(is_unexpected_single_byte(&[0xff]));
        assert!(!is_unexpected_single_byte(&[0x7f]));
        assert!(!is_unexpected_single_byte(&[0x1b, 0x5b, 0x4d, 0xff]));
        assert!(!is_unexpected_single_byte("中".as_bytes()));
        assert!(!is_unexpected_single_byte(&[]));
    }

    /// 对照 `testCtrlLettersBecomeTerminalControlBytes`。
    #[test]
    fn ctrl_letters_become_terminal_control_bytes() {
        for (key, byte) in [
            ("a", 0x01u8),
            ("c", 0x03),
            ("e", 0x05),
            ("l", 0x0c),
            ("n", 0x0e),
            ("p", 0x10),
            ("r", 0x12),
        ] {
            assert_eq!(control_byte(key), Some(byte), "Ctrl+{key}");
        }
        // 大写字母走同一条路径。
        assert_eq!(control_byte("C"), Some(0x03));
    }

    /// 对照 `testControlPunctuationAndAlreadyEncodedBytes`。
    #[test]
    fn control_punctuation_and_already_encoded_bytes() {
        assert_eq!(control_byte(" "), Some(0x00));
        assert_eq!(control_byte("["), Some(0x1b));
        assert_eq!(control_byte("\\"), Some(0x1c));
        assert_eq!(control_byte("]"), Some(0x1d));
        assert_eq!(control_byte("^"), Some(0x1e));
        assert_eq!(control_byte("~"), Some(0x1e));
        assert_eq!(control_byte("_"), Some(0x1f));
        assert_eq!(control_byte("/"), Some(0x1f));
        assert_eq!(control_byte("`"), Some(0x00));
        assert_eq!(control_byte("?"), Some(0x7f));
        assert_eq!(control_byte("\u{03}"), Some(0x03));
        assert_eq!(BACKSPACE_BYTE, 0x7f);
        assert_eq!(control_byte("中"), None);
    }

    /// 对照 `testCtrlDigitsMatchXtermC0Aliases`。
    #[test]
    fn ctrl_digits_match_xterm_c0_aliases() {
        for (key, byte) in [
            ("2", 0x00u8),
            ("3", 0x1b),
            ("4", 0x1c),
            ("5", 0x1d),
            ("6", 0x1e),
            ("7", 0x1f),
            ("8", 0x7f),
        ] {
            assert_eq!(control_byte(key), Some(byte), "Ctrl+{key}");
        }
    }

    #[test]
    fn unmapped_printable_characters_have_no_control_byte() {
        for key in ["1", "9", "0", "=", "-", "+", "'", ",", ";", "9"] {
            assert_eq!(control_byte(key), None, "Ctrl+{key} 不应有控制字节");
        }
    }

    /// 对照 `CmdEnterRoutingTests` 的三个断言。
    #[test]
    fn cmd_enter_routes_by_surface() {
        assert_eq!(
            cmd_enter_action(Surface::MainWindow),
            CmdEnterAction::ToggleTmuxZoom,
            "主窗口保持 tmux zoom"
        );
        assert_eq!(
            cmd_enter_action(Surface::AttentionPanel),
            CmdEnterAction::OpenReplyOverlay
        );
        assert_eq!(OVERLAY_IDENTIFIER, "muxterm.replyOverlay");
        assert_eq!(
            cmd_enter_action(Surface::ReplyOverlay),
            CmdEnterAction::CloseReplyOverlay,
            "overlay 再按一次关掉"
        );
    }
}
