//! 终端里的链接命中：Command+点击要打开浏览器（用户报障「浏览器 cmd+点击
//! 可以打开才行」）。
//!
//! 两级来源，与 Linux `terminal/pane_view.rs::open_url_at` 一致：
//! 1. OSC 8 超链接（单元格自带 URI，优先）；由 `terminal/parser.rs` 记录；
//! 2. 纯文本里的 `http(s)://…` 正则匹配。
//!
//! 这里是纯函数：输入一行字符 + 光标列，输出要打开的 URI。

use std::sync::OnceLock;

use regex::Regex;

/// 行内 URL 的正则：`https?://` 之后不接受空白与引号/尖括号。
///
/// 与 Linux 侧 `vte4::Regex::for_match(r#"https?://[^\s<>"']+"#)` 同义；
/// 反引号也排除，避免把 Markdown 行内代码的收尾符号吃进去。
fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s<>"'`]+"#).expect("URL 正则必须能编译"))
}

/// 结尾标点不算 URL 的一部分（iTerm2 / SwiftTerm 的 `checkMatchAt` 同行为）。
fn is_trailing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '」' | '』'
    )
}

/// 括号只有在不配对时才当标点剥掉（`https://x/y_(z)` 里的 `)` 属于链接）。
fn is_unbalanced_closer(url: &[char], ch: char) -> bool {
    let opener = match ch {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => return false,
    };
    let opens = url.iter().filter(|c| **c == opener).count();
    let closes = url.iter().filter(|c| **c == ch).count();
    closes > opens
}

/// 这一列是否落在某条 URL 上；是则返回修剪过的 URI。
///
/// `line` 是整行字符（一格一个字符，宽字符的占位格已由调用方换成空格）。
pub fn url_at(line: &[char], col: usize) -> Option<String> {
    if line.is_empty() || col >= line.len() {
        return None;
    }
    let text: String = line.iter().collect();
    // 字符下标 ↔ 字节偏移：regex 给的是字节位置。
    let mut byte_starts = Vec::with_capacity(line.len());
    let mut offset = 0usize;
    for ch in line {
        byte_starts.push(offset);
        offset += ch.len_utf8();
    }
    let byte_to_char = |byte: usize| -> Option<usize> {
        if byte == text.len() {
            Some(line.len())
        } else {
            byte_starts.binary_search(&byte).ok()
        }
    };

    for matched in url_regex().find_iter(&text) {
        let Some(start) = byte_to_char(matched.start()) else {
            continue;
        };
        let Some(mut end) = byte_to_char(matched.end()) else {
            continue;
        };
        while end > start {
            let last = line[end - 1];
            if is_trailing_punctuation(last) || is_unbalanced_closer(&line[start..end], last) {
                end -= 1;
            } else {
                break;
            }
        }
        if col >= start && col < end {
            return Some(line[start..end].iter().collect());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    /// 点中 URL 的任意一列都要打开整条链接。
    #[test]
    fn a_url_under_the_cursor_is_returned_whole() {
        let line = chars("see https://example.com/a?b=1 now");
        for col in 4..=28 {
            assert_eq!(
                url_at(&line, col).as_deref(),
                Some("https://example.com/a?b=1"),
                "第 {col} 列"
            );
        }
        assert_eq!(url_at(&line, 0), None, "URL 之前的文字不是链接");
        assert_eq!(url_at(&line, 29), None, "URL 之后的空格不是链接");
    }

    /// 句尾标点不算链接（否则浏览器会收到 `…/x).`）。
    #[test]
    fn trailing_punctuation_is_trimmed() {
        let line = chars("(see https://example.com/x).");
        let url = url_at(&line, 10).expect("必须命中");
        assert_eq!(url, "https://example.com/x");

        let quoted = chars("open 'https://example.com/q', please");
        assert_eq!(url_at(&quoted, 8).as_deref(), Some("https://example.com/q"));
    }

    /// 配对的括号属于链接本身（维基那种 `Foo_(bar)`）。
    #[test]
    fn balanced_brackets_stay_in_the_url() {
        let line = chars("https://en.wikipedia.org/wiki/Foo_(bar)");
        assert_eq!(
            url_at(&line, 5).as_deref(),
            Some("https://en.wikipedia.org/wiki/Foo_(bar)")
        );
    }

    /// 只有 http/https 才算；普通命令与文件路径不打开。
    #[test]
    fn only_http_urls_match() {
        let line = chars("cargo test --package muxterm ./target/debug/x");
        assert_eq!(url_at(&line, 3), None);
        assert_eq!(url_at(&line, 30), None);
        assert_eq!(url_at(&[], 0), None);
    }

    /// 中文行里的 URL 列号按「格」算，不按字节（宽字符不能把命中算歪）。
    #[test]
    fn wide_characters_keep_the_column_mapping() {
        let line = chars("打开 https://example.com/中文 看看");
        let url = url_at(&line, 5).expect("必须命中");
        assert_eq!(url, "https://example.com/中文");
        assert_eq!(url_at(&line, 1), None, "宽字符本身不是链接");
    }
}
