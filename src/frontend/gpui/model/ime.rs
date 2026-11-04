//! IME 边界规则：占位标量、空按键、候选窗锚点。
//!
//! 对照 macOS `Terminal/TerminalView.swift`（`MuxTerminalView.keyDown` /
//! `insertText` / `firstRect`）与 `AppE2ETests/CmdEnterKeyE2ETests.swift`。
//!
//! 为什么要有这一层：某些中文输入法把组合中的占位键作为 U+FFFF 放进
//! `characters`，SwiftTerm 会把它编码成 `EF BF BF`，远端 zsh 显示成
//! `<ffff>` 并污染命令行。gpui 侧同样必须在这条边界上过滤。

/// 输入法组合期的占位标量：U+FFFF（非字符）、U+FFFE（反向非字符）。
pub const PLACEHOLDER_SCALARS: [char; 2] = ['\u{ffff}', '\u{fffe}'];

/// 该标量是不是 IME 占位符。
pub fn is_placeholder(scalar: char) -> bool {
    PLACEHOLDER_SCALARS.contains(&scalar)
}

/// 整段文本是否**只**由占位符组成。空串不算（空串要走 `interpretKeyEvents`）。
pub fn is_placeholder_only(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_placeholder)
}

/// 去掉占位符后的文本。没有占位符时逐字节相等。
pub fn strip_placeholders(text: &str) -> String {
    if !text.chars().any(is_placeholder) {
        return text.to_string();
    }
    text.chars().filter(|ch| !is_placeholder(*ch)).collect()
}

/// IME 提交 → 要写进 PTY 的字节。占位符全部去掉；清空后返回 `None`，
/// 一个字节都不发（否则远端会看到 `<ffff>`）。
pub fn commit_bytes(text: &str) -> Option<Vec<u8>> {
    let cleaned = strip_placeholders(text);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.into_bytes())
    }
}

/// `keyDown` 在这一层的去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDispatch {
    /// 占位符按键：一个字节都不发，也不交给 IME（否则会插进 <ffff>）。
    Ignore,
    /// `characters` 为空：仍交给平台/IME，保持候选与提交事件正常工作。
    Interpret,
    /// 普通按键。
    Input,
}

/// 决定一次 `keyDown` 走哪条路。
pub fn key_dispatch(characters: Option<&str>) -> KeyDispatch {
    match characters {
        // 占位符只由占位符组成时才是占位按键；空串走 IME。
        Some(text) if is_placeholder_only(text) => KeyDispatch::Ignore,
        Some("") => KeyDispatch::Interpret,
        None | Some(_) => KeyDispatch::Input,
    }
}

/// 候选窗锚点（相对 pane 的像素坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaretRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// IME 候选窗应锚定到**下一个将要输入的字符**。
///
/// 光标停在最后一列时下一字符会软换行；仍返回最后一格会把候选窗挤到 pane
/// 右边缘并裁掉，所以退回上一行行首。
pub fn candidate_anchor(caret: CaretRect, cursor_x: usize, cols: usize) -> CaretRect {
    if cols > 0 && cursor_x + 1 >= cols {
        CaretRect {
            x: 0.0,
            y: (caret.y - caret.height).max(0.0),
            ..caret
        }
    } else {
        caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `CmdEnterKeyE2ETests::testImePlaceholderNeverReachesRemoteShell`。
    #[test]
    fn ime_placeholder_never_reaches_the_shell() {
        // characters 为空 → 交给 IME；只有占位符 → 丢弃。
        assert_eq!(key_dispatch(Some("")), KeyDispatch::Interpret);
        assert_eq!(key_dispatch(Some("\u{ffff}")), KeyDispatch::Ignore);
        assert_eq!(key_dispatch(Some("\u{fffe}")), KeyDispatch::Ignore);
        assert_eq!(key_dispatch(Some("\u{ffff}\u{fffe}")), KeyDispatch::Ignore);
        // 混了真字符就不是占位按键。
        assert_eq!(key_dispatch(Some("中")), KeyDispatch::Input);
        assert_eq!(key_dispatch(Some("a")), KeyDispatch::Input);
        assert_eq!(key_dispatch(None), KeyDispatch::Input);
    }

    /// 同上的 `insertText` 部分：提交文本要去掉占位符，但不许丢真字符。
    #[test]
    fn ime_commit_strips_placeholders_and_keeps_real_text() {
        assert_eq!(strip_placeholders("中文"), "中文");
        assert_eq!(strip_placeholders("\u{ffff}"), "");
        assert_eq!(strip_placeholders("中\u{ffff}文"), "中文");
        assert_eq!(strip_placeholders("a\u{fffe}b"), "ab");
        // 没有占位符时原样返回（包括代理对与非 BMP 字符）。
        assert_eq!(strip_placeholders("🦀ok"), "🦀ok");
        assert!(!is_placeholder_only(""));
        assert!(is_placeholder_only("\u{ffff}\u{fffe}"));
        assert!(!is_placeholder_only("中\u{ffff}"));
    }

    /// 提交路径：纯占位符不发字节，混排只丢占位符。
    #[test]
    fn commit_bytes_drop_placeholders() {
        assert_eq!(commit_bytes("\u{ffff}"), None);
        assert_eq!(commit_bytes(""), None);
        assert_eq!(commit_bytes("\u{fffe}\u{ffff}"), None);
        assert_eq!(commit_bytes("中文").unwrap(), "中文".as_bytes().to_vec());
        assert_eq!(
            commit_bytes("中\u{ffff}文").unwrap(),
            "中文".as_bytes().to_vec()
        );
    }

    /// 候选窗在最后一列要退到上一行行首，其余位置保持不动。
    #[test]
    fn candidate_anchor_wraps_at_the_last_column() {
        let caret = CaretRect {
            x: 720.0,
            y: 320.0,
            width: 8.0,
            height: 16.0,
        };
        let anchor = candidate_anchor(caret, 79, 80);
        assert_eq!(anchor.x, 0.0);
        assert_eq!(anchor.y, 304.0);
        assert_eq!(anchor.width, 8.0);
        assert_eq!(anchor.height, 16.0);

        // 差一格不算：只有 >= cols - 1 才回退。
        let mid = CaretRect {
            x: 100.0,
            y: 200.0,
            width: 8.0,
            height: 16.0,
        };
        assert_eq!(candidate_anchor(mid, 10, 80), mid);
        assert_eq!(candidate_anchor(mid, 78, 80), mid);
        // 第一行没有上一行可退：夹到 0。
        let top = CaretRect {
            x: 720.0,
            y: 0.0,
            width: 8.0,
            height: 16.0,
        };
        assert_eq!(candidate_anchor(top, 79, 80).y, 0.0);
        // cols 为 0（尚未拿到尺寸）时不回退。
        assert_eq!(candidate_anchor(caret, 79, 0), caret);
    }
}
