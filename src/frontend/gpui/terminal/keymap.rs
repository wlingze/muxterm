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
        if !text.is_empty() {
            return Some(prefix_alt(text.as_bytes().to_vec(), press.alt));
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
}
