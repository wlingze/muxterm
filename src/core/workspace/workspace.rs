//! Workspace：池里一格，包装一个 Runtime。
//!
//! 一个 Workspace = 一个 Runtime + 本工作区 pane 文本副本。Runtime 推
//! `StateChange::PaneOutput` 时，Workspace 把原始字节喂进对应 Pane 的
//! `TerminalState`（Index 面，供搜索/提醒；live 显示仍走 Surface 原始字节）。

use std::collections::HashMap;

use crate::core::model::backend::Runtime;
use crate::core::model::state::{State, StateChange};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::model::terminal_model::TerminalModel;
use crate::core::protocol::terminal::emulate::DEFAULT_SCROLLBACK_LINES;
use crate::core::types::{PaneId, TabId};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pane_buf::PaneBuf;

/// 一次搜索命中：工作区 + tab + pane + scrollback seq + 行文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub workspace_id: String,
    pub tab_id: TabId,
    pub pane_id: PaneId,
    pub seq: u64,
    pub line: String,
}

/// 一个工作区：稳定 id + 一个 Runtime + 本工作区 pane 缓冲（PaneBuf）。
pub struct Workspace {
    id: WorkspaceId,
    name: String,
    model: TerminalModel,
    panes: HashMap<PaneId, PaneBuf>,
}

impl Workspace {
    /// 创建工作区，接管给定 backend（W4 改名为 Runtime）。
    pub fn new(id: WorkspaceId, name: String, runtime: Box<dyn Runtime>) -> Self {
        Self {
            id,
            name,
            model: TerminalModel::new(runtime),
            panes: HashMap::new(),
        }
    }

    /// 稳定工作区 id。
    pub fn id(&self) -> &WorkspaceId {
        &self.id
    }

    /// 用户可见的工作区名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 只读访问底层 Runtime。
    pub fn runtime(&self) -> &dyn Runtime {
        self.model.runtime()
    }

    /// 换掉底层 Runtime（W17a 自动重连；PaneBuf 副本保留）。
    pub fn swap_runtime(&mut self, runtime: Box<dyn Runtime>) {
        self.model.swap_runtime(runtime);
    }

    /// 可变访问底层 Runtime，供测试注入事件。
    pub fn runtime_mut(&mut self) -> &mut dyn Runtime {
        self.model.runtime_mut()
    }

    /// 只读访问当前状态快照。
    pub fn state(&self) -> &dyn State {
        self.model.state()
    }

    /// 执行一个 Task（NewTab / SplitPane / SendKeys / …）。
    pub fn execute(&mut self, task: Task) -> anyhow::Result<TaskOutcome> {
        self.model.execute(task)
    }

    /// 建立连接（spawn tmux / 启动本地 shell）。
    pub async fn connect(&mut self) -> anyhow::Result<()> {
        self.model.connect().await
    }

