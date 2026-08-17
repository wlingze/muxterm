//! HerdrRuntime：绑定一个 Herdr workspace 的 Runtime 视图。
//!
//! 一个 `HerdrSession`（Arc）可被多个 `HerdrRuntime` 共享；每个 Runtime
//! 只填一个 Muxterm Workspace。直播字节走 observe 流（client socket），
//! attach 快照用 `pane.read`，输入走 API socket `pane.send_input/keys`。

use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Arc};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{BackendStatus, PaneInfo, State, StateChange, TabInfo};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::input::KeyEvent;
use crate::core::types::{PaneId, TabId};

use super::observe::{ObserveEvent, ObserveStream};
use super::session::{HerdrSession, SessionSnapshot};

/// HerdrRuntime 支持的能力（v1 不含 WorktreeRemove）。
const HERDR_CAPABILITIES: &[RuntimeCapability] = &[
    RuntimeCapability::PersistDetach,
    RuntimeCapability::Discover,
    RuntimeCapability::MultiTab,
    RuntimeCapability::SplitPane,
    RuntimeCapability::WorktreeList,
    RuntimeCapability::WorktreeCreate,
    RuntimeCapability::WorktreeOpen,
];

/// 绑定一个 Herdr workspace 的 Runtime。
pub struct HerdrRuntime {
    session: Arc<HerdrSession>,
    workspace_id: String,
    workspace_name: String,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    layouts: HashMap<TabId, TabLayout>,
    outputs: HashMap<PaneId, Vec<u8>>,
    status: BackendStatus,
    active_tab: Option<TabId>,
    active_pane: Option<PaneId>,
    events: VecDeque<StateChange>,
    herdr_tab_to_tab: HashMap<String, TabId>,
    herdr_pane_to_pane: HashMap<String, PaneId>,
    tab_to_herdr_tab: HashMap<TabId, String>,
    pane_to_herdr_pane: HashMap<PaneId, String>,
    observe_rx: Option<mpsc::Receiver<ObserveEvent>>,
    observe_streams: Vec<ObserveStream>,
}

