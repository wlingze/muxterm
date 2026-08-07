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

    #[test]
    fn encode_alt_char() {
        assert_eq!(encode(&KeyEvent::Alt('n')), b"\x1bn");
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
        assert_eq!(encode(&KeyEvent::Function(1)), b"\x1bOP");
        assert_eq!(encode(&KeyEvent::Function(5)), b"\x1b[15~");
        assert_eq!(encode(&KeyEvent::Function(12)), b"\x1b[24~");
        assert!(encode(&KeyEvent::Function(0)).is_empty());
        assert!(encode(&KeyEvent::Function(13)).is_empty());
    }
}
