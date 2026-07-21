//! 测试用 MockBackend：一个最小可用的内存后端实现。
//!
//! 实现 [`Backend`] + [`State`]，用于 `TerminalModel` 的纯逻辑单元测试，
//! 不依赖任何 I/O 或 GUI。覆盖常见 `Task`（split / close / switch / next / prev /
//! new window / close window / send keys / write raw / resize / shutdown）的行为，
//! 让 model 层测试有可验证的真实后端。

#![allow(dead_code)]

use super::*;
use crate::core::model::layout::SplitDir;
use crate::core::model::layout::{LayoutNode, WindowLayout};
use crate::core::model::state::{PaneInfo, SessionInfo, WindowInfo};
use crate::core::model::task::Task;
use crate::core::types::{PaneId, SessionId, WindowId};

/// 最小可用的 mock backend，用于 trait 编译检查 + TerminalModel 单元测试。
pub struct MockBackend {
    pub(crate) sessions: Vec<SessionInfo>,
    pub(crate) windows: Vec<WindowInfo>,
    pub(crate) panes: Vec<PaneInfo>,
    pub(crate) layouts: Vec<WindowLayout>,
    pub(crate) outputs: Vec<(PaneId, Vec<u8>)>,
    pub(crate) status: BackendStatus,
    pub(crate) events: Vec<StateChange>,
    pub executed: Vec<Task>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            sessions: vec![],
            windows: vec![],
            panes: vec![],
            layouts: vec![],
            outputs: vec![],
            status: BackendStatus::Disconnected,
            events: vec![],
            executed: vec![],
        }
    }

    /// 预置一个单 pane 的最简状态，并标记 Connected。
    pub fn with_single_pane() -> Self {
        let mut b = Self::new();
        b.sessions.push(SessionInfo {
            id: SessionId(1),
            name: "mock".into(),
            active_window: Some(WindowId(1)),
        });
        b.windows.push(WindowInfo {
            id: WindowId(1),
            name: "w1".into(),
            session: SessionId(1),
            active: true,
        });
        b.panes.push(PaneInfo {
            id: PaneId(1),
            window: WindowId(1),
            active: true,
            title: "bash".into(),
            cols: 80,
            rows: 24,
        });
        b.layouts.push(WindowLayout {
            window: WindowId(1),
            tree: LayoutNode::leaf(PaneId(1)),
            active: PaneId(1),
        });
        b.outputs.push((PaneId(1), Vec::new()));
        b.status = BackendStatus::Connected;
        b
    }
}

