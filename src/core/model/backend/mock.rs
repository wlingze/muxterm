//! 测试用 MockRuntime：一个最小可用的内存后端实现。
//!
//! 实现 [`Runtime`] + [`State`]，用于 `TerminalModel` 的纯逻辑单元测试，
//! 不依赖任何 I/O 或 GUI。覆盖常见 `Task`（split / close / switch / next / prev /
//! new window / close window / send keys / write raw / resize / shutdown）的行为，
//! 让 model 层测试有可验证的真实后端。

#![allow(dead_code)]

use super::*;
#[allow(unused_imports)]
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{PaneInfo, TabInfo};
use crate::core::model::task::Task;
use crate::core::types::{PaneId, TabId};
use std::sync::{Arc, Mutex};

/// 最小可用的 mock backend，用于 trait 编译检查 + TerminalModel 单元测试。
pub struct MockRuntime {
    pub(crate) workspace_name: String,
    pub(crate) workspace_runtime: String,
    pub(crate) tabs: Vec<TabInfo>,
    pub(crate) panes: Vec<PaneInfo>,
    pub(crate) layouts: Vec<TabLayout>,
    pub outputs: Vec<(PaneId, Vec<u8>)>,
    pub(crate) status: BackendStatus,
    pub(crate) events: Vec<StateChange>,
    pub executed: Vec<Task>,
    /// 可选共享执行日志：池淘汰测试在 Workspace 被移出后仍能检查 Detach/Shutdown。
    pub executed_log: Option<Arc<Mutex<Vec<Task>>>>,
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRuntime {
    pub fn new() -> Self {
        Self {
            workspace_name: "mock".into(),
            workspace_runtime: "tmux".into(),
            tabs: vec![],
            panes: vec![],
            layouts: vec![],
            outputs: vec![],
            status: BackendStatus::Disconnected,
            events: vec![],
            executed: vec![],
            executed_log: None,
        }
    }

    /// 预置一个单 pane 的最简状态，并标记 Connected。
    pub fn with_single_pane() -> Self {
        let mut b = Self::new();
        b.tabs.push(TabInfo {
            id: TabId(1),
            name: "t1".into(),
            active: true,
        });
        b.panes.push(PaneInfo {
            id: PaneId(1),
            tab: TabId(1),
            active: true,
            title: "bash".into(),
            cols: 80,
            rows: 24,
        });
        b.layouts.push(TabLayout {
            tab: TabId(1),
            tree: LayoutNode::leaf(PaneId(1)),
            active: PaneId(1),
        });
        b.outputs.push((PaneId(1), Vec::new()));
        b.status = BackendStatus::Connected;
        b
    }
}

impl State for MockRuntime {
    fn workspace_name(&self) -> &str {
        &self.workspace_name
    }
    fn workspace_runtime(&self) -> &str {
        &self.workspace_runtime
    }
    fn active_tab(&self) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| t.active)
    }
    fn active_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.active)
    }
    fn tabs(&self) -> Vec<&TabInfo> {
        self.tabs.iter().collect()
    }
    fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| &t.id == tab)
    }
    fn layout(&self, tab: &TabId) -> Option<&TabLayout> {
        self.layouts.iter().find(|l| &l.tab == tab)
    }
    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
        self.panes.iter().filter(|p| &p.tab == tab).collect()
    }
    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| &p.id == pane)
    }
    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
        self.outputs
            .iter()
            .find(|(pid, _)| pid == pane)
            .map(|(_, v)| v.as_slice())
    }
    fn status(&self) -> BackendStatus {
        self.status
    }
}

