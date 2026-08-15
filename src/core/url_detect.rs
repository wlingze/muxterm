//! URL 检测与打开（LINUX-PLAN §4.2）。
//!
//! core 只做纯函数：`url_at_line` 从一行文本的列位置找 URL；`link_at` 从
//! OSC 8 的 Cell.link 取 URI。打开走 `UrlOpener` trait，测试注入 Recording，
//! 生产实现禁止真的调浏览器（由 GTK 层接 `gio::AppInfo`）。

use std::cell::RefCell;
use std::rc::Rc;

use crate::core::protocol::terminal::emulate::TerminalState;

/// 从一行文本的列位置找 URL（大小写不敏感 scheme）。
///
/// 规则：只认 `scheme://` 开头（https/http 等）；括号包裹的 URL 剥掉尾部
/// 闭合括号；无 scheme 的 `example.com` 不算 URL（减少误开）。
pub fn url_at_line(line: &str, col: usize) -> Option<String> {
    if col >= line.len() {
        return None;
    }
    let bytes = line.as_bytes();
    // 从 col 向左找 URL 起点（scheme 边界）。
    let mut start = col;
    while start > 0 {
        let c = bytes[start - 1];
        if c.is_ascii_alphanumeric()
            || c == b':'
            || c == b'/'
            || c == b'.'
            || c == b'_'
            || c == b'-'
        {
            start -= 1;
        } else {
            break;
        }
    }
    let candidate = &line[start..];
    // 找 scheme:// 前缀。
    let lower = candidate.to_ascii_lowercase();
    let scheme_end = lower.find("://")?;
    let scheme = &lower[..scheme_end];
    if !scheme.chars().all(|c| c.is_ascii_alphanumeric()) || scheme.is_empty() {
        return None;
    }
    // 从 scheme 起点向右扫到空白，再剥尾部闭合括号/标点。
    let url_start = start;
    let mut end = url_start;
    while end < line.len() && !bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    while end > url_start + 3 {
        let c = bytes[end - 1];
        if c == b')'
            || c == b']'
            || c == b'}'
            || c == b','
            || c == b'.'
            || c == b';'
            || c == b'!'
            || c == b'?'
        {
            end -= 1;
        } else {
            break;
        }
    }
    let url = &line[url_start..end];
    if url.len() <= "://".len() + 1 {
        return None;
    }
    Some(url.to_string())
}

/// 从 TerminalState 的 OSC 8 链接取 URI（row/col 0 基）。
pub fn link_at(state: &TerminalState, row: usize, col: usize) -> Option<String> {
    state.cell(row, col).and_then(|c| c.link.clone())
}

/// URL 打开出口（测试注入 Recording，生产接 GTK）。
pub trait UrlOpener {
    fn open(&self, uri: &str);
}

/// 无操作 opener。
pub struct NullOpener;

impl UrlOpener for NullOpener {
    fn open(&self, _uri: &str) {}
}

/// 记录型 opener（测试断言 URI，禁止真开浏览器）。
#[derive(Clone, Default)]
pub struct RecordingOpener {
    pub opened: Rc<RefCell<Vec<String>>>,
}

impl RecordingOpener {
    pub fn new() -> Self {
        Self::default()
    }
}

impl UrlOpener for RecordingOpener {
    fn open(&self, uri: &str) {
        self.opened.borrow_mut().push(uri.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_at_line_finds_https_at_column() {
        let line = "see https://example.com/a now";
        let col = line.find("https").unwrap();
        assert_eq!(
            url_at_line(line, col),
            Some("https://example.com/a".to_string())
        );
        // 列在 URL 中间也能找到。
        let mid = line.find("example").unwrap();
        assert_eq!(
            url_at_line(line, mid),
            Some("https://example.com/a".to_string())
        );
    }

    #[test]
    fn url_at_line_strips_trailing_punctuation_and_parens() {
        assert_eq!(
            url_at_line("(https://example.com/a).", 1),
            Some("https://example.com/a".to_string())
        );
        assert_eq!(
            url_at_line("see https://example.com/a, ok", 4),
            Some("https://example.com/a".to_string())
        );
    }

    #[test]
    fn url_at_line_rejects_no_scheme() {
        assert_eq!(url_at_line("visit example.com now", 6), None);
        assert_eq!(url_at_line("plain text", 0), None);
    }

    #[test]
    fn link_at_reads_osc8_uri() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]8;;https://example.invalid/x\x1b\\hello");
        assert_eq!(
            link_at(&t, 0, 0),
            Some("https://example.invalid/x".to_string())
        );
        assert_eq!(link_at(&t, 0, 5), None);
    }

    /// S11：OSC 8 包着的 URL，Recording opener 收到一次。
    #[test]
    fn url_click_records_https_uri() {
        let mut t = TerminalState::new(80, 24);
        t.feed(b"\x1b]8;;https://example.invalid/x\x1b\\hello");
        let uri = link_at(&t, 0, 0).expect("OSC 8 应给 URI");
        let opener = RecordingOpener::new();
        opener.open(&uri);
        assert_eq!(
            *opener.opened.borrow(),
            vec!["https://example.invalid/x".to_string()]
        );
    }
}
