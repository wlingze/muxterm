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
use crate::core::protocol::terminal::emulate::{TerminalState, DEFAULT_SCROLLBACK_LINES};
use crate::core::types::PaneId;
use crate::core::workspace::id::WorkspaceId;

/// 一个工作区：稳定 id + 一个 Runtime+ 本工作区 pane 文本副本。
pub struct Workspace {
    id: WorkspaceId,
    name: String,
    model: TerminalModel,
    panes: HashMap<PaneId, TerminalState>,
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

    /// 某 pane 的文本（可见屏 + scrollback，供搜索/提醒）。
    pub fn pane_text(&self, pane: PaneId) -> String {
        self.panes
            .get(&pane)
            .map(|t| t.last_n_lines(DEFAULT_SCROLLBACK_LINES).join("\n"))
            .unwrap_or_default()
    }

    /// 把事件流里的 pane 输出喂进本工作区副本；pane 关闭时删除副本。
    fn feed_events(&mut self, events: &[StateChange]) {
        for event in events {
            match event {
                StateChange::PaneOutput { pane, data } => {
                    let (cols, rows) = self
                        .state()
                        .pane(pane)
                        .map(|p| (p.cols, p.rows))
                        .unwrap_or((80, 24));
                    let state = self.panes.entry(*pane).or_insert_with(|| {
                        TerminalState::new(usize::from(cols), usize::from(rows))
                    });
                    state.resize(usize::from(cols), usize::from(rows));
                    state.feed(data);
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
