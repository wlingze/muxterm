//! 默认快捷键以 Colemak 字母为基准，切换布局后保持同手位。
//!
//! 用户确认的规则（2026-11-16）：Colemak ⌘D / ⌘S / ⌘T 分别对应
//! QWERTY ⌘G / ⌘D / ⌘F。这条规则适用于所有默认快捷键，而不是三个特例。
//! 普通打字仍用布局字符，绝不能经此表转换。

/// macOS ANSI 虚拟键码、QWERTY 标签与 Colemak 标签。
/// 数字、标点不换位；功能键由平台的语义名处理。
const KEYS: &[(u16, &str, &str)] = &[
    (0, "a", "a"),
    (1, "s", "r"),
    (2, "d", "s"),
    (3, "f", "t"),
    (4, "h", "h"),
    (5, "g", "d"),
    (6, "z", "z"),
    (7, "x", "x"),
    (8, "c", "c"),
    (9, "v", "v"),
    (11, "b", "b"),
    (12, "q", "q"),
    (13, "w", "w"),
    (14, "e", "f"),
    (15, "r", "p"),
    (16, "y", "j"),
    (17, "t", "g"),
    (18, "1", "1"),
    (19, "2", "2"),
    (20, "3", "3"),
    (21, "4", "4"),
    (22, "6", "6"),
    (23, "5", "5"),
    (24, "=", "="),
    (25, "9", "9"),
    (26, "7", "7"),
    (27, "-", "-"),
    (28, "8", "8"),
    (29, "0", "0"),
    (30, "]", "]"),
    (31, "o", "y"),
    (32, "u", "l"),
    (33, "[", "["),
    (34, "i", "u"),
    (35, "p", ";"),
    (37, "l", "i"),
    (38, "j", "n"),
    (39, "'", "'"),
    (40, "k", "e"),
    (41, ";", "o"),
    (42, "\\", "\\"),
    (43, ",", ","),
    (44, "/", "/"),
    (45, "n", "k"),
    (46, "m", "m"),
    (47, ".", "."),
    (50, "`", "`"),
];

pub fn us_char(key_code: u16) -> Option<&'static str> {
    KEYS.iter().find(|row| row.0 == key_code).map(|row| row.1)
}

/// 物理位置换成快捷键表使用的 Colemak 标签。
pub fn colemak_char(key_code: u16) -> Option<&'static str> {
    KEYS.iter().find(|row| row.0 == key_code).map(|row| row.2)
}

/// Colemak 标签对应的 macOS 物理键码（菜单与测试也使用这一份表）。
pub fn key_code(colemak: &str) -> Option<u16> {
    KEYS.iter().find(|row| row.2 == colemak).map(|row| row.0)
}

/// QWERTY 字符 → 同位置 Colemak 字符。
pub fn from_qwerty(key: &str) -> String {
    KEYS.iter()
        .find(|row| row.1 == key)
        .map(|row| row.2)
        .unwrap_or(key)
        .to_string()
}

/// Colemak 字符 → 同位置 QWERTY 字符。
pub fn to_qwerty(key: &str) -> String {
    KEYS.iter()
        .find(|row| row.2 == key)
        .map(|row| row.1)
        .unwrap_or(key)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户明确确认：Colemak 为基准，所有默认快捷键在 QWERTY 下同手位。
    #[test]
    fn colemak_is_the_anchor_not_qwerty() {
        for (code, colemak, qwerty) in [
            (5, "d", "g"),
            (2, "s", "d"),
            (3, "t", "f"),
            (15, "p", "r"),
            (14, "f", "e"),
            (45, "k", "n"),
            (37, "i", "l"),
        ] {
            assert_eq!(colemak_char(code), Some(colemak));
            assert_eq!(us_char(code), Some(qwerty));
            assert_eq!(key_code(colemak), Some(code));
            assert_eq!(to_qwerty(colemak), qwerty);
            assert_eq!(from_qwerty(qwerty), colemak);
        }
    }

    #[test]
    fn every_key_round_trips_without_duplicate_codes_or_labels() {
        let mut codes = std::collections::HashSet::new();
        let mut qwerty = std::collections::HashSet::new();
        let mut colemak = std::collections::HashSet::new();
        for &(code, q, c) in KEYS {
            assert!(codes.insert(code));
            assert!(qwerty.insert(q));
            assert!(colemak.insert(c));
            assert_eq!(from_qwerty(&to_qwerty(c)), c);
        }
        assert_eq!(
            colemak
                .iter()
                .filter(|key| key.chars().all(char::is_alphabetic))
                .count(),
            26
        );
    }

    #[test]
    fn digits_punctuation_and_function_keys_do_not_move() {
        for key in [
            "1", "0", "[", "]", "=", "-", ",", "/", "left", "enter", "escape",
        ] {
            assert_eq!(from_qwerty(key), key);
            assert_eq!(to_qwerty(key), key);
        }
        for code in [123, 124, 125, 126, 53, 36] {
            assert_eq!(colemak_char(code), None);
        }
    }
}
