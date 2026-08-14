//! ReplicaStore：跨工作区的 pane 终端副本（LINUX-PLAN §7）。
//!
//! 每个 `(workspace_id, pane_id)` 持有一个 `TerminalState`，前台与后台
//! 连接都往这里 feed `%output`，保证切走工作区后历史与注意力信号不丢。
//! VTE 只负责显示；读历史/搜索一律走本副本。

use std::collections::HashMap;

use crate::core::attention::signal::AttentionSignal;
use crate::core::protocol::terminal::emulate::TerminalState;

/// 工作区 → pane → 终端副本。
pub struct ReplicaStore {
    inner: HashMap<String, HashMap<u32, TerminalState>>,
    scrollback_max: usize,
}

impl ReplicaStore {
    pub fn new(scrollback_max: usize) -> Self {
        Self {
            inner: HashMap::new(),
            scrollback_max: scrollback_max.max(1),
        }
    }

    /// 取（必要时创建）某工作区某 pane 的副本。
    pub fn ensure_pane(&mut self, ws: &str, pane: u32, cols: u16, rows: u16) -> &mut TerminalState {
        let max = self.scrollback_max;
        let state = self
            .inner
            .entry(ws.to_string())
            .or_default()
            .entry(pane)
            .or_insert_with(|| {
                TerminalState::with_scrollback(
                    usize::from(cols.max(1)),
                    usize::from(rows.max(1)),
                    max,
                )
            });
        state.resize(usize::from(cols.max(1)), usize::from(rows.max(1)));
        state
    }

    /// 把 pane 输出喂进副本（自动 resize 到最新尺寸），返回本轮注意力信号。
    pub fn feed(
        &mut self,
        ws: &str,
        pane: u32,
        bytes: &[u8],
        cols: u16,
        rows: u16,
    ) -> Vec<AttentionSignal> {
        let state = self.ensure_pane(ws, pane, cols, rows);
        state.feed(bytes);
        state.take_attention_signals()
    }

    /// 只读访问副本。
    pub fn get(&self, ws: &str, pane: u32) -> Option<&TerminalState> {
        self.inner.get(ws).and_then(|m| m.get(&pane))
    }

    /// pane 关闭时删除副本。
    pub fn drop_pane(&mut self, ws: &str, pane: u32) {
        if let Some(panes) = self.inner.get_mut(ws) {
            panes.remove(&pane);
        }
    }

    /// 工作区 evict 时删除全部副本。
    pub fn drop_workspace(&mut self, ws: &str) {
        self.inner.remove(ws);
    }

    /// 某 pane 最近 n 行（可见屏 + scrollback）。
    pub fn last_n_lines(&self, ws: &str, pane: u32, n: usize) -> Vec<String> {
        self.get(ws, pane)
            .map(|t| t.last_n_lines(n))
            .unwrap_or_default()
    }
}

/// 把一条 pane 输出应用到副本（window 前台/后台 pump 共用入口）。
pub fn apply_output_to_replicas(
    replicas: &mut ReplicaStore,
    ws: &str,
    pane: u32,
    bytes: &[u8],
    cols: u16,
    rows: u16,
) -> Vec<AttentionSignal> {
    replicas.feed(ws, pane, bytes, cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replica_feed_accumulates_scrollback() {
        let mut store = ReplicaStore::new(100);
        store.feed("ws-a", 1, b"hello\r\nworld\r\n", 80, 24);
        let lines = store.last_n_lines("ws-a", 1, 2);
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[test]
    fn replica_isolated_per_workspace() {
        let mut store = ReplicaStore::new(100);
        store.feed("ws-a", 1, b"alpha\r\n", 80, 24);
        store.feed("ws-b", 1, b"beta\r\n", 80, 24);
        assert_eq!(store.last_n_lines("ws-a", 1, 1), vec!["alpha"]);
        assert_eq!(store.last_n_lines("ws-b", 1, 1), vec!["beta"]);
    }

    #[test]
    fn drop_workspace_forgets_panes() {
        let mut store = ReplicaStore::new(100);
        store.feed("ws-a", 1, b"alpha\r\n", 80, 24);
        store.drop_workspace("ws-a");
        assert!(store.get("ws-a", 1).is_none());
        assert!(store.last_n_lines("ws-a", 1, 1).is_empty());
    }

    #[test]
    fn last_n_lines_reads_copy() {
        let mut store = ReplicaStore::new(100);
        store.feed("ws-a", 1, b"one\r\ntwo\r\n", 80, 24);
        let lines = store.last_n_lines("ws-a", 1, 1);
        assert_eq!(lines, vec!["two"]);
        // 副本不受外部修改影响（返回的是克隆）。
        store.feed("ws-a", 1, b"three\r\n", 80, 24);
        assert_eq!(store.last_n_lines("ws-a", 1, 2), vec!["two", "three"]);
    }

    #[test]
    fn apply_output_feeds_replica() {
        let mut store = ReplicaStore::new(100);
        apply_output_to_replicas(&mut store, "ws-a", 7, b"hello\r\n", 80, 24);
        assert_eq!(store.last_n_lines("ws-a", 7, 1), vec!["hello"]);
    }

    #[test]
    fn feed_returns_attention_signals() {
        let mut store = ReplicaStore::new(100);
        let sigs = apply_output_to_replicas(&mut store, "ws-a", 1, b"\x1b]133;C\x07", 80, 24);
        assert_eq!(sigs, vec![AttentionSignal::CommandStart]);
    }

    #[test]
    fn resize_updates_existing_pane() {
        let mut store = ReplicaStore::new(100);
        store.feed("ws-a", 1, b"x", 80, 24);
        store.feed("ws-a", 1, b"y", 120, 40);
        let t = store.get("ws-a", 1).unwrap();
        assert_eq!(t.cols(), 120);
        assert_eq!(t.rows(), 40);
    }
}
