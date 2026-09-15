//! 区分「真的在打字」和鼠标/焦点上报。
//!
//! TUI agent 打开 mouse tracking 后，点击、滚轮都会写成 CSI 进 PTY。
//! 那不是一轮新的生成，不能把 idle 打成 working。

/// 整段输入都是鼠标上报或 focus in/out 时为 true。
pub fn is_pointer_or_focus_input(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < data.len() {
        if data[i] != 0x1b {
            return false;
        }
        i += 1;
        if i >= data.len() || data[i] != b'[' {
            return false;
        }
        i += 1;
        if i >= data.len() {
            return false;
        }
        match data[i] {
            b'I' | b'O' => i += 1,
            b'M' => {
                // X10：ESC [ M Cb Cx Cy
                i += 1;
                i += 3;
                if i > data.len() {
                    return false;
                }
            }
            b'<' => {
                // SGR：ESC [ < ... M/m
                i += 1;
                while i < data.len() && data[i] != b'M' && data[i] != b'm' {
                    i += 1;
                }
                if i >= data.len() {
                    return false;
                }
                i += 1;
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::is_pointer_or_focus_input;

    #[test]
    fn sgr_click_and_wheel_are_pointer_input() {
        assert!(is_pointer_or_focus_input(b"\x1b[<0;12;34M"));
        assert!(is_pointer_or_focus_input(b"\x1b[<0;12;34m"));
        assert!(is_pointer_or_focus_input(b"\x1b[<64;5;6m"));
        assert!(is_pointer_or_focus_input(b"\x1b[<0;12;34M\x1b[<0;12;34m"));
    }

    #[test]
    fn x10_mouse_and_focus_are_pointer_input() {
        assert!(is_pointer_or_focus_input(b"\x1b[M #!"));
        assert!(is_pointer_or_focus_input(b"\x1b[I"));
        assert!(is_pointer_or_focus_input(b"\x1b[O"));
    }

    #[test]
    fn typed_keys_are_not_pointer_input() {
        assert!(!is_pointer_or_focus_input(b"hello"));
        assert!(!is_pointer_or_focus_input(b"\r"));
        assert!(!is_pointer_or_focus_input(b"\x1b[A"));
        assert!(!is_pointer_or_focus_input(b"a\x1b[<0;1;1M"));
        assert!(!is_pointer_or_focus_input(b""));
    }
}