impl HerdrRuntime {
    /// 绑定共享 session + 一个 Herdr workspace_id（如 `w1`）。
    pub fn new(session: Arc<HerdrSession>, workspace_id: impl Into<String>) -> Self {
        Self {
            session,
            workspace_id: workspace_id.into(),
            workspace_name: String::new(),
            tabs: vec![],
            panes: vec![],
            layouts: HashMap::new(),
            outputs: HashMap::new(),
            status: BackendStatus::Disconnected,
            active_tab: None,
            active_pane: None,
            events: VecDeque::new(),
            herdr_tab_to_tab: HashMap::new(),
            herdr_pane_to_pane: HashMap::new(),
            tab_to_herdr_tab: HashMap::new(),
            pane_to_herdr_pane: HashMap::new(),
            observe_rx: None,
            observe_streams: vec![],
        }
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn session(&self) -> &HerdrSession {
        &self.session
    }

    /// 共享的 session Arc（H3 测试断言同一 socket 只建一条）。
    pub fn session_arc(&self) -> &Arc<HerdrSession> {
        &self.session
    }

    /// 测试/诊断：当前绑定的 Herdr workspace id。
    pub fn test_workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// 把 snapshot 里属于本 workspace 的 tab/pane/layout 填进产品状态。
    fn apply_snapshot(&mut self, snap: &SessionSnapshot) {
        let Some(ws) = snap
            .workspaces
            .iter()
            .find(|w| w.workspace_id == self.workspace_id)
        else {
            return;
        };
        self.workspace_name = ws.label.clone();

        self.tabs.clear();
        self.panes.clear();
        self.layouts.clear();
        self.herdr_tab_to_tab.clear();
        self.herdr_pane_to_pane.clear();
        self.tab_to_herdr_tab.clear();
        self.pane_to_herdr_pane.clear();

        for tab in snap
            .tabs
            .iter()
            .filter(|t| t.workspace_id == self.workspace_id)
        {
            let id = TabId(numeric_suffix(&tab.tab_id));
            self.herdr_tab_to_tab.insert(tab.tab_id.clone(), id);
            self.tab_to_herdr_tab.insert(id, tab.tab_id.clone());
            self.tabs.push(TabInfo {
                id,
                name: tab.label.clone(),
                active: false,
            });
        }
        for pane in snap
            .panes
            .iter()
            .filter(|p| p.workspace_id == self.workspace_id)
        {
            let id = PaneId(numeric_suffix(&pane.pane_id));
            let tab = self
                .herdr_tab_to_tab
                .get(&pane.tab_id)
                .copied()
                .unwrap_or(TabId(1));
            self.herdr_pane_to_pane.insert(pane.pane_id.clone(), id);
            self.pane_to_herdr_pane.insert(id, pane.pane_id.clone());
            let (cols, rows) = snap
                .layouts
                .iter()
                .find(|l| l.tab_id == pane.tab_id)
                .map(|l| (l.width, l.height))
                .unwrap_or((80, 24));
            self.panes.push(PaneInfo {
                id,
                tab,
                active: false,
                title: pane
                    .cwd
                    .as_deref()
                    .map(|c| {
                        c.rsplit('/')
                            .next()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("herdr")
                            .to_string()
                    })
                    .unwrap_or_else(|| "herdr".into()),
                cols,
                rows,
            });
        }

        // 布局：单 pane 直接 leaf；多 pane 按顺序水平兜底。
        for layout in snap
            .layouts
            .iter()
            .filter(|l| l.workspace_id == self.workspace_id)
        {
            let tab = self
                .herdr_tab_to_tab
                .get(&layout.tab_id)
                .copied()
                .unwrap_or(TabId(1));
            let mut leaves: Vec<PaneId> = layout
                .panes
                .iter()
                .filter_map(|p| self.herdr_pane_to_pane.get(p).copied())
                .collect();
            if leaves.is_empty() {
                leaves = self
                    .panes
                    .iter()
                    .filter(|p| p.tab == tab)
                    .map(|p| p.id)
                    .collect();
            }
            let tree = if leaves.is_empty() {
                LayoutNode::leaf(PaneId(0))
            } else {
                let mut tree = LayoutNode::leaf(leaves[0]);
                for p in &leaves[1..] {
                    tree.split_at(leaves[0], *p, SplitDir::Horizontal);
                }
                tree
            };
            let active = snap
                .focused_pane_id
                .as_deref()
                .and_then(|f| self.herdr_pane_to_pane.get(f).copied())
                .filter(|p| leaves.contains(p))
                .unwrap_or(leaves[0]);
            self.layouts.insert(tab, TabLayout { tab, tree, active });
        }

        // active 标记。
        if let Some(focused) = snap.focused_tab_id.as_deref() {
            if let Some(tab) = self.herdr_tab_to_tab.get(focused) {
                self.active_tab = Some(*tab);
                for t in self.tabs.iter_mut() {
                    t.active = t.id == *tab;
                }
            }
        }
        if let Some(focused) = snap.focused_pane_id.as_deref() {
            if let Some(pane) = self.herdr_pane_to_pane.get(focused) {
                self.active_pane = Some(*pane);
                for p in self.panes.iter_mut() {
                    p.active = p.id == *pane;
                }
            }
        }
        if self.active_tab.is_none() {
            self.active_tab = self.tabs.first().map(|t| t.id);
        }
        if self.active_pane.is_none() {
            self.active_pane = self.panes.first().map(|p| p.id);
        }

        // 事件：拓扑 + 激活。
        for tab in &self.tabs {
            self.events.push_back(StateChange::TabAdded { tab: tab.id });
        }
        for pane in &self.panes {
            self.events.push_back(StateChange::PaneAdded {
                pane: pane.id,
                tab: pane.tab,
            });
        }
        for (tab, layout) in &self.layouts {
            self.events.push_back(StateChange::LayoutChanged {
                tab: *tab,
                layout: layout.clone(),
            });
        }
        if let Some(tab) = self.active_tab {
            self.events.push_back(StateChange::ActiveTabChanged { tab });
        }
        if let Some((tab, pane)) = self.active_tab.zip(self.active_pane) {
            self.events
                .push_back(StateChange::ActivePaneChanged { tab, pane });
        }
    }

    /// 用 `pane.read` 播种 attach 快照（禁止当直播轮询）。
    fn seed_pane_read(&mut self) {
        let panes: Vec<(PaneId, String)> = self
            .panes
            .iter()
            .filter_map(|p| {
                self.pane_to_herdr_pane
                    .get(&p.id)
                    .cloned()
                    .map(|h| (p.id, h))
            })
            .collect();
        for (pane, herdr_pane) in panes {
            match self.session.pane_read_ansi(&herdr_pane) {
                Ok(bytes) if !bytes.is_empty() => {
                    self.outputs
                        .entry(pane)
                        .or_default()
                        .extend_from_slice(&bytes);
                    self.events
                        .push_back(StateChange::PaneOutput { pane, data: bytes });
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(
                        target = "muxterm::herdr",
                        pane = %herdr_pane,
                        error = %err,
                        "pane.read 快照失败"
                    );
                }
            }
        }
    }

    /// 为每个 pane 起 observe 流（直播字节）。
    fn start_observe_streams(&mut self) {
        let (tx, rx) = mpsc::channel::<ObserveEvent>();
        self.observe_rx = Some(rx);
        let socket = self.session.client_socket_path().to_path_buf();
        let panes: Vec<(PaneId, String, u16, u16)> = self
            .panes
            .iter()
            .filter_map(|p| {
                self.pane_to_herdr_pane
                    .get(&p.id)
                    .cloned()
                    .map(|h| (p.id, h, p.cols, p.rows))
            })
            .collect();
        for (pane, herdr_pane, cols, rows) in panes {
            match ObserveStream::start(&socket, &herdr_pane, pane, cols, rows, tx.clone()) {
                Ok(stream) => self.observe_streams.push(stream),
                Err(err) => {
                    tracing::warn!(
                        target = "muxterm::herdr",
                        pane = %herdr_pane,
                        error = %err,
                        "observe 流启动失败"
                    );
                }
            }
        }
    }

    /// 取 observe 事件并转成 StateChange。
    fn drain_observe(&mut self) {
        let Some(rx) = &self.observe_rx else {
            return;
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                ObserveEvent::Frame {
                    pane,
                    bytes,
                    width,
                    height,
                    full: _,
                } => {
                    if let Some(p) = self.panes.iter_mut().find(|p| p.id == pane) {
                        p.cols = width;
                        p.rows = height;
                    }
                    self.outputs
                        .entry(pane)
                        .or_default()
                        .extend_from_slice(&bytes);
                    self.events
                        .push_back(StateChange::PaneOutput { pane, data: bytes });
                }
                ObserveEvent::Closed { pane, reason } => {
                    tracing::info!(
                        target = "muxterm::herdr",
                        pane = %pane,
                        reason = ?reason,
                        "observe 流关闭"
                    );
                }
                ObserveEvent::Error { pane, message } => {
                    tracing::warn!(
                        target = "muxterm::herdr",
                        pane = %pane,
                        error = %message,
                        "observe 流错误"
                    );
                }
            }
        }
    }

