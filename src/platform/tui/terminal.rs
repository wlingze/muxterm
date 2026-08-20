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

    /// 运行时调整尺寸：保留屏幕 / 光标 / 滚动区域，不清空重放。
    ///
    /// 之前这里重建 `TerminalState` 并把 `fed_len` 清零，下次同步会从头
    /// 重放被截断的累计输出，ANSI 流从中间开始解析，导致 codex/htop
    /// 调整大小后内容错乱。
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.state.resize(self.cols as usize, self.rows as usize);
    }

    /// 取出终端生成的查询应答（OSC 颜色 / CSI DA / DSR），原样回写 shell。
    pub fn take_reply(&mut self) -> Vec<u8> {
        self.state.take_reply()
    }

    /// 用累计输出做增量同步（GTK 同款）：只 feed 比已 feed 长度更新的部分。
    ///
    /// 输出被后端截断（`full.len() < fed_len`）时**不重放**：本地终端已经
    /// 持有比累计缓冲更完整的屏幕，重置并从截断的尾部重放只会让 ANSI 流从
    /// 中间开始解析，产生乱码 / 黑屏。真实终端的做法是保留完整屏幕模型，
    /// 只在缺失时用全帧快照重建，而不是重放有损字节流。
    ///
    /// 但要把游标推进到截断后缓冲的末尾：否则要等缓冲重新长过旧游标
    /// （最多 2MB）才有增量可 feed，期间 pane 内容完全不刷新。
    pub fn sync_from_output(&mut self, full: &[u8]) {
        if full.len() > self.fed_len {
            let delta = &full[self.fed_len..];
            self.feed(delta);
            self.fed_len = full.len();
        } else if full.len() < self.fed_len {
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
pub struct TerminalManager {
    panes: HashMap<u32, PaneTerminal>,
    /// 是否把终端模拟器生成的查询应答（OSC 10/11、CSI DA 等）回写给 pane。
    ///
    /// 仅本地 / daemon 后端（前端就是该 PTY 的终端模拟器）为 `true`；tmux
    /// 控制模式（`tmux` / `tmux-ssh`）必须为 `false`：tmux 拥有 pane 的
    /// PTY 与协议，应答经 `send-keys -l` 回写会作为按键被 pane 回显并执行，
    /// 造成 `git lg` 的 `10;rgb:...` / `65;...c` 泄漏进 shell。
    pub forward_replies: bool,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            panes: HashMap::new(),
            forward_replies: true,
        }
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalManager {
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
        if !self.forward_replies {
            let _ = pt.take_reply();
        }
        pt.fed_len = pt.fed_len.saturating_add(data.len());
    }

    /// 事件驱动的增量同步：把后端 `%output` 事件字节喂进对应 pane 的终端。
    ///
    /// 与按累计输出位置切片（`sync_output`）不同，事件字节就是后端实际收到的
    /// 新输出，**顺序天然正确**：即使累计缓冲因 2MB 上限被截断，事件流也从不
    /// 跳段，终端模拟器不会从 ANSI 序列中间开始解析。
    ///
    /// 只负责 feed，不重放历史；重放会在切换 tab 后重新生成旧查询应答，
    /// 泄漏成 shell 里的字面文本（git lg 的 `10;rgb:...` / `65;...c`）。
    pub fn feed_event(&mut self, pane_id: u32, data: &[u8]) {
        let (cols, rows) = self
            .panes
            .get(&pane_id)
            .map(|p| (p.cols, p.rows))
            .unwrap_or((80, 24));
        self.feed(pane_id, cols, rows, data);
    }

    /// 用权威 snapshot 替换已有 pane 的 VT 状态；不可把它当增量 feed，
    /// 否则 pause/resync 后旧的半截 CUP 帧会继续污染屏幕。
    pub fn replace_snapshot(&mut self, pane_id: u32, data: &[u8]) {
        let (cols, rows) = self
            .panes
            .get(&pane_id)
            .map(|p| (p.cols, p.rows))
            .unwrap_or((80, 24));
        self.panes.remove(&pane_id);
        self.seed(pane_id, cols, rows, data);
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
        if !self.forward_replies {
            let _ = pt.take_reply();
        }
    }

    /// 首次播种：pane 还没有终端状态时，用累计输出初始化。
    ///
    /// 只对**不存在的** pane 生效；已存在（已通过事件喂过增量）的 pane 只调整
    /// 尺寸，绝不重放历史，避免重复生成历史查询应答。
    pub fn seed(&mut self, pane_id: u32, cols: u16, rows: u16, full: &[u8]) {
        if self.panes.contains_key(&pane_id) {
            self.resize_pane(pane_id, cols, rows);
            return;
        }
        let mut pt = PaneTerminal::new(cols.max(1), rows.max(1));
        pt.resize(cols, rows);
        pt.feed(full);
        if !self.forward_replies {
            let _ = pt.take_reply();
        }
        pt.fed_len = full.len();
        self.panes.insert(pane_id, pt);
    }

    /// 调整已有 pane 的终端尺寸（保留屏幕/光标/滚动区域），不存在则忽略。
    pub fn resize_pane(&mut self, pane_id: u32, cols: u16, rows: u16) {
        if let Some(pt) = self.panes.get_mut(&pane_id) {
            pt.resize(cols, rows);
        }
    }

    /// 是否已持有某 pane 的终端状态。
    pub fn has(&self, pane_id: u32) -> bool {
        self.panes.contains_key(&pane_id)
    }

    /// 移除某 pane 的终端状态（仅 pane 真正关闭时调用，tab 切换不调用）。
    pub fn remove(&mut self, pane_id: u32) {
        self.panes.remove(&pane_id);
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

    /// 取出所有 pane 的查询应答，供前端统一 `send_input` 回写。
    pub fn drain_replies(&mut self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        for (id, pt) in &mut self.panes {
            let reply = pt.take_reply();
            if !reply.is_empty() {
                out.push((*id, reply));
            }
        }
        out
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

    /// 回归：事件驱动 feed 后，tab 切换/重绘只调整尺寸，**不重放历史**，
    /// 因此历史查询应答不会再次生成并泄漏进 shell。
    #[test]
    fn event_feed_does_not_reanswer_queries_on_resize_redraw() {
        let mut mgr = TerminalManager::new();
        // 一条真实查询：OSC 10/11 + CSI DA（git lg / zsh 提示符常用）
        mgr.feed_event(1, b"x\x1b]10;?\x07\x1b]11;?\x07\x1b[c");
        let replies = mgr.drain_replies();
        assert_eq!(replies.len(), 1, "实时查询应恰好应答一次");
        assert_eq!(replies[0].0, 1);
        let data = String::from_utf8_lossy(&replies[0].1).into_owned();
        assert!(data.contains("10;rgb:"));
        assert!(data.contains("11;rgb:"));
        assert!(data.contains("65;4;1;2;6;21;22;17;28c"));

        // 模拟切走再切回后的重绘：只 resize，不应再产生任何应答
        mgr.resize_pane(1, 80, 24);
        mgr.resize_pane(1, 120, 40);
        assert!(
            mgr.drain_replies().is_empty(),
            "重绘不得重新生成历史查询应答"
        );
    }

    /// 回归：切到别的 tab（snapshot 不再包含该 pane）时，终端状态必须保留；
    /// 只有 PaneClosed 事件才真正移除。
    #[test]
    fn pane_state_survives_inactive_tab_and_only_close_removes_it() {
        let mut mgr = TerminalManager::new();
        mgr.feed_event(1, b"hello");
        // 旧实现这里会 retain(active tab ids)，导致 pane 1 状态被清掉
        assert!(mgr.has(1), "切 tab 不应清掉非激活 pane 的终端状态");
        mgr.resize_pane(1, 40, 12);
        assert_eq!(mgr.size(1), Some((40, 12)));
        mgr.remove(1);
        assert!(!mgr.has(1), "只有 PaneClosed 才应移除状态");
    }

    /// seed 只对不存在的 pane 生效；已存在的 pane 不会重放累计输出。
    #[test]
    fn seed_never_replays_for_existing_pane() {
        let mut mgr = TerminalManager::new();
        mgr.feed_event(1, b"\x1b]10;?\x07");
        let first = mgr.drain_replies();
        assert_eq!(first.len(), 1);

        // 用带历史查询的累计输出再次 seed：必须只 resize，不重放
        let full = b"x\x1b]10;?\x07\x1b]11;?\x07\x1b[c".to_vec();
        mgr.seed(1, 80, 24, &full);
        assert!(
            mgr.drain_replies().is_empty(),
            "seed 已存在的 pane 不得重新生成应答"
        );

        // 不存在的 pane 才允许从累计输出播种
        mgr.seed(2, 80, 24, &full);
        assert_eq!(mgr.drain_replies().len(), 1);
        assert!(mgr.has(2));
    }

    /// 回归：tmux 控制模式下，前端只是渲染镜像，解析出的查询应答必须丢弃，
    /// 不能经 send-keys 回写（否则泄漏成 shell 字面命令）。
    #[test]
    fn tmux_mode_discards_query_replies() {
        let mut mgr = TerminalManager::new();
        mgr.forward_replies = false;
        mgr.feed_event(1, b"\x1b]10;?\x07\x1b]11;?\x07\x1b[c");
        assert!(
            mgr.drain_replies().is_empty(),
            "tmux 控制模式不得生成任何回写应答"
        );

        // seed 路径同样丢弃：从累计输出播种时不能重新应答历史查询
        mgr.seed(2, 80, 24, b"x\x1b]10;?\x07\x1b]11;?\x07\x1b[c");
        assert!(
            mgr.drain_replies().is_empty(),
            "tmux 控制模式播种历史输出时也不得生成应答"
        );
    }

    /// 回归：累计输出被 2MB 上限截断时，只做正向增量同步，**不**清空重放。
    /// 重放截断尾部会让 ANSI 流从中间开始解析，屏幕乱码；保留本地已 feed
    /// 的完整屏幕才是终端模拟器的正确行为。
    #[test]
    fn truncated_cumulative_output_does_not_reset_or_replay() {
        let mut mgr = TerminalManager::new();
        mgr.feed_event(1, b"line1\r\nline2\r\n");
        let rows = |mgr: &TerminalManager| {
            mgr.screen(1)
                .unwrap()
                .into_iter()
                .map(|r| r.trim_end().to_string())
                .filter(|r| !r.is_empty())
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&mgr), vec!["line1", "line2"]);

        // 模拟后端截断：累计缓冲只剩尾部（且从 ANSI 序列中间开始）。
        let tail = b"\x1b[31mline9\r\n".to_vec();
        mgr.sync_output(1, 80, 24, &tail);

        // 屏幕必须保持旧内容 + 只追加尾部之前的新内容（尾部比 fed_len 短，
        // 本实现选择不重放，屏幕不变，绝不出现从 `[31m` 中间解析的乱码）。
        let screen = rows(&mgr).join("|");
        assert!(
            screen.contains("line1") && screen.contains("line2"),
            "截断不应清空已渲染屏幕，实际: {screen:?}"
        );
        assert!(
            !screen.contains("line9"),
            "后端截断丢掉的旧内容不应被重放补齐，实际: {screen:?}"
        );

        // 截断后继续有正常增量时，必须只 feed 新增部分（游标已推进到截断
        // 末尾），不会饿死到缓冲重新长过旧游标，也不会把旧内容再喂一遍。
        mgr.feed_event(1, b"\r\nline3");
        let screen2 = rows(&mgr).join("|");
        assert!(
            screen2.contains("line2") && screen2.contains("line3"),
            "截断后增量应继续渲染，实际: {screen2:?}"
        );
        assert!(
            !screen2.contains("line9"),
            "截断后也不得重放旧尾部，实际: {screen2:?}"
        );
    }
}