impl State for MockBackend {
    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }
    fn active_session(&self) -> Option<&SessionInfo> {
        self.sessions.first()
    }
    fn active_window(&self) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.active)
    }
    fn active_pane(&self) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.active)
    }
    fn layout(&self, window: &WindowId) -> Option<&WindowLayout> {
        self.layouts.iter().find(|l| &l.window == window)
    }
    fn panes(&self, window: &WindowId) -> Vec<&PaneInfo> {
        self.panes.iter().filter(|p| &p.window == window).collect()
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
impl Backend for MockBackend {
    async fn connect(&mut self) -> anyhow::Result<()> {
        self.status = BackendStatus::Connected;
        self.events
            .push(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> anyhow::Result<TaskOutcome> {
        self.executed.push(task.clone());
        let outcome = match task {
            Task::SplitPane { target, dir, .. } => {
                let target = target.unwrap_or(PaneId(1));
                let win_id = self
                    .panes
                    .iter()
                    .find(|p| p.id == target)
                    .map(|p| p.window)
                    .unwrap_or(WindowId(1));
                let new_id = PaneId(self.panes.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
                // 取消旧 active，设置新 pane 为 active
                for p in self.panes.iter_mut() {
                    if p.window == win_id {
                        p.active = p.id == new_id;
                    }
                }
                self.panes.push(PaneInfo {
                    id: new_id,
                    window: win_id,
                    active: true,
                    title: "bash".into(),
                    cols: 40,
                    rows: 24,
                });
                self.outputs.push((new_id, Vec::new()));
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.window == win_id) {
                    wl.tree.split_at(target, new_id, *dir);
                    wl.active = new_id;
                    self.events.push(StateChange::PaneAdded {
                        pane: new_id,
                        window: win_id,
                    });
                    self.events.push(StateChange::LayoutChanged {
                        window: win_id,
                        layout: wl.clone(),
                    });
                    self.events.push(StateChange::ActivePaneChanged {
                        window: win_id,
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
                let win_id = self
                    .panes
                    .iter()
                    .find(|p| p.id == *target)
                    .map(|p| p.window)
                    .unwrap_or(WindowId(1));
                self.panes.retain(|p| p.id != *target);
                self.outputs.retain(|(pid, _)| pid != target);
                let was_active = self.layouts.iter().any(|l| l.active == *target);
                // 更新布局树（移除叶子）
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.window == win_id) {
                    let _ = wl.tree.remove(*target);
                    if wl.active == *target {
                        // active 切到剩余的第一个 pane
                        wl.active = wl.tree.leaves().first().copied().unwrap_or(*target);
                    }
                    self.events.push(StateChange::PaneClosed { pane: *target });
                    self.events.push(StateChange::LayoutChanged {
                        window: win_id,
                        layout: wl.clone(),
                    });
                }
                // 更新 panes 的 active 标记
                if was_active {
                    let new_active = self
                        .layouts
                        .iter()
                        .find(|l| l.window == win_id)
                        .map(|l| l.active);
                    for p in self.panes.iter_mut() {
                        if p.window == win_id {
                            p.active = Some(p.id) == new_active;
                        }
                    }
                    if let Some(a) = new_active {
                        self.events.push(StateChange::ActivePaneChanged {
                            window: win_id,
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
                let win_id = self.panes.iter().find(|p| p.id == *target).unwrap().window;
                for p in self.panes.iter_mut() {
                    if p.window == win_id {
                        p.active = p.id == *target;
                    }
                }
                if let Some(wl) = self.layouts.iter_mut().find(|l| l.window == win_id) {
                    wl.active = *target;
                }
                self.events.push(StateChange::ActivePaneChanged {
                    window: win_id,
                    pane: *target,
                });
                TaskOutcome::Done
            }
            Task::NextPane | Task::PrevPane => {
                // model 层已 resolve 成 SwitchPane，这里兜底：按布局算下一个/上一个
                let active = self
                    .panes
                    .iter()
                    .find(|p| p.active)
                    .map(|p| (p.id, p.window));
                if let Some((active_id, win_id)) = active {
                    let wl = self.layouts.iter().find(|l| l.window == win_id);
                    let next = match task {
                        Task::NextPane => wl.and_then(|l| l.tree.next_leaf(active_id)),
                        Task::PrevPane => wl.and_then(|l| l.tree.prev_leaf(active_id)),
                        _ => None,
                    };
                    if let Some(n) = next {
                        for p in self.panes.iter_mut() {
                            if p.window == win_id {
                                p.active = p.id == n;
                            }
                        }
                        if let Some(wl) = self.layouts.iter_mut().find(|l| l.window == win_id) {
                            wl.active = n;
                        }
                        self.events.push(StateChange::ActivePaneChanged {
                            window: win_id,
                            pane: n,
                        });
                    }
                }
                TaskOutcome::Done
            }
            Task::NewWindow { name, .. } => {
                let new_win = WindowId(self.windows.iter().map(|w| w.id.0).max().unwrap_or(0) + 1);
                let sess = self.sessions.first().map(|s| s.id).unwrap_or(SessionId(1));
                // 旧 window 取消 active
                for w in self.windows.iter_mut() {
                    w.active = false;
                }
                self.windows.push(WindowInfo {
                    id: new_win,
                    name: name.clone().unwrap_or_else(|| format!("w{}", new_win.0)),
                    session: sess,
                    active: true,
                });
                let new_pane = PaneId(self.panes.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
                self.panes.push(PaneInfo {
                    id: new_pane,
                    window: new_win,
                    active: true,
                    title: "bash".into(),
                    cols: 80,
                    rows: 24,
                });
                self.outputs.push((new_pane, Vec::new()));
                self.layouts.push(WindowLayout {
                    window: new_win,
                    tree: LayoutNode::leaf(new_pane),
                    active: new_pane,
                });
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sess) {
                    s.active_window = Some(new_win);
                }
                self.events.push(StateChange::WindowAdded {
                    window: new_win,
                    session: sess,
                });
                self.events.push(StateChange::ActiveWindowChanged {
                    session: sess,
                    window: new_win,
                });
                TaskOutcome::Done
            }
            Task::CloseWindow { target } => {
                if !self.windows.iter().any(|w| w.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {target} 不存在"),
                    });
                }
                let sess = self
                    .windows
                    .iter()
                    .find(|w| w.id == *target)
                    .map(|w| w.session)
                    .unwrap_or(SessionId(1));
                // 移除该 window 下所有 pane
                let to_remove: Vec<PaneId> = self
                    .panes
                    .iter()
                    .filter(|p| p.window == *target)
                    .map(|p| p.id)
                    .collect();
                self.panes.retain(|p| p.window != *target);
                self.outputs.retain(|(pid, _)| !to_remove.contains(pid));
                self.layouts.retain(|l| l.window != *target);
                self.windows.retain(|w| w.id != *target);
                // active window 切到剩余的第一个
                if let Some(w) = self.windows.first() {
                    let wid = w.id;
                    for x in self.windows.iter_mut() {
                        x.active = x.id == wid;
                    }
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sess) {
                        s.active_window = Some(wid);
                    }
                    self.events.push(StateChange::ActiveWindowChanged {
                        session: sess,
                        window: wid,
                    });
                }
                self.events
                    .push(StateChange::WindowClosed { window: *target });
                TaskOutcome::Done
            }
            Task::SwitchWindow { target } => {
                if !self.windows.iter().any(|w| w.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {target} 不存在"),
                    });
                }
                let sess = self
                    .windows
                    .iter()
                    .find(|w| w.id == *target)
                    .map(|w| w.session)
                    .unwrap_or(SessionId(1));
                for w in self.windows.iter_mut() {
                    w.active = w.id == *target;
                }
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sess) {
                    s.active_window = Some(*target);
                }
                self.events.push(StateChange::ActiveWindowChanged {
                    session: sess,
                    window: *target,
                });
                TaskOutcome::Done
            }
            Task::RenameWindow { target, name } => {
                if !self.windows.iter().any(|w| w.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("window {target} 不存在"),
                    });
                }
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == *target) {
                    w.name = name.clone();
                }
                self.events.push(StateChange::WindowRenamed {
                    window: *target,
                    name: name.clone(),
                });
                TaskOutcome::Done
            }
            Task::SwitchSession { .. } | Task::RenameSession { .. } => {
                // 单 session mock，直接 Done
                TaskOutcome::Done
            }
            Task::SendKeys { target, keys } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                use crate::core::terminal::input::encode;
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
            Task::ResizePaneStep { target, .. } => {
                if !self.panes.iter().any(|p| p.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
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
    async fn mock_backend_connect_and_split() {
        let mut b = MockBackend::with_single_pane();
        assert_eq!(b.backend_status(), BackendStatus::Connected);
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
        assert_eq!(b.panes(&WindowId(1)).len(), 2);
        assert_eq!(b.active_pane().unwrap().id, PaneId(2));
    }

    #[tokio::test]
    async fn mock_backend_shutdown() {
        let mut b = MockBackend::with_single_pane();
        b.shutdown().await.unwrap();
        assert_eq!(b.backend_status(), BackendStatus::Exited);
        let events = b.take_events();
        assert!(matches!(
            events[0],
            StateChange::BackendStatusChanged(BackendStatus::Exited)
        ));
    }

    #[test]
    fn mock_backend_take_events_drains() {
        let mut b = MockBackend::with_single_pane();
        b.events.push(StateChange::SessionsChanged);
        b.events.push(StateChange::SessionsChanged);
        assert_eq!(b.take_events().len(), 2);
        assert!(b.take_events().is_empty()); // 已排空
    }
}