    /// 把 KeyEvent 映射成 herdr key-combo 字符串。
    fn key_to_herdr(key: &KeyEvent) -> String {
        match key {
            KeyEvent::Char(c) => c.to_string(),
            KeyEvent::Enter => "enter".into(),
            KeyEvent::Tab => "tab".into(),
            KeyEvent::Backspace => "backspace".into(),
            KeyEvent::Escape => "esc".into(),
            KeyEvent::Ctrl(c) => format!("ctrl+{c}"),
            KeyEvent::Alt(c) => format!("alt+{c}"),
            KeyEvent::Function(n) => format!("f{n}"),
            KeyEvent::Arrow(dir) => match dir {
                crate::core::protocol::terminal::input::ArrowDir::Up => "up".into(),
                crate::core::protocol::terminal::input::ArrowDir::Down => "down".into(),
                crate::core::protocol::terminal::input::ArrowDir::Left => "left".into(),
                crate::core::protocol::terminal::input::ArrowDir::Right => "right".into(),
            },
        }
    }

    fn herdr_pane(&self, pane: PaneId) -> Option<&str> {
        self.pane_to_herdr_pane.get(&pane).map(String::as_str)
    }
}

/// `w1:t1` / `w1:p1` 的数字后缀 → 产品 id。
fn numeric_suffix(id: &str) -> u32 {
    id.rsplit(':')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

impl State for HerdrRuntime {
    fn workspace_name(&self) -> &str {
        if self.workspace_name.is_empty() {
            self.session.name()
        } else {
            &self.workspace_name
        }
    }

    fn workspace_runtime(&self) -> &str {
        "herdr"
    }

    fn active_tab(&self) -> Option<&TabInfo> {
        self.active_tab
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    fn active_pane(&self) -> Option<&PaneInfo> {
        self.active_pane
            .and_then(|id| self.panes.iter().find(|p| p.id == id))
    }

    fn tabs(&self) -> Vec<&TabInfo> {
        self.tabs.iter().collect()
    }

    fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| &t.id == tab)
    }

    fn layout(&self, tab: &TabId) -> Option<&TabLayout> {
        self.layouts.get(tab)
    }

    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
        self.panes.iter().filter(|p| &p.tab == tab).collect()
    }

    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| &p.id == pane)
    }

    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
        self.outputs.get(pane).map(Vec::as_slice)
    }

    fn status(&self) -> BackendStatus {
        self.status
    }
}

