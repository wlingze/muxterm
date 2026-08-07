//! 输入编码：键盘事件 → 写入 pty 的字节序列。

/// 方向键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowDir {
    Up,
    Down,
    Left,
    Right,
}

/// 抽象键盘事件（与 GUI toolkit 解耦）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyEvent {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    /// Ctrl+字母（如 C-c、C-d、C-z）。
    Ctrl(char),
    /// Alt+字符（ESC 前缀）。
    Alt(char),
    /// F1–F12（`1..=12`）。
    Function(u8),
    Arrow(ArrowDir),
}

/// 将键盘事件编码为 pty 字节流（xterm / VT100 兼容）。
pub fn encode(event: &KeyEvent) -> Vec<u8> {
    match event {
        KeyEvent::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        KeyEvent::Enter => vec![b'\r'],
        KeyEvent::Tab => vec![b'\t'],
        // 多数终端对 Backspace 发 DEL (0x7f)
        KeyEvent::Backspace => vec![0x7f],
        KeyEvent::Escape => vec![0x1b],
        KeyEvent::Ctrl(c) => {
            let ch = c.to_ascii_lowercase();
            if ch.is_ascii_lowercase() {
                vec![(ch as u8) & 0x1f]
            } else if *c == '@' || *c == ' ' {
                vec![0x00]
            } else if *c == '[' {
                vec![0x1b]
            } else if *c == '\\' {
                vec![0x1c]
            } else if *c == ']' {
                vec![0x1d]
            } else if *c == '^' {
                vec![0x1e]
            } else if *c == '_' {
                vec![0x1f]
            } else if *c == '?' {
                vec![0x7f]
            } else if *c == '/' {
                // xterm/常见终端约定：Ctrl+/ 与 Ctrl+_ 一样发 US (0x1f)，
                // 而不是按 ASCII 掩码落成 SI (0x0f)。
                vec![0x1f]
            } else if *c == '2' {
                vec![0x00]
            } else if *c == '3' {
                vec![0x1b]
            } else if *c == '4' {
                vec![0x1c]
            } else if *c == '5' {
                vec![0x1d]
            } else if *c == '6' {
                vec![0x1e]
            } else if *c == '7' {
                vec![0x1f]
            } else if *c == '8' {
                vec![0x7f]
            } else {
                // 非标准 Ctrl 组合：尽量按 ASCII 控制位处理
                vec![(*c as u8) & 0x1f]
            }
        }
        KeyEvent::Alt(c) => {
            let mut out = vec![0x1b];
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            out
        }
        KeyEvent::Function(n) => encode_function(*n),
        KeyEvent::Arrow(dir) => match dir {
            ArrowDir::Up => b"\x1b[A".to_vec(),
            ArrowDir::Down => b"\x1b[B".to_vec(),
            ArrowDir::Right => b"\x1b[C".to_vec(),
            ArrowDir::Left => b"\x1b[D".to_vec(),
        },
    }
}

fn encode_function(n: u8) -> Vec<u8> {
    // xterm 默认：F1–F4 用 SS3，F5–F12 用 CSI
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_char_ascii() {
        assert_eq!(encode(&KeyEvent::Char('a')), b"a");
    }

    #[test]
    fn encode_char_unicode() {
        assert_eq!(encode(&KeyEvent::Char('中')), "中".as_bytes());
    }

    #[test]
    fn encode_enter_tab_escape_backspace() {
        assert_eq!(encode(&KeyEvent::Enter), b"\r");
        assert_eq!(encode(&KeyEvent::Tab), b"\t");
        assert_eq!(encode(&KeyEvent::Escape), b"\x1b");
        assert_eq!(encode(&KeyEvent::Backspace), b"\x7f");
    }

    #[test]
    fn encode_ctrl_letters() {
        assert_eq!(encode(&KeyEvent::Ctrl('c')), b"\x03");
        assert_eq!(encode(&KeyEvent::Ctrl('d')), b"\x04");
        assert_eq!(encode(&KeyEvent::Ctrl('z')), b"\x1a");
        assert_eq!(encode(&KeyEvent::Ctrl('C')), b"\x03");
    }

    /// xterm 控制位对照表：Ctrl+标点/空格/字母都必须落到对应 C0 控制位。
    /// 参考 xterm ctlseqs 的键盘编码约定。
    #[test]
    fn encode_ctrl_punctuation_matches_xterm_c0() {
        let cases: &[(char, u8)] = &[
            ('@', 0x00),
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('~', 0x1e),
            ('_', 0x1f),
            ('`', 0x00),
            ('?', 0x7f),
            ('/', 0x1f), // Ctrl-/ 等同 Ctrl+_
            ('2', 0x00),
            ('3', 0x1b),
            ('4', 0x1c),
            ('5', 0x1d),
            ('6', 0x1e),
            ('7', 0x1f),
            ('8', 0x7f),
        ];
        for (key, byte) in cases {
            assert_eq!(encode(&KeyEvent::Ctrl(*key)), vec![*byte], "Ctrl+{key}");
        }
    }

    #[test]
    fn encode_alt_char() {
        assert_eq!(encode(&KeyEvent::Alt('n')), b"\x1bn");
        // Alt+非 ASCII：ESC 前缀 + UTF-8 原文
        assert_eq!(encode(&KeyEvent::Alt('中')), "\x1b中".as_bytes());
    }

    #[test]
    fn encode_arrows() {
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Up)), b"\x1b[A");
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Down)), b"\x1b[B");
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Right)), b"\x1b[C");
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Left)), b"\x1b[D");
    }

    #[test]
    fn encode_function_keys() {
        // xterm 默认功能键表：F1-F4 用 SS3，F5-F12 用 CSI ~
        let expected: &[&[u8]] = &[
            b"\x1bOP", b"\x1bOQ", b"\x1bOR", b"\x1bOS", b"\x1b[15~", b"\x1b[17~",
            b"\x1b[18~", b"\x1b[19~", b"\x1b[20~", b"\x1b[21~", b"\x1b[23~", b"\x1b[24~",
        ];
        for (i, exp) in expected.iter().enumerate() {
            assert_eq!(
                encode(&KeyEvent::Function((i + 1) as u8)),
                *exp,
                "F{}",
                i + 1
            );
        }
        assert!(encode(&KeyEvent::Function(0)).is_empty());
        assert!(encode(&KeyEvent::Function(13)).is_empty());
    }
}
