//! TUI 终端渲染层：每个 pane 用一个无头 [`TerminalState`] 模拟终端屏幕。
//!
//! GTK 前端用 VTE（真实终端模拟器）渲染，TUI 没有 VTE，但项目自带
//! `core::protocol::terminal::emulate::TerminalState`（基于 vte crate）能正确
//! 跟踪光标移动 / 覆盖写 / 清屏 / 换行，生成真实的屏幕网格。
//!
//! 之前 TUI 把 pane 的**累计原始输出**当作纯文本行直接打印，导致：
//! - 回显字符双写（`ls` 变 `ls ls`）
//! - 光标覆盖写 / 清屏不生效，内容错位乱码
//!
//! 这里改为：把增量输出 feed 进 TerminalState，用它的屏幕快照渲染。

use std::collections::HashMap;

use crate::core::protocol::terminal::emulate::{Cell, TerminalState};

/// 每 pane 的终端状态 + 尺寸。
pub struct PaneTerminal {
    state: TerminalState,
    cols: u16,
    rows: u16,
    /// 已 feed 到 state 的累计输出长度（用于增量同步）。
    fed_len: usize,
}

impl PaneTerminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            state: TerminalState::new(cols as usize, rows as usize),
            cols,
            rows,
            fed_len: 0,
        }
    }

    /// 把增量输出喂进模拟器。
    pub fn feed(&mut self, data: &[u8]) {
        self.state.feed(data);
    }

    /// 重建/调整尺寸（清空重来）。
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.state = TerminalState::new(self.cols as usize, self.rows as usize);
        self.fed_len = 0;
    }

    /// 用累计输出做增量同步（GTK 同款）：只 feed 比已 feed 长度更新的部分。
    /// 输出被截断时重置并全量重放。
    pub fn sync_from_output(&mut self, full: &[u8]) {
        if full.len() > self.fed_len {
            let delta = &full[self.fed_len..];
            self.feed(delta);
            self.fed_len = full.len();
        } else if full.len() < self.fed_len {
            // 输出被重置/截断：清空重放
            self.state = TerminalState::new(self.cols as usize, self.rows as usize);
            self.feed(full);
            self.fed_len = full.len();
        }
    }

    /// 当前屏幕快照（每行一个字符串，含空白），按行数补齐。
    pub fn screen(&self) -> Vec<String> {
        self.state.snapshot()
    }

    /// 当前光标所在行（0 基）。
    pub fn cursor_row(&self) -> usize {
        self.state.cursor_row()
    }

    /// 当前屏幕带样式单元格网格（每行 Vec<Cell>），保留颜色/样式。
    pub fn styled_screen(&self) -> Vec<Vec<Cell>> {
        self.state.styled_screen()
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }
}

/// 管理所有 pane 的终端状态。
#[derive(Default)]
pub struct TerminalManager {
    panes: HashMap<u32, PaneTerminal>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 确保 pane 有终端状态（懒创建）。
    pub fn ensure(&mut self, pane_id: u32, cols: u16, rows: u16) {
        self.panes
            .entry(pane_id)
            .or_insert_with(|| PaneTerminal::new(cols.max(1), rows.max(1)));
    }

    /// 把某 pane 的增量输出 feed 进其终端。
    pub fn feed(&mut self, pane_id: u32, cols: u16, rows: u16, data: &[u8]) {
        let pt = self
            .panes
            .entry(pane_id)
            .or_insert_with(|| PaneTerminal::new(cols.max(1), rows.max(1)));
        pt.resize(cols, rows);
        pt.feed(data);
    }

    /// 取某 pane 的屏幕快照；不存在返回空。
    pub fn screen(&self, pane_id: u32) -> Option<Vec<String>> {
        self.panes.get(&pane_id).map(|p| p.screen())
    }

    /// 取某 pane 的带样式屏幕网格 + 光标行；不存在返回 None。
    pub fn styled_screen_with_cursor(&self, pane_id: u32) -> Option<(Vec<Vec<Cell>>, usize)> {
        self.panes
            .get(&pane_id)
            .map(|p| (p.styled_screen(), p.cursor_row()))
    }

    /// 用累计输出做增量同步（GTK 同款 delta 方案）。
    pub fn sync_output(&mut self, pane_id: u32, cols: u16, rows: u16, full: &[u8]) {
        let pt = self
            .panes
            .entry(pane_id)
            .or_insert_with(|| PaneTerminal::new(cols.max(1), rows.max(1)));
        pt.resize(cols, rows);
        pt.sync_from_output(full);
    }

    /// 取某 pane 的屏幕快照 + 光标行；不存在返回 None。
    pub fn screen_with_cursor(&self, pane_id: u32) -> Option<(Vec<String>, usize)> {
        self.panes
            .get(&pane_id)
            .map(|p| (p.screen(), p.cursor_row()))
    }

    /// 取某 pane 的尺寸。
    pub fn size(&self, pane_id: u32) -> Option<(u16, u16)> {
        self.panes.get(&pane_id).map(|p| (p.cols, p.rows))
    }

    /// 删除不再存在的 pane。
    pub fn retain(&mut self, ids: &[u32]) {
        self.panes.retain(|id, _| ids.contains(id));
    }

    /// 清空所有（重连时用）。
    pub fn clear(&mut self) {
        self.panes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// resize 不再重建状态：屏幕内容和光标被保留，增量输出继续正确。
    #[test]
    fn pane_resize_preserves_screen_and_continues_delta() {
        let mut pt = PaneTerminal::new(10, 5);
        pt.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        pt.resize(10, 3);
        assert_eq!(pt.rows(), 3);
        let trimmed = |s: &str| s.trim_end().to_string();
        assert_eq!(
            pt.screen().iter().map(|s| trimmed(s)).collect::<Vec<_>>(),
            vec!["3", "4", "5"]
        );
        // 增量同步：resize 后新输出只喂增量，不重放历史
        pt.feed(b"\r\n6");
        assert_eq!(
            pt.screen().iter().map(|s| trimmed(s)).collect::<Vec<_>>(),
            vec!["4", "5", "6"]
        );
    }

    /// 查询应答从 TerminalState 冒泡出来，供前端回写 shell。
    #[test]
    fn pane_take_reply_returns_query_response() {
        let mut pt = PaneTerminal::new(80, 24);
        pt.feed(b"\x1b]10;?\x1b\\");
        assert_eq!(pt.take_reply(), b"\x1b]10;rgb:0000/0000/0000\x1b\\");
        assert!(pt.take_reply().is_empty());
    }

    /// TerminalManager 可以统一收集所有 pane 的应答。
    #[test]
    fn manager_drains_replies_for_all_panes() {
        let mut mgr = TerminalManager::new();
        mgr.feed(1, 80, 24, b"\x1b]10;?\x1b\\");
        mgr.feed(2, 80, 24, b"\x1b[c");
        let replies = mgr.drain_replies();
        assert_eq!(replies.len(), 2);
        assert!(replies.iter().any(|(id, _)| *id == 1));
        assert!(replies.iter().any(|(id, _)| *id == 2));
    }
}