#[async_trait]
impl Runtime for HerdrRuntime {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        HERDR_CAPABILITIES
    }

    async fn connect(&mut self) -> Result<()> {
        if self.status == BackendStatus::Connected {
            return Ok(());
        }
        self.status = BackendStatus::Connecting;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connecting));

        self.session
            .ping()
            .context("Herdr ping 失败（隔离 named session）")?;
        let snap = self
            .session
            .snapshot()
            .context("Herdr session.snapshot 失败")?;
        self.apply_snapshot(&snap);
        if self.panes.is_empty() {
            return Err(anyhow!(
                "Herdr workspace {} 在 snapshot 里没有 pane",
                self.workspace_id
            ));
        }
        self.seed_pane_read();
        self.start_observe_streams();

        self.status = BackendStatus::Connected;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> Result<TaskOutcome> {
        if self.status != BackendStatus::Connected {
            return Ok(TaskOutcome::Rejected {
                reason: "Herdr 未连接".into(),
            });
        }
        match task {
            Task::WriteRaw { target, data } => {
                let Some(herdr_pane) = self.herdr_pane(*target) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                let text = String::from_utf8_lossy(data);
                self.session
                    .pane_send_input(herdr_pane, &text)
                    .map_err(|e| anyhow!("pane.send_input 失败: {e}"))?;
                Ok(TaskOutcome::Done)
            }
            Task::SendKeys { target, keys } => {
                let Some(herdr_pane) = self.herdr_pane(*target) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                let mapped: Vec<String> = keys.iter().map(Self::key_to_herdr).collect();
                self.session
                    .pane_send_keys(herdr_pane, &mapped)
                    .map_err(|e| anyhow!("pane.send_keys 失败: {e}"))?;
                Ok(TaskOutcome::Done)
            }
            Task::SwitchPane { target } => {
                let Some(pane) = self.panes.iter().find(|p| p.id == *target) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                let tab = pane.tab;
                for p in self.panes.iter_mut() {
                    if p.tab == tab {
                        p.active = p.id == *target;
                    }
                }
                if let Some(l) = self.layouts.get_mut(&tab) {
                    l.active = *target;
                }
                self.active_pane = Some(*target);
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab, pane: *target });
                Ok(TaskOutcome::Done)
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
                self.active_tab = Some(*target);
                self.events
                    .push_back(StateChange::ActiveTabChanged { tab: *target });
                Ok(TaskOutcome::Done)
            }
            Task::NextPane | Task::PrevPane => {
                let Some(active) = self.active_pane else {
                    return Ok(TaskOutcome::Rejected {
                        reason: "无激活 pane".into(),
                    });
                };
                let tab = self
                    .panes
                    .iter()
                    .find(|p| p.id == active)
                    .map(|p| p.tab)
                    .unwrap_or(TabId(1));
                let leaves = self
                    .layouts
                    .get(&tab)
                    .map(|l| l.tree.leaves())
                    .unwrap_or_default();
                let idx = leaves.iter().position(|p| *p == active);
                let next = match (task, idx) {
                    (Task::NextPane, Some(i)) => leaves.get(i + 1).or_else(|| leaves.first()),
                    (Task::PrevPane, Some(i)) => {
                        if i == 0 {
                            leaves.last()
                        } else {
                            leaves.get(i - 1)
                        }
                    }
                    _ => None,
                };
                if let Some(next) = next {
                    return self.execute(&Task::SwitchPane { target: *next });
                }
                Ok(TaskOutcome::Done)
            }
            Task::NewTab {
                name,
                command: _,
                workdir: _,
            } => {
                let result = self
                    .session
                    .call(
                        "tab.create",
                        serde_json::json!({
                            "workspace_id": self.workspace_id,
                            "label": name.clone().unwrap_or_default(),
                        }),
                    )
                    .map_err(|e| anyhow!("tab.create 失败: {e}"))?;
                self.apply_created_tab(&result);
                Ok(TaskOutcome::Done)
            }
            Task::SplitPane {
                target,
                dir,
                command: _,
                workdir: _,
            } => {
                let target = target.unwrap_or_else(|| self.active_pane.unwrap_or(PaneId(1)));
                let Some(herdr_pane) = self.herdr_pane(target) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                let direction = match dir {
                    SplitDir::Horizontal => "right",
                    SplitDir::Vertical => "down",
                };
                let result = self
                    .session
                    .call(
                        "pane.split",
                        serde_json::json!({
                            "pane_id": herdr_pane,
                            "direction": direction,
                        }),
                    )
                    .map_err(|e| anyhow!("pane.split 失败: {e}"))?;
                self.apply_split_pane(&result);
                Ok(TaskOutcome::Done)
            }
            Task::Detach => {
                self.status = BackendStatus::Disconnected;
                self.events.push_back(StateChange::BackendStatusChanged(
                    BackendStatus::Disconnected,
                ));
                Ok(TaskOutcome::Done)
            }
            Task::Shutdown => {
                self.status = BackendStatus::Exited;
                self.events
                    .push_back(StateChange::BackendStatusChanged(BackendStatus::Exited));
                Ok(TaskOutcome::Done)
            }
            _ => Ok(TaskOutcome::Rejected {
                reason: format!("Herdr v1 未实现 Task {task:?}"),
            }),
        }
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        self.drain_observe();
        self.events.drain(..).collect()
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.observe_streams.clear();
        self.observe_rx = None;
        self.status = BackendStatus::Disconnected;
        self.events.push_back(StateChange::BackendStatusChanged(
            BackendStatus::Disconnected,
        ));
        Ok(())
    }
}

