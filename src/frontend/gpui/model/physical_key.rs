//! 物理键位 → US QWERTY 字符。
//!
//! 为什么需要这一层：macOS 的菜单 key equivalent 和 gpui 的 `Keystroke.key`
//! 都按**当前键盘布局产出的字符**匹配。Colemak / Dvorak / AZERTY 把字母挪了
//! 位置，于是「键盘上印着 `D` 的那个键 + Cmd」产出的字符是 `s`：
//!
//! | 物理键（键帽） | QWERTY 字符 | Colemak 产出 | Cmd 组合落点 |
//! | --- | --- | --- | --- |
//! | T | `t` | `g` | 没有任何绑定 → 静默丢弃 |
//! | D | `d` | `s` | 落到「纵向分屏」→ 方向错 |
//! | S | `s` | `r` | 没有任何绑定 → 静默丢弃 |
//! | C / V / A / Z / X | 同字母 | 同字母 | 正常 |
//!
//! 这就是用户报的「cmd-d / cmd-t / cmd-s 全失败，别的 Cmd 组合却好使」：
//! Colemak 没挪 C/V/A/Z/X，只挪了 D/T/S 这些。
//!
//! 终端的业界惯例是按**物理键位**派发快捷键：
//! - Ghostty 用 W3C key code（物理键）派发，只在给原生菜单做 key equivalent
//!   时才把物理键翻译成布局字符
//!   <https://github.com/ghostty-org/ghostty/pull/7320>
//! - Zed 明确「先派发快捷键，再交给输入法」，并且同样按 key code 查表
//!   <https://zed.dev/blog/keyboard-localization>
//!
//! 所以这里把 macOS 虚拟键码（`kVK_ANSI_*`）翻成 US QWERTY 字符，让 Cmd 组合
//! 按物理位置匹配；布局字符作为兜底（方向键、中文输入法的全角括号等）。

/// macOS 虚拟键码 → US QWERTY 字符（单字符，不含 Shift 变体）。
///
/// 数字取自 Carbon 的 `kVK_ANSI_*` 常量（`HIToolbox/Events.h`）。只收本前端
/// 关心的键：字母、数字、标点；功能键交给 `Keystroke.key` 的语义名。
pub fn us_char(key_code: u16) -> Option<&'static str> {
    Some(match key_code {
        // 字母行
        0 => "a",
        1 => "s",
        2 => "d",
        3 => "f",
        4 => "h",
        5 => "g",
        6 => "z",
        7 => "x",
        8 => "c",
        9 => "v",
        11 => "b",
        12 => "q",
        13 => "w",
        14 => "e",
        15 => "r",
        16 => "y",
        17 => "t",
        31 => "o",
        32 => "u",
        34 => "i",
        35 => "p",
        37 => "l",
        38 => "j",
        40 => "k",
        45 => "n",
        46 => "m",
        // 数字行
        18 => "1",
        19 => "2",
        20 => "3",
        21 => "4",
        22 => "6",
        23 => "5",
        24 => "=",
        25 => "9",
        26 => "7",
        27 => "-",
        28 => "8",
        29 => "0",
        // 标点
        30 => "]",
        33 => "[",
        39 => "'",
        41 => ";",
        42 => "\\",
        43 => ",",
        44 => "/",
        47 => ".",
        50 => "`",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户报障现场：Colemak 布局，按键盘上印着 D/T/S 的三个物理键。
    ///
    /// 对照 `KeyBindingsTests.testCommandShortcutsUsePrintedKeyOnNonQwertyLayout`。
    #[test]
    fn printed_keys_map_to_their_label_not_the_layout_char() {
        // keyCode 2 = QWERTY D 位，Colemak 下产出 's'
        assert_eq!(us_char(2), Some("d"));
        // keyCode 17 = QWERTY T 位，Colemak 下产出 'g'
        assert_eq!(us_char(17), Some("t"));
        // keyCode 1 = QWERTY S 位，Colemak 下产出 'r'
        assert_eq!(us_char(1), Some("s"));
    }

    /// 没挪过位置的字母两种布局一样，不能因为加了物理键表而改变行为。
    #[test]
    fn unmoved_letters_keep_their_identity() {
        for (code, expected) in [
            (8u16, "c"),
            (9, "v"),
            (0, "a"),
            (6, "z"),
            (7, "x"),
            (13, "w"),
            (12, "q"),
        ] {
            assert_eq!(us_char(code), Some(expected), "keyCode {code}");
        }
    }

    /// 数字键：Cmd-1..9 切 tab 依赖这张表。
    #[test]
    fn digit_row_round_trips() {
        for (code, expected) in [
            (18u16, "1"),
            (19, "2"),
            (20, "3"),
            (21, "4"),
            (23, "5"),
            (22, "6"),
            (26, "7"),
            (28, "8"),
            (25, "9"),
            (29, "0"),
        ] {
            assert_eq!(us_char(code), Some(expected), "keyCode {code}");
        }
    }

    /// 方向键 / 功能键没有可打印字符，必须返回 `None` 让调用方回落到
    /// `Keystroke.key` 的语义名（`left` / `right` / `escape` …）。
    #[test]
    fn non_printing_keys_have_no_char() {
        for code in [123u16, 124, 125, 126, 53, 36, 48, 51, 122, 120] {
            assert_eq!(us_char(code), None, "keyCode {code}");
        }
    }

    /// 表里不允许出现重复键码（写错会静默丢一个键）。
    #[test]
    fn table_has_no_duplicate_codes() {
        let mut seen = std::collections::HashSet::new();
        for code in 0u16..=127 {
            if us_char(code).is_some() {
                assert!(seen.insert(code), "duplicate keyCode {code}");
            }
        }
    }
}
