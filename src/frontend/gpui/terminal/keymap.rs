//! 键盘 → 终端字节。纯函数，不依赖 UI 框架，便于测试。

/// 一次按键，字段从平台层翻译而来。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPress {
    /// 键位名：`a`、`enter`、`up`、`f5`。
    pub key: String,
    /// 实际字符（含输入法提交结果）。没有可打印字符时为 `None`。
    pub key_char: Option<String>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

/// 把按键翻译成要写进 PTY 的字节。
///
/// 返回 `None` 表示这一下不归终端：命令键（Cmd/Ctrl+非字符）与未知键交给 chrome。
pub fn encode(press: &KeyPress, app_cursor_keys: bool) -> Option<Vec<u8>> {
    if press.cmd {
        return None;
    }

    let key = press.key.as_str();
    // xterm 修饰键参数：1 + (shift=1 | alt=2 | ctrl=4)。
    let modifier = 1 + u8::from(press.shift) + 2 * u8::from(press.alt) + 4 * u8::from(press.ctrl);

    if let Some(sequence) = special_key(key, modifier, app_cursor_keys) {
        return Some(prefix_alt(sequence, press.alt));
    }

    if press.ctrl {
        if let Some(byte) = control_byte(key) {
            return Some(vec![byte]);
        }
    }

    if let Some(text) = &press.key_char {
        // 输入法占位符（U+FFFF/U+FFFE）不能进 PTY：远端 zsh 会显示成 <ffff>。
        // 对照 macOS `MuxTerminalView.keyDown` / `insertText`。
        use crate::frontend::gpui::model::ime::{key_dispatch, strip_placeholders, KeyDispatch};
        if key_dispatch(Some(text.as_str())) == KeyDispatch::Ignore {
            return None;
        }
        let cleaned = strip_placeholders(text);
        if !cleaned.is_empty() {
            return Some(prefix_alt(cleaned.into_bytes(), press.alt));
        }
    }

    // 没有 key_char（例如某些布局）时退回键位名的单字符。
    if key.chars().count() == 1 {
        let mut bytes = key.as_bytes().to_vec();
        if press.alt {
            bytes.insert(0, 0x1b);
        }
        return Some(bytes);
    }

    None
}

/// 这一下按键要不要交给平台输入法处理（可以开始/继续组词）。
///
/// 对照 macOS `MuxTerminalView.keyDown`：无修饰键的可打印文本走
/// `interpretKeyEvents`，由 `insertText(_:replacementRange:)` 提交；控制键、
/// 功能键、Alt/Ctrl 组合直接编码。
///
/// 为什么不能直接编码：中文输入法的首字母 `n`、`i` 也是普通可打印字符。
/// 前端一旦把它编码进 PTY 并消费按键，macOS 的 `inputContext` 就收不到
/// 这次 keyDown，组词根本起不来，用户只能打出字母（或什么都打不出）。
pub fn defers_to_ime(press: &KeyPress) -> bool {
    if press.cmd || press.ctrl || press.alt {
        return false;
    }
    let Some(text) = press.key_char.as_deref() else {
        return false;
    };
    use crate::frontend::gpui::model::ime::{key_dispatch, KeyDispatch};
    !text.is_empty()
        && text.chars().all(|ch| !ch.is_control())
        && key_dispatch(Some(text)) != KeyDispatch::Ignore
}

fn prefix_alt(mut bytes: Vec<u8>, alt: bool) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let lower = ch.to_ascii_lowercase();
    match lower {
        'a'..='z' => Some(lower as u8 - b'a' + 1),
        '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        ' ' => Some(0x00),
        _ => None,
    }
}

