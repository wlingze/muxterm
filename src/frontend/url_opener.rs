//! Frontend-owned URL opening出口。
//!
//! Core 只负责 URL 文本和 OSC 8 链接的解析；真正的打开动作以及测试替身
//! 属于 frontend/platform，不应让 Core 持有 UI side effect trait。

use std::cell::RefCell;
use std::rc::Rc;

/// URL 打开出口（生产实现由具体 frontend 接入系统 API）。
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
    fn recording_opener_collects_uris() {
        let opener = RecordingOpener::new();
        opener.open("https://example.invalid/x");
        assert_eq!(
            *opener.opened.borrow(),
            vec!["https://example.invalid/x".to_string()]
        );
    }
}