impl HerdrRuntime {
    /// tab.create 响应 → 新 tab + root pane 进状态。
    fn apply_created_tab(&mut self, result: &serde_json::Value) {
        let Some(tab_id) = result
            .get("tab")
            .and_then(|t| t.get("tab_id"))
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let id = TabId(numeric_suffix(tab_id));
        if self.herdr_tab_to_tab.contains_key(tab_id) {
            return;
        }
        self.herdr_tab_to_tab.insert(tab_id.to_string(), id);
        self.tab_to_herdr_tab.insert(id, tab_id.to_string());
        self.tabs.push(TabInfo {
            id,
            name: result
                .get("tab")
                .and_then(|t| t.get("label"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("new")
                .to_string(),
            active: true,
        });
        if let Some(pane_id) = result
            .get("root_pane")
            .and_then(|p| p.get("pane_id"))
            .and_then(serde_json::Value::as_str)
        {
            let pid = PaneId(numeric_suffix(pane_id));
            self.herdr_pane_to_pane.insert(pane_id.to_string(), pid);
            self.pane_to_herdr_pane.insert(pid, pane_id.to_string());
            self.panes.push(PaneInfo {
                id: pid,
                tab: id,
                active: true,
                title: "herdr".into(),
                cols: 80,
                rows: 24,
            });
            self.layouts.insert(
                id,
                TabLayout {
                    tab: id,
                    tree: LayoutNode::leaf(pid),
                    active: pid,
                },
            );
            self.events.push_back(StateChange::TabAdded { tab: id });
            self.events
                .push_back(StateChange::PaneAdded { pane: pid, tab: id });
            self.events.push_back(StateChange::LayoutChanged {
                tab: id,
                layout: self.layouts[&id].clone(),
            });
            self.events
                .push_back(StateChange::ActiveTabChanged { tab: id });
            self.events
                .push_back(StateChange::ActivePaneChanged { tab: id, pane: pid });
        }
    }

    /// pane.split 响应 → 新 pane 进状态。
    fn apply_split_pane(&mut self, result: &serde_json::Value) {
        let Some(pane_id) = result
            .get("pane")
            .and_then(|p| p.get("pane_id"))
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let pid = PaneId(numeric_suffix(pane_id));
        if self.herdr_pane_to_pane.contains_key(pane_id) {
            return;
        }
        let tab = self.active_tab.unwrap_or(TabId(1));
        self.herdr_pane_to_pane.insert(pane_id.to_string(), pid);
        self.pane_to_herdr_pane.insert(pid, pane_id.to_string());
        self.panes.push(PaneInfo {
            id: pid,
            tab,
            active: true,
            title: "herdr".into(),
            cols: 80,
            rows: 24,
        });
        if let Some(l) = self.layouts.get_mut(&tab) {
            let base = l.active;
            l.tree.split_at(base, pid, SplitDir::Horizontal);
            l.active = pid;
        }
        self.events
            .push_back(StateChange::PaneAdded { pane: pid, tab });
        if let Some(l) = self.layouts.get(&tab) {
            self.events.push_back(StateChange::LayoutChanged {
                tab,
                layout: l.clone(),
            });
        }
        self.events
            .push_back(StateChange::ActivePaneChanged { tab, pane: pid });
    }
}