fn special_key(key: &str, modifier: u8, app_cursor_keys: bool) -> Option<Vec<u8>> {
    let plain = modifier == 1;
    let cursor = |normal: &str, application: &str, final_byte: char| -> Vec<u8> {
        if plain {
            if app_cursor_keys {
                application.as_bytes().to_vec()
            } else {
                normal.as_bytes().to_vec()
            }
        } else {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        }
    };

    let sequence: Vec<u8> = match key {
        "enter" | "return" => vec![0x0d],
        "tab" => {
            if modifier == 2 {
                b"\x1b[Z".to_vec()
            } else {
                vec![0x09]
            }
        }
        "backspace" => vec![0x7f],
        "escape" => vec![0x1b],
        "space" => vec![b' '],
        "up" => cursor("\x1b[A", "\x1bOA", 'A'),
        "down" => cursor("\x1b[B", "\x1bOB", 'B'),
        "right" => cursor("\x1b[C", "\x1bOC", 'C'),
        "left" => cursor("\x1b[D", "\x1bOD", 'D'),
        "home" => cursor("\x1b[H", "\x1bOH", 'H'),
        "end" => cursor("\x1b[F", "\x1bOF", 'F'),
        "insert" => format!("\x1b[2;{modifier}~").into_bytes(),
        "delete" => format!("\x1b[3;{modifier}~").into_bytes(),
        "pageup" => format!("\x1b[5;{modifier}~").into_bytes(),
        "pagedown" => format!("\x1b[6;{modifier}~").into_bytes(),
        "f1" => format!("\x1b[1;{modifier}P").into_bytes(),
        "f2" => format!("\x1b[1;{modifier}Q").into_bytes(),
        "f3" => format!("\x1b[1;{modifier}R").into_bytes(),
        "f4" => format!("\x1b[1;{modifier}S").into_bytes(),
        "f5" => format!("\x1b[15;{modifier}~").into_bytes(),
        "f6" => format!("\x1b[17;{modifier}~").into_bytes(),
        "f7" => format!("\x1b[18;{modifier}~").into_bytes(),
        "f8" => format!("\x1b[19;{modifier}~").into_bytes(),
        "f9" => format!("\x1b[20;{modifier}~").into_bytes(),
        "f10" => format!("\x1b[21;{modifier}~").into_bytes(),
        "f11" => format!("\x1b[23;{modifier}~").into_bytes(),
        "f12" => format!("\x1b[24;{modifier}~").into_bytes(),
        _ => return None,
    };
    Some(sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: &str) -> KeyPress {
        KeyPress {
            key: key.to_string(),
            key_char: Some(key.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn plain_character_goes_through() {
        assert_eq!(encode(&press("a"), false), Some(b"a".to_vec()));
    }

    /// 对照 `CmdEnterKeyE2ETests::testImePlaceholderNeverReachesRemoteShell`。
    #[test]
    fn ime_placeholders_never_become_bytes() {
        let mut placeholder = press("a");
        placeholder.key_char = Some("\u{ffff}".into());
        assert_eq!(encode(&placeholder, false), None);
        placeholder.key_char = Some("\u{fffe}\u{ffff}".into());
        assert_eq!(encode(&placeholder, false), None);

        // 混了真字符：只丢掉占位符。
        let mut mixed = press("a");
        mixed.key_char = Some("中\u{ffff}文".into());
        assert_eq!(
            encode(&mixed, false).unwrap(),
            "中文".as_bytes().to_vec(),
            "IME commit must retain valid Chinese text"
        );

        // 普通 IME 提交原样通过。
        let mut chinese = press("a");
        chinese.key_char = Some("中文".into());
        assert_eq!(encode(&chinese, false).unwrap(), "中文".as_bytes().to_vec());

        // 空字符按键退回键位名，仍能敲出普通字母。
        let mut empty = press("a");
        empty.key_char = Some(String::new());
        assert_eq!(encode(&empty, false), Some(vec![b'a']));
    }

    /// 对照 `TerminalInputEncodingTests::testControlBytes`。
    #[test]
    fn control_letters_become_control_bytes() {
        let mut keys = press("c");
        keys.ctrl = true;
        assert_eq!(encode(&keys, false), Some(vec![0x03]));
    }

    #[test]
    fn alt_prefixes_escape() {
        let mut keys = press("b");
        keys.alt = true;
        assert_eq!(encode(&keys, false), Some(vec![0x1b, b'b']));
    }

    #[test]
    fn cmd_is_left_to_the_chrome() {
        let mut keys = press("t");
        keys.cmd = true;
        assert_eq!(encode(&keys, false), None);
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        let mut keys = press("up");
        keys.key_char = None;
        assert_eq!(encode(&keys, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(&keys, true), Some(b"\x1bOA".to_vec()));
    }

    #[test]
    fn ctrl_arrow_adds_modifier_param() {
        let mut keys = press("up");
        keys.key_char = None;
        keys.ctrl = true;
        assert_eq!(encode(&keys, false), Some(b"\x1b[1;5A".to_vec()));
    }

    #[test]
    fn enter_backspace_escape() {
        let mut enter = press("enter");
        enter.key_char = None;
        assert_eq!(encode(&enter, false), Some(vec![0x0d]));
        let mut backspace = press("backspace");
        backspace.key_char = None;
        assert_eq!(encode(&backspace, false), Some(vec![0x7f]));
        let mut escape = press("escape");
        escape.key_char = None;
        assert_eq!(encode(&escape, false), Some(vec![0x1b]));
    }

    #[test]
    fn shift_tab_is_backtab() {
        let mut keys = press("tab");
        keys.key_char = None;
        keys.shift = true;
        assert_eq!(encode(&keys, false), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn function_keys() {
        let mut keys = press("f5");
        keys.key_char = None;
        assert_eq!(encode(&keys, false), Some(b"\x1b[15;1~".to_vec()));
    }

    #[test]
    fn ime_committed_text_is_forwarded() {
        let keys = KeyPress {
            key: "process".into(),
            key_char: Some("你好".into()),
            ..Default::default()
        };
        assert_eq!(encode(&keys, false), Some("你好".as_bytes().to_vec()));
    }

    /// 回归（中文输入）：拼音首字母必须留给平台输入法，不能直接编码。
    ///
    /// 对照 macOS `MuxTerminalView.keyDown` 的 `interpretKeyEvents` 分支。
    #[test]
    fn pinyin_letters_are_left_to_the_ime() {
        assert!(defers_to_ime(&press("n")));
        assert!(defers_to_ime(&press("i")));
        // Shift+字母仍是可打印文本（大写），同样交给输入法。
        let mut upper = press("n");
        upper.shift = true;
        upper.key_char = Some("N".into());
        assert!(defers_to_ime(&upper));
        // 空格是拼音选字的提交键，必须交给输入法。
        let mut space = press("space");
        space.key_char = Some(" ".into());
        assert!(defers_to_ime(&space));
        // 已提交的中文（含非 BMP）也应该走输入法边界。
        let mut chinese = press("process");
        chinese.key_char = Some("你好🦀".into());
        assert!(defers_to_ime(&chinese));
    }

    /// 控制键、功能键、Alt/Ctrl 组合必须继续直接编码，不能依赖输入法。
    #[test]
    fn control_function_and_option_keys_bypass_the_ime() {
        let mut enter = press("enter");
        enter.key_char = None;
        assert!(!defers_to_ime(&enter));
        let mut tab = press("tab");
        tab.key_char = None;
        assert!(!defers_to_ime(&tab));
        let mut escape = press("escape");
        escape.key_char = None;
        assert!(!defers_to_ime(&escape));
        let mut arrow = press("up");
        arrow.key_char = None;
        assert!(!defers_to_ime(&arrow));

        let mut ctrl_n = press("n");
        ctrl_n.ctrl = true;
        assert!(!defers_to_ime(&ctrl_n));
        let mut alt_n = press("n");
        alt_n.alt = true;
        assert!(!defers_to_ime(&alt_n));
        let mut cmd_n = press("n");
        cmd_n.cmd = true;
        assert!(!defers_to_ime(&cmd_n));

        // 控制字符（回车在部分布局里带 `\r`）不能送进输入法。
        let mut cr = press("enter");
        cr.key_char = Some("\r".into());
        assert!(!defers_to_ime(&cr));
        // 占位符按键不能交给输入法（否则会插进 <ffff>），走 `encode` 的丢弃逻辑。
        let mut placeholder = press("process");
        placeholder.key_char = Some("\u{ffff}".into());
        assert!(!defers_to_ime(&placeholder));
    }
}