    /// 关闭 Runtime 并释放资源。
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.model.shutdown().await
    }

    /// 拉取尚未消费的状态变更事件，并把 `PaneOutput` 喂进本工作区 pane 文本。
    pub fn take_events(&mut self) -> Vec<StateChange> {
        let events = self.model.take_events();
        self.feed_events(&events);
        events
    }

    /// 先从 Runtime 拉取最新事件（异步输出），再取走并喂进本工作区副本。
    pub fn refresh(&mut self) -> Vec<StateChange> {
        let events = self.model.refresh();
        self.feed_events(&events);
        events
    }

    /// 某 pane 的文本（可见屏 + scrollback，供搜索/提醒）。
    pub fn pane_text(&self, pane: PaneId) -> String {
        self.panes
            .get(&pane)
            .map(|t| t.last_n_lines(DEFAULT_SCROLLBACK_LINES).join("\n"))
            .unwrap_or_default()
    }

    /// 某 pane 的 scrollback 中指定 seq 的行索引（W17c 搜索跳转用）。
    pub fn pane_line_index_by_seq(&self, pane: PaneId, seq: u64) -> Option<usize> {
        self.panes.get(&pane).and_then(|t| t.line_index_by_seq(seq))
    }

    /// 某 pane 的最近 n 行。
    pub fn pane_last_n_lines(&self, pane: PaneId, n: usize) -> Vec<String> {
        self.panes
            .get(&pane)
            .map(|t| t.last_n_lines(n))
            .unwrap_or_default()
    }

    /// 某 pane 的原始字节环（peek / 小终端播种用）。
    pub fn pane_raw_bytes(&self, pane: PaneId) -> Vec<u8> {
        self.panes
            .get(&pane)
            .map(|t| t.raw_bytes().to_vec())
            .unwrap_or_default()
    }

    /// 取走某 pane 的 OSC/CSI 查询应答。
    pub fn take_reply(&mut self, pane: PaneId) -> Vec<u8> {
        self.panes
            .get_mut(&pane)
            .map(|t| t.take_reply())
            .unwrap_or_default()
    }

    /// 某 pane 的可见网格 ANSI（首屏播种用；禁止当 live 显示）。
    pub fn pane_visible_ansi(&self, pane: PaneId) -> Vec<u8> {
        self.panes
            .get(&pane)
            .map(|t| t.visible_ansi())
            .unwrap_or_default()
    }

    /// 某 pane 的滚动窗口 ANSI。
    pub fn pane_scroll_ansi(&self, pane: PaneId, offset: u32, rows: u32) -> Vec<u8> {
        self.panes
            .get(&pane)
            .map(|t| t.scroll_ansi(offset, rows))
            .unwrap_or_default()
    }

    /// 某 pane 网格是否全空。
    pub fn pane_is_blank(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).map(|t| t.is_blank()).unwrap_or(true)
    }

    /// 某 pane 是否 bracketed paste 模式。
    pub fn pane_bracketed_paste(&self, pane: PaneId) -> bool {
        self.panes
            .get(&pane)
            .map(|t| t.bracketed_paste())
            .unwrap_or(false)
    }

    /// 某 pane 的 viewport 滚动偏移。
    pub fn pane_viewport(&self, pane: PaneId) -> u32 {
        self.panes.get(&pane).map(|t| t.viewport()).unwrap_or(0)
    }

    /// 设置某 pane 的 viewport 滚动偏移（跳转后恢复）。
    pub fn set_pane_viewport(&mut self, pane: PaneId, offset: u32) {
        if let Some(t) = self.panes.get_mut(&pane) {
            t.set_viewport(offset);
        }
    }

    /// 取走某 pane 尚未消费的注意力信号。
    pub fn take_attention_signals(
        &mut self,
        pane: PaneId,
    ) -> Vec<crate::core::attention::signal::AttentionSignal> {
        self.panes
            .get_mut(&pane)
            .map(|t| t.take_attention_signals())
            .unwrap_or_default()
    }

    /// 某 pane 最近一次 feed 的 seq + 最后非空行。
    pub fn pane_last_line_seq(&self, pane: PaneId) -> (String, u64) {
        self.panes
            .get(&pane)
            .map(|t| t.last_line_seq())
            .unwrap_or_default()
    }

    /// 测试/注入用：直接向某 pane 的 PaneBuf 喂字节（绕过 Runtime）。
    pub fn feed_pane_bytes(&mut self, pane: PaneId, bytes: &[u8], cols: u16, rows: u16) {
        let buf = self.panes.entry(pane).or_insert_with(|| {
            PaneBuf::new(
                usize::from(cols),
                usize::from(rows),
                DEFAULT_SCROLLBACK_LINES,
            )
        });
        buf.feed(bytes, cols, rows);
    }

    /// 搜索本工作区某 pane，返回带 tab 的命中。
    pub fn search_pane(&self, pane: PaneId, query: &str) -> Vec<SearchHit> {
        let Some(buf) = self.panes.get(&pane) else {
            return Vec::new();
        };
        let tab_id = self.state().pane(&pane).map(|p| p.tab).unwrap_or(TabId(0));
        buf.search(query)
            .into_iter()
            .map(|(seq, line)| SearchHit {
                workspace_id: self.id.replica_id(),
                tab_id,
                pane_id: pane,
                seq,
                line,
            })
            .collect()
    }

    /// 搜索本工作区全部 pane。
    pub fn search_workspace(&self, query: &str) -> Vec<SearchHit> {
        tracing::info!(
            target: "muxterm::search",
            query = query,
            panes = self.panes.len(),
            "search_workspace"
        );
        let mut out = Vec::new();
        for pane in self.panes.keys() {
            out.extend(self.search_pane(*pane, query));
        }
        out
    }

    /// 把事件流里的 pane 输出喂进本工作区 PaneBuf；pane 关闭时删除副本。
    fn feed_events(&mut self, events: &[StateChange]) {
        for event in events {
            match event {
                StateChange::PaneOutput { pane, data } => {
                    let (cols, rows) = self
                        .state()
                        .pane(pane)
                        .map(|p| (p.cols, p.rows))
                        .unwrap_or((80, 24));
                    let buf = self.panes.entry(*pane).or_insert_with(|| {
                        PaneBuf::new(
                            usize::from(cols),
                            usize::from(rows),
                            DEFAULT_SCROLLBACK_LINES,
                        )
                    });
                    buf.feed(data, cols, rows);
                }
                StateChange::PaneClosed { pane } => {
                    self.panes.remove(pane);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockRuntime;
    use crate::core::model::task::Task;

    fn workspace(name: &str) -> Workspace {
        let id = WorkspaceId::new("local", None, name, "tmux", "");
        Workspace::new(
            id,
            name.to_string(),
            Box::new(MockRuntime::with_single_pane()),
        )
    }

    /// mock Runtime 推一段 %output 等价事件（WriteRaw → PaneOutput），
    /// Workspace 应把字节喂进本工作区 pane 文本。
    #[test]
    fn pane_text_contains_token_after_output_event() {
        let mut w = workspace("demo");
        w.execute(Task::WriteRaw {
            target: PaneId(1),
            data: b"hello MUXTERM_TOKEN\r\n".to_vec(),
        })
        .unwrap();
        let events = w.take_events();
        assert!(events.iter().any(|e| matches!(
            e,
            StateChange::PaneOutput {
                pane: PaneId(1),
                ..
            }
        )));
        assert!(w.pane_text(PaneId(1)).contains("MUXTERM_TOKEN"));
    }

    /// W6：search_pane 命中带 tab_id；同 pane 不同 tab 不串。
    #[test]
    fn search_pane_returns_hits_with_tab_id() {
        let mut w = workspace("demo");
        w.feed_pane_bytes(PaneId(1), b"alpha TOKEN_BODY one\r\n", 80, 24);
        w.feed_pane_bytes(PaneId(2), b"beta TOKEN_BODY two\r\n", 80, 24);

        let hits = w.search_pane(PaneId(1), "TOKEN_BODY");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].pane_id, PaneId(1));
        assert_eq!(hits[0].tab_id, TabId(1), "pane 1 属于 tab 1");
        assert!(hits[0].line.contains("TOKEN_BODY"));
        assert!(hits[0].workspace_id.contains("demo"));
    }

    /// W6：search_workspace 覆盖本工作区全部 pane。
    #[test]
    fn search_workspace_finds_all_panes() {
        let mut w = workspace("demo");
        w.feed_pane_bytes(PaneId(1), b"alpha TOKEN_BODY one\r\n", 80, 24);
        w.feed_pane_bytes(PaneId(2), b"beta\r\n", 80, 24);
        w.feed_pane_bytes(PaneId(3), b"gamma TOKEN_BODY two\r\n", 80, 24);

        let hits = w.search_workspace("TOKEN_BODY");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.pane_id == PaneId(1)));
        assert!(hits.iter().any(|h| h.pane_id == PaneId(3)));
        assert!(w.search_workspace("missing").is_empty());
    }

    /// W6：byte ring 超过 cap 丢最旧，搜索仍能命中最近 token。
    #[test]
    fn byte_ring_drops_oldest_but_search_finds_recent() {
        let mut w = workspace("demo");
        // 灌入远超 MAX_PANE_OUTPUT_BYTES 的旧行（带换行，ring 对齐到行边界），
        // 再写最近 token。
        let line = b"old-data-line\r\n";
        let mut old = Vec::new();
        while old.len() < crate::core::buffer_cap::MAX_PANE_OUTPUT_BYTES + 1024 {
            old.extend_from_slice(line);
        }
        w.feed_pane_bytes(PaneId(1), &old, 80, 24);
        w.feed_pane_bytes(PaneId(1), b"RECENT_TOKEN\r\n", 80, 24);

        let raw = w.pane_raw_bytes(PaneId(1));
        assert!(
            raw.len() <= crate::core::buffer_cap::MAX_PANE_OUTPUT_BYTES,
            "byte ring 应有界: {}",
            raw.len()
        );
        assert!(
            String::from_utf8_lossy(&raw).contains("RECENT_TOKEN"),
            "最近 token 应保留在 ring 里"
        );
        let hits = w.search_pane(PaneId(1), "RECENT_TOKEN");
        assert!(
            !hits.is_empty(),
            "搜索仍应命中最近 token（scrollback 有界但保留尾部）"
        );
    }

    /// W6：viewport 可设置/读取（跳转后恢复）。
    #[test]
    fn pane_viewport_roundtrip() {
        let mut w = workspace("demo");
        w.feed_pane_bytes(PaneId(1), b"line\r\n", 80, 24);
        assert_eq!(w.pane_viewport(PaneId(1)), 0);
        w.set_pane_viewport(PaneId(1), 12);
        assert_eq!(w.pane_viewport(PaneId(1)), 12);
    }

    /// 两个 Workspace、同一 PaneId 数字 → 文本互不污染。
    #[test]
    fn same_pane_id_isolated_between_workspaces() {
        let mut a = workspace("alpha");
        let mut b = workspace("beta");
        a.execute(Task::WriteRaw {
            target: PaneId(1),
            data: b"alpha-only\r\n".to_vec(),
        })
        .unwrap();
        b.execute(Task::WriteRaw {
            target: PaneId(1),
            data: b"beta-only\r\n".to_vec(),
        })
        .unwrap();
        a.take_events();
        b.take_events();

        let a_text = a.pane_text(PaneId(1));
        let b_text = b.pane_text(PaneId(1));
        assert!(a_text.contains("alpha-only"));
        assert!(!a_text.contains("beta-only"));
        assert!(b_text.contains("beta-only"));
        assert!(!b_text.contains("alpha-only"));
    }
}