#[async_trait]
impl Runtime for MockRuntime {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        self.status = BackendStatus::Connected;
        self.events
            .push(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> anyhow::Result<TaskOutcome> {
        self.executed.push(task.clone());
        if let Some(log) = &self.executed_log {
            log.lock().unwrap().push(task.clone());
        }
        let outcome = match task {
            Task::SplitPane { target, dir, .. } => {
                let target = target.unwrap_or(PaneId(1));
                let tab_id = self
                    .panes
                    .iter()
                    .find(|p| p.id == target)
                    .map(|p| p.tab)
                    .unwrap_or(TabId(1));
                let new_id = PaneId(self.panes.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
                // 取消旧 active，设置新 pane 为 active
                for p in self.panes.iter_mut() {
                    if p.tab == tab_id {
                        p.active = p.id == new_id;
                    }
                }
                self.panes.push(PaneInfo {
                    id: new_id,
                    tab: tab_id,
                    active: true,
                    title: "bash".into(),
                    cols: 40,
                    rows: 24,
                });
                self.outputs.push((new_id, Vec::new()));
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.tab == tab_id) {
                    wl.tree.split_at(target, new_id, *dir);
                    wl.active = new_id;
                    self.events.push(StateChange::PaneAdded {
                        pane: new_id,
                        tab: tab_id,
                    });
                    self.events.push(StateChange::LayoutChanged {
                        tab: tab_id,
                        layout: wl.clone(),
                    });
                    self.events.push(StateChange::ActivePaneChanged {
                        tab: tab_id,
                        pane: new_id,
                    });
                }
                TaskOutcome::Done
            }
            Task::ClosePane { target } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                let tab_id = self
                    .panes
                    .iter()
                    .find(|p| p.id == *target)
                    .map(|p| p.tab)
                    .unwrap_or(TabId(1));
                let was_active = self
                    .panes
                    .iter()
                    .find(|p| p.id == *target)
                    .map(|p| p.active)
                    .unwrap_or(false);
                self.panes.retain(|p| p.id != *target);
                self.outputs.retain(|(pid, _)| pid != target);
                // 更新布局树（移除叶子）
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.tab == tab_id) {
                    let _ = wl.tree.remove(*target);
                    if wl.active == *target {
                        // active 切到剩余的第一个 pane
                        wl.active = wl.tree.leaves().first().copied().unwrap_or(*target);
                    }
                    self.events.push(StateChange::PaneClosed { pane: *target });
                    self.events.push(StateChange::LayoutChanged {
                        tab: tab_id,
                        layout: wl.clone(),
                    });
                }
                // 更新 panes 的 active 标记
                if was_active {
                    let new_active = self
                        .layouts
                        .iter()
                        .find(|l| l.tab == tab_id)
                        .map(|l| l.active);
                    for p in self.panes.iter_mut() {
                        if p.tab == tab_id {
                            p.active = Some(p.id) == new_active;
                        }
                    }
                    if let Some(a) = new_active {
                        self.events.push(StateChange::ActivePaneChanged {
                            tab: tab_id,
                            pane: a,
                        });
                    }
                }
                TaskOutcome::Done
            }
            Task::SwitchPane { target } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                let tab_id = self.panes.iter().find(|p| p.id == *target).unwrap().tab;
                for p in self.panes.iter_mut() {
                    if p.tab == tab_id {
                        p.active = p.id == *target;
                    }
                }
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.tab == tab_id) {
                    wl.active = *target;
                }
                self.events.push(StateChange::ActivePaneChanged {
                    tab: tab_id,
                    pane: *target,
                });
                TaskOutcome::Done
            }
            Task::NextPane | Task::PrevPane => {
                // model 层已 resolve 成 SwitchPane，这里兜底：按布局算下一个/上一个
                let active = self.panes.iter().find(|p| p.active).map(|p| (p.id, p.tab));
                if let Some((active_id, tab_id)) = active {
                    let wl = self.layouts.iter().find(|l| l.tab == tab_id);
                    let next = match task {
                        Task::NextPane => wl.and_then(|l| l.tree.next_leaf(active_id)),
                        Task::PrevPane => wl.and_then(|l| l.tree.prev_leaf(active_id)),
                        _ => None,
                    };
                    if let Some(n) = next {
                        for p in self.panes.iter_mut() {
                            if p.tab == tab_id {
                                p.active = p.id == n;
                            }
                        }
                        if let Some(wl) = self.layouts.iter_mut().find(|l| l.tab == tab_id) {
                            wl.active = n;
                        }
                        self.events.push(StateChange::ActivePaneChanged {
                            tab: tab_id,
                            pane: n,
                        });
                    }
                }
                TaskOutcome::Done
            }
            Task::RenameWorkspace { name } => {
                self.workspace_name = name.clone();
                self.events
                    .push(StateChange::WorkspaceRenamed { name: name.clone() });
                TaskOutcome::Done
            }
            Task::SendKeys { target, keys } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                use crate::core::protocol::terminal::input::encode;
                let mut buf = Vec::new();
                for k in keys {
                    buf.extend_from_slice(&encode(k));
                }
                if let Some(slot) = self.outputs.iter_mut().find(|(pid, _)| pid == target) {
                    slot.1.extend_from_slice(&buf);
                }
                self.events.push(StateChange::PaneOutput {
                    pane: *target,
                    data: buf,
                });
                TaskOutcome::Done
            }
            Task::WriteRaw { target, data } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                if let Some(slot) = self.outputs.iter_mut().find(|(pid, _)| pid == target) {
                    slot.1.extend_from_slice(data);
                }
                self.events.push(StateChange::PaneOutput {
                    pane: *target,
                    data: data.clone(),
                });
                TaskOutcome::Done
            }
            Task::ReportPaneColours { .. } => TaskOutcome::Done,
            Task::TogglePaneFullscreen { .. } => TaskOutcome::Done,
            Task::ResizePane { target, cols, rows } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                if let Some(p) = self.panes.iter_mut().find(|p| p.id == *target) {
                    p.cols = *cols;
                    p.rows = *rows;
                }
                self.events.push(StateChange::PaneResized {
                    pane: *target,
                    cols: *cols,
                    rows: *rows,
                });
                TaskOutcome::Done
            }
            Task::ResizeClient { .. } => TaskOutcome::Done,
            Task::ResizePaneAxis { target, dir, size } => {
                let Some(pane) = self.panes.iter_mut().find(|p| p.id == *target) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                match dir {
                    SplitDir::Horizontal => pane.cols = *size,
                    SplitDir::Vertical => pane.rows = *size,
                }
                self.events.push(StateChange::PaneResized {
                    pane: *target,
                    cols: pane.cols,
                    rows: pane.rows,
                });
                TaskOutcome::Done
            }
            Task::ResizePaneStep { target, .. } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                TaskOutcome::Done
            }
            Task::NewTab {
                name,
                command: _,
                workdir: _,
            } => {
                let new_tab = TabId(self.tabs.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
                let new_pane = PaneId(self.panes.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
                for t in self.tabs.iter_mut() {
                    t.active = false;
                }
                self.tabs.push(TabInfo {
                    id: new_tab,
                    name: name.clone().unwrap_or_else(|| format!("t{}", new_tab.0)),
                    active: true,
                });
                self.panes.push(PaneInfo {
                    id: new_pane,
                    tab: new_tab,
                    active: true,
                    title: "bash".into(),
                    cols: 80,
                    rows: 24,
                });
                self.outputs.push((new_pane, Vec::new()));
                self.layouts.push(TabLayout {
                    tab: new_tab,
                    tree: LayoutNode::leaf(new_pane),
                    active: new_pane,
                });
                self.events.push(StateChange::TabAdded { tab: new_tab });
                self.events.push(StateChange::PaneAdded {
                    pane: new_pane,
                    tab: new_tab,
                });
                TaskOutcome::Done
            }
            Task::CloseTab { target } => {
                if !self.tabs.iter().any(|t| t.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 不存在"),
                    });
                }
                self.panes.retain(|p| p.tab != *target);
                self.layouts.retain(|l| l.tab != *target);
                self.tabs.retain(|t| t.id != *target);
                self.events.push(StateChange::TabClosed { tab: *target });
                TaskOutcome::Done
            }
            Task::SwitchTab { target } => {
                if !self.tabs.iter().any(|t| t.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 不存在"),
                    });
                }
                for t in self.tabs.iter_mut() {
                    t.active = t.id == *target;
                }
                self.events
                    .push(StateChange::ActiveTabChanged { tab: *target });
                TaskOutcome::Done
            }
            Task::RenameTab { target, name } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == *target) {
                    t.name = name.clone();
                }
                self.events.push(StateChange::TabRenamed {
                    tab: *target,
                    name: name.clone(),
                });
                TaskOutcome::Done
            }
            Task::Detach => {
                self.status = BackendStatus::Disconnected;
                self.events.push(StateChange::BackendStatusChanged(
                    BackendStatus::Disconnected,
                ));
                TaskOutcome::Done
            }
            Task::Shutdown => {
                self.status = BackendStatus::Exited;
                self.events
                    .push(StateChange::BackendStatusChanged(BackendStatus::Exited));
                TaskOutcome::Done
            }
        };
        Ok(outcome)
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        std::mem::take(&mut self.events)
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.status = BackendStatus::Exited;
        self.events
            .push(StateChange::BackendStatusChanged(BackendStatus::Exited));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn mock_runtime_connect_and_split() {
        let mut b = MockRuntime::with_single_pane();
        assert_eq!(b.runtime_status(), BackendStatus::Connected);
        let events = b.take_events();
        assert!(events.is_empty()); // with_single_pane 不预置事件

        // 执行 split
        let task = Task::SplitPane {
            target: None,
            dir: SplitDir::Horizontal,
            command: None,
            workdir: None,
        };
        let outcome = b.execute(&task).unwrap();
        assert_eq!(outcome, TaskOutcome::Done);
        assert_eq!(b.executed.len(), 1);

        // 应该产生 3 个事件：PaneAdded、LayoutChanged、ActivePaneChanged
        let events = b.take_events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            StateChange::PaneAdded {
                pane: PaneId(2),
                ..
            }
        ));
        assert!(matches!(events[1], StateChange::LayoutChanged { .. }));
        assert!(matches!(
            events[2],
            StateChange::ActivePaneChanged {
                pane: PaneId(2),
                ..
            }
        ));

        // 验证状态更新
        assert_eq!(b.panes(&TabId(1)).len(), 2);
        assert_eq!(b.active_pane().unwrap().id, PaneId(2));
    }

    #[tokio::test]
    async fn mock_runtime_shutdown() {
        let mut b = MockRuntime::with_single_pane();
        b.shutdown().await.unwrap();
        assert_eq!(b.runtime_status(), BackendStatus::Exited);
        let events = b.take_events();
        assert!(matches!(
            events[0],
            StateChange::BackendStatusChanged(BackendStatus::Exited)
        ));
    }

    #[test]
    fn mock_runtime_take_events_drains() {
        let mut b = MockRuntime::with_single_pane();
        b.events.push(StateChange::PoolChanged);
        b.events.push(StateChange::PoolChanged);
        assert_eq!(b.take_events().len(), 2);
        assert!(b.take_events().is_empty()); // 已排空
    }
}
