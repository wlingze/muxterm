//! 输出缓冲：纯环形行缓冲，不解析 ANSI。
//!
//! 生产路径已由 `emulate.rs` 自持 scrollback 取代（LINUX-PLAN §6）；
//! 本类型保留作语义参考。不要在生产路径新增调用者。

/// 固定上限的滚动行缓冲。
#[derive(Debug, Clone)]
pub struct ScrollbackBuffer {
    lines: Vec<String>,
    max_lines: usize,
    /// 当前未完成行（尚未遇到 `\n`）。
    pending: String,
}

impl ScrollbackBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: Vec::new(),
            max_lines: max_lines.max(1),
            pending: String::new(),
        }
    }

    /// 追加文本；按 `\n` 拆行，超限时丢弃最旧行。
    pub fn push(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.pending);
                self.push_line(line);
            } else if ch == '\r' {
                // 忽略 CR（与常见终端 scrollback 行为一致）
            } else {
                self.pending.push(ch);
            }
        }
    }

    fn push_line(&mut self, line: String) {
        if self.lines.len() >= self.max_lines {
            let overflow = self.lines.len() + 1 - self.max_lines;
            self.lines.drain(0..overflow);
        }
        self.lines.push(line);
    }

    /// 已完成的行（不含 pending）。
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// 子串匹配（大小写敏感），返回 `(行号, 行内容)`；行号从 0 起。
    pub fn search(&self, query: &str) -> Vec<(usize, String)> {
        if query.is_empty() {
            return Vec::new();
        }
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(query))
            .map(|(i, l)| (i, l.clone()))
            .collect()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.pending.clear();
    }

    /// 当前未完成行（测试 / 调试用）。
    #[cfg(test)]
    pub(crate) fn pending(&self) -> &str {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_splits_lines() {
        let mut buf = ScrollbackBuffer::new(100);
        buf.push("hello\nworld\n");
        assert_eq!(buf.lines(), &["hello".to_string(), "world".to_string()]);
        assert_eq!(buf.pending(), "");
    }

    #[test]
    fn pending_until_newline() {
        let mut buf = ScrollbackBuffer::new(10);
        buf.push("partial");
        assert!(buf.lines().is_empty());
        assert_eq!(buf.pending(), "partial");
        buf.push(" line\n");
        assert_eq!(buf.lines(), &["partial line".to_string()]);
    }

    #[test]
    fn ring_evicts_oldest() {
        let mut buf = ScrollbackBuffer::new(3);
        buf.push("a\nb\nc\nd\n");
        assert_eq!(
            buf.lines(),
            &["b".to_string(), "c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn search_returns_matches() {
        let mut buf = ScrollbackBuffer::new(20);
        buf.push("alpha\nbeta\nalphabet\n");
        let hits = buf.search("alpha");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0], (0, "alpha".into()));
        assert_eq!(hits[1], (2, "alphabet".into()));
        assert!(buf.search("").is_empty());
        assert!(buf.search("zzz").is_empty());
    }

    #[test]
    fn clear_resets() {
        let mut buf = ScrollbackBuffer::new(10);
        buf.push("x\ny");
        buf.clear();
        assert!(buf.lines().is_empty());
        assert_eq!(buf.pending(), "");
    }

    #[test]
    fn ignores_cr() {
        let mut buf = ScrollbackBuffer::new(10);
        buf.push("ab\rc\n");
        assert_eq!(buf.lines(), &["abc".to_string()]);
    }
}
