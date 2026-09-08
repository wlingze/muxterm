//! TUI keyboard input encoding.
//!
//! Input is a frontend concern: the TUI turns logical keys into bytes for the
//! public FFI task/input surface.

/// Directional key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowDir {
    Up,
    Down,
    Left,
    Right,
}

/// Toolkit-neutral logical keyboard event used by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyEvent {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Ctrl(char),
    Alt(char),
    Function(u8),
    Arrow(ArrowDir),
}

/// Encode one logical key using xterm/VT100 conventions.
pub fn encode(event: &KeyEvent) -> Vec<u8> {
    match event {
        KeyEvent::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        KeyEvent::Enter => vec![b'\r'],
        KeyEvent::Tab => vec![b'\t'],
        KeyEvent::Backspace => vec![0x7f],
        KeyEvent::Escape => vec![0x1b],
        KeyEvent::Ctrl(c) => encode_ctrl(*c),
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

fn encode_ctrl(c: char) -> Vec<u8> {
    let ch = c.to_ascii_lowercase();
    if ch.is_ascii_lowercase() {
        vec![(ch as u8) & 0x1f]
    } else {
        match c {
            '@' | ' ' | '2' | '`' => vec![0x00],
            '[' | '3' => vec![0x1b],
            '\\' | '4' => vec![0x1c],
            ']' | '5' => vec![0x1d],
            '^' | '~' | '6' => vec![0x1e],
            '_' | '/' | '7' => vec![0x1f],
            '?' | '8' => vec![0x7f],
            _ => vec![(c as u8) & 0x1f],
        }
    }
}

fn encode_function(n: u8) -> Vec<u8> {
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
    fn encode_char_and_basic_keys() {
        assert_eq!(encode(&KeyEvent::Char('a')), b"a");
        assert_eq!(encode(&KeyEvent::Char('中')), "中".as_bytes());
        assert_eq!(encode(&KeyEvent::Enter), b"\r");
        assert_eq!(encode(&KeyEvent::Tab), b"\t");
        assert_eq!(encode(&KeyEvent::Escape), b"\x1b");
        assert_eq!(encode(&KeyEvent::Backspace), b"\x7f");
    }

    #[test]
    fn encode_ctrl_punctuation_and_letters() {
        let cases: &[(char, u8)] = &[
            ('c', 0x03),
            ('C', 0x03),
            ('@', 0x00),
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('~', 0x1e),
            ('_', 0x1f),
            ('?', 0x7f),
            ('/', 0x1f),
            ('2', 0x00),
            ('3', 0x1b),
            ('8', 0x7f),
        ];
        for (key, byte) in cases {
            assert_eq!(encode(&KeyEvent::Ctrl(*key)), vec![*byte], "Ctrl+{key}");
        }
    }

    #[test]
    fn encode_alt_arrows_and_function_keys() {
        assert_eq!(encode(&KeyEvent::Alt('n')), b"\x1bn");
        assert_eq!(encode(&KeyEvent::Alt('中')), "\x1b中".as_bytes());
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Up)), b"\x1b[A");
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Down)), b"\x1b[B");
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Right)), b"\x1b[C");
        assert_eq!(encode(&KeyEvent::Arrow(ArrowDir::Left)), b"\x1b[D");
        assert_eq!(encode(&KeyEvent::Function(1)), b"\x1bOP");
        assert_eq!(encode(&KeyEvent::Function(12)), b"\x1b[24~");
        assert!(encode(&KeyEvent::Function(13)).is_empty());
    }
}
