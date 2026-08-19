//! HerdrRuntime：绑定一个 Herdr workspace 的 Runtime 视图。
//!
//! 一个 `HerdrSession`（Arc）可被多个 `HerdrRuntime` 共享；每个 Runtime
//! 只填一个 Muxterm Workspace。直播字节走 observe 流（client socket），
//! attach 快照用 `pane.read`；原始键盘字节走 `pane.send_text`，语义按键走
//! `pane.send_keys`。逐键输入禁止走会自动包 bracketed-paste 的 `pane.send_input`。

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
use super::session::{
    HerdrSession, LayoutRecord, LayoutRect, LayoutSplitDirection, SessionSnapshot,
};

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
    observe_tx: Option<mpsc::Sender<ObserveEvent>>,
    observe_streams: Vec<ObserveStream>,
    /// SSH 远端 socket 转发进程（Drop/shutdown 时杀掉）。
    forward: Option<std::process::Child>,
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
            observe_tx: None,
            observe_streams: vec![],
            forward: None,
        }
    }

    /// 绑定共享 session + workspace，并接管 SSH socket 转发进程。
    pub fn with_forward(
        session: Arc<HerdrSession>,
        workspace_id: impl Into<String>,
        forward: std::process::Child,
    ) -> Self {
        let mut rt = Self::new(session, workspace_id);
        rt.forward = Some(forward);
        rt
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

    /// 产品 worktree 方法：list 走 session.worktree_list（能力已在 support）。
    pub fn worktrees(&self) -> anyhow::Result<Vec<crate::core::model::backend::WorktreeInfo>> {
        let list = self.session.worktree_list(&self.workspace_id)?;
        Ok(list
            .worktrees
            .into_iter()
            .map(|w| crate::core::model::backend::WorktreeInfo {
                path: w.path,
                branch: w.branch,
                repo_root: w.repo_root,
                // 产品 WorkspaceId 由 session/socket/workspace_id 确定；池层
                // 再按 slots 过滤「是否已打开」。
                open_workspace: w.open_workspace_id.as_deref().map(|wid| {
                    crate::core::workspace::spec::WorkspaceSpec::herdr(
                        self.session.name(),
                        wid,
                        self.session.socket_path().to_string_lossy(),
                    )
                    .id()
                }),
                linked: w.linked,
            })
            .collect())
    }

    /// 创建 worktree：Herdr 建好后返回新格 WorkspaceSpec。
    pub fn create_worktree(
        &self,
        spec: &crate::core::model::backend::WorktreeCreateSpec,
    ) -> anyhow::Result<crate::core::workspace::spec::WorkspaceSpec> {
        let record = self.session.worktree_create(
            &self.workspace_id,
            &spec.branch,
            &spec.path,
            spec.base.as_deref(),
            spec.label.as_deref(),
        )?;
        let new_ws = record
            .open_workspace_id
            .ok_or_else(|| anyhow!("worktree.create 未返回 workspace_id"))?;
        Ok(crate::core::workspace::spec::WorkspaceSpec::herdr(
            self.session.name(),
            new_ws,
            self.session.socket_path().to_string_lossy(),
        ))
    }

    /// 打开已有 checkout：Herdr 返回已有格 WorkspaceSpec。
    pub fn open_worktree(
        &self,
        path: &str,
    ) -> anyhow::Result<crate::core::workspace::spec::WorkspaceSpec> {
        let record = self.session.worktree_open(&self.workspace_id, path)?;
        let new_ws = record
            .open_workspace_id
            .ok_or_else(|| anyhow!("worktree.open 未返回 workspace_id"))?;
        Ok(crate::core::workspace::spec::WorkspaceSpec::herdr(
            self.session.name(),
            new_ws,
            self.session.socket_path().to_string_lossy(),
        ))
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
        self.active_tab = None;
        self.active_pane = None;
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
            let pane_rect = snap
                .layouts
                .iter()
                .find(|l| l.tab_id == pane.tab_id)
                .and_then(|layout| {
                    layout
                        .panes
                        .iter()
                        .find(|candidate| candidate.pane_id == pane.pane_id)
                })
                .map(|pane| pane.rect);
            let (cols, rows) = pane_rect
                .map(|rect| (rect.width, rect.height))
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

        // Herdr 的 PaneLayoutSnapshot 已包含完整 BSP split 路径、方向、ratio
        // 和每个 pane rect；这里必须按权威树重建，不能把多 pane 猜成水平。
        let workspace_id = self.workspace_id.clone();
        for layout in snap
            .layouts
            .iter()
            .filter(|l| l.workspace_id == workspace_id)
        {
            self.apply_layout_record(layout, false);
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

    /// 把一条 Herdr PaneLayoutSnapshot 应用到现有 id 映射与产品状态。
    fn apply_layout_record(&mut self, layout: &LayoutRecord, emit_event: bool) -> bool {
        let Some(tab) = self.herdr_tab_to_tab.get(&layout.tab_id).copied() else {
            return false;
        };

        for source_pane in &layout.panes {
            let Some(pane) = self.herdr_pane_to_pane.get(&source_pane.pane_id).copied() else {
                continue;
            };
            if let Some(info) = self.panes.iter_mut().find(|info| info.id == pane) {
                info.cols = source_pane.rect.width;
                info.rows = source_pane.rect.height;
            }
        }

        let leaves: Vec<PaneId> = layout
            .panes
            .iter()
            .filter_map(|pane| self.herdr_pane_to_pane.get(&pane.pane_id).copied())
            .collect();
        if leaves.is_empty() {
            return false;
        }

        let tree = match layout_tree_from_record(layout, &self.herdr_pane_to_pane) {
            Some(tree) => tree,
            None => {
                if !layout.splits.is_empty() {
                    tracing::warn!(
                        target = "muxterm::herdr",
                        tab = %layout.tab_id,
                        splits = ?layout.splits,
                        "Herdr split tree invalid; using legacy horizontal fallback"
                    );
                }
                legacy_horizontal_tree(&leaves)
            }
        };
        let active = self
            .herdr_pane_to_pane
            .get(&layout.focused_pane_id)
            .copied()
            .filter(|pane| tree.contains(*pane))
            .unwrap_or(leaves[0]);
        let product_layout = TabLayout { tab, tree, active };
        self.layouts.insert(tab, product_layout.clone());
        if emit_event {
            self.events.push_back(StateChange::LayoutChanged {
                tab,
                layout: product_layout,
            });
        }
        true
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
        self.observe_tx = Some(tx.clone());
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

    /// 以新的 Surface 字符格重建一个只读 observer。Herdr observer 本身
    /// 不能 resize；重连会先发一帧与 VTE 网格一致的 full frame。
    fn replace_observe_stream(&mut self, pane: PaneId, cols: u16, rows: u16) -> Result<()> {
        let herdr_pane = self
            .pane_to_herdr_pane
            .get(&pane)
            .cloned()
            .ok_or_else(|| anyhow!("pane {pane} 缺 Herdr id"))?;
        let tx = self
            .observe_tx
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("Herdr observe channel 未启动"))?;
        let stream = ObserveStream::start(
            self.session.client_socket_path(),
            &herdr_pane,
            pane,
            cols,
            rows,
            tx,
        )?;
        self.observe_streams
            .retain(|existing| existing.pane() != pane);
        self.observe_streams.push(stream);
        Ok(())
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

    /// 关闭本 Runtime 自己启动的 SSH 转发，并只清理它在系统临时目录下的
    /// `muxterm-herdr-fwd-*` socket。绝不删除默认 Herdr socket。
    fn stop_forward(&mut self) {
        let Some(mut forward) = self.forward.take() else {
            return;
        };
        let _ = forward.kill();
        let _ = forward.wait();
        let temp_dir = std::env::temp_dir();
        for socket in [
            self.session.socket_path(),
            self.session.client_socket_path(),
        ] {
            let is_ours = socket.parent() == Some(temp_dir.as_path())
                && socket
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("muxterm-herdr-fwd-"));
            if is_ours {
                let _ = std::fs::remove_file(socket);
            }
        }
    }
}

/// Herdr public id 的 bijective base-32 字母表（协议 19）。
const HERDR_PUBLIC_ID_ALPHABET: &[u8; 32] = b"123456789ABCDEFGHJKMNPQRSTVWXYZ0";

/// `w1:t1` / `w1:pR` 的 public 后缀 → 产品 id。
///
/// 后缀不是十进制：pA=10、pR=24、p0=32、p11=33。解析失败才返回
/// 保留值 0；合法字母 ID 绝不能彼此碰撞成 PaneId(0)。
fn numeric_suffix(id: &str) -> u32 {
    id.rsplit(':')
        .next()
        .and_then(|value| value.strip_prefix(['t', 'p']))
        .and_then(|encoded| {
            encoded.bytes().try_fold(0u32, |decoded, byte| {
                let digit = HERDR_PUBLIC_ID_ALPHABET
                    .iter()
                    .position(|candidate| *candidate == byte)?;
                decoded
                    .checked_mul(HERDR_PUBLIC_ID_ALPHABET.len() as u32)?
                    .checked_add(u32::try_from(digit).ok()? + 1)
            })
        })
        .unwrap_or(0)
}

#[derive(Clone, Copy)]
struct MappedLayoutPane {
    id: PaneId,
    rect: LayoutRect,
}

/// 用 Herdr split path 重建 BSP 树。path false/true 分别表示 first/second；
/// pane 没有显式 path，因此按权威 split rect + ratio 将 pane rect 分区。
fn layout_tree_from_record(
    layout: &LayoutRecord,
    pane_ids: &HashMap<String, PaneId>,
) -> Option<LayoutNode> {
    let panes: Vec<MappedLayoutPane> = layout
        .panes
        .iter()
        .filter_map(|pane| {
            pane_ids
                .get(&pane.pane_id)
                .copied()
                .map(|id| MappedLayoutPane {
                    id,
                    rect: pane.rect,
                })
        })
        .collect();
    if panes.is_empty() {
        return None;
    }
    if panes.len() == 1 {
        return Some(LayoutNode::Leaf(panes[0].id));
    }

    let mut splits = HashMap::new();
    for split in &layout.splits {
        if splits.insert(split.path.clone(), split).is_some() {
            return None;
        }
    }
    build_layout_subtree(&[], &panes, &splits)
}

fn build_layout_subtree(
    path: &[bool],
    panes: &[MappedLayoutPane],
    splits: &HashMap<Vec<bool>, &super::session::LayoutSplitRecord>,
) -> Option<LayoutNode> {
    if panes.len() == 1 {
        return Some(LayoutNode::Leaf(panes[0].id));
    }
    let split = splits.get(path)?;
    let dir = match split.direction {
        LayoutSplitDirection::Right => SplitDir::Horizontal,
        LayoutSplitDirection::Down => SplitDir::Vertical,
        LayoutSplitDirection::Unknown(_) => return None,
    };
    let ratio = if split.ratio.is_finite() {
        split.ratio.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let first_span = match dir {
        SplitDir::Horizontal => (f32::from(split.rect.width) * ratio).round() as u16,
        SplitDir::Vertical => (f32::from(split.rect.height) * ratio).round() as u16,
    };
    let boundary_twice = match dir {
        SplitDir::Horizontal => u32::from(split.rect.x.saturating_add(first_span)) * 2,
        SplitDir::Vertical => u32::from(split.rect.y.saturating_add(first_span)) * 2,
    };
    let mut first_panes = Vec::new();
    let mut second_panes = Vec::new();
    for pane in panes {
        let center_twice = match dir {
            SplitDir::Horizontal => u32::from(pane.rect.x) * 2 + u32::from(pane.rect.width),
            SplitDir::Vertical => u32::from(pane.rect.y) * 2 + u32::from(pane.rect.height),
        };
        if center_twice < boundary_twice {
            first_panes.push(*pane);
        } else {
            second_panes.push(*pane);
        }
    }
    if first_panes.is_empty() || second_panes.is_empty() {
        return None;
    }

    let mut first_path = path.to_vec();
    first_path.push(false);
    let mut second_path = path.to_vec();
    second_path.push(true);
    Some(LayoutNode::Split {
        dir,
        ratio: (ratio * 1000.0).round() as u16,
        first: Box::new(build_layout_subtree(&first_path, &first_panes, splits)?),
        second: Box::new(build_layout_subtree(&second_path, &second_panes, splits)?),
    })
}

/// 兼容旧录制快照：没有 splits 时保留原先的确定性水平兜底。
fn legacy_horizontal_tree(leaves: &[PaneId]) -> LayoutNode {
    let mut tree = LayoutNode::leaf(leaves[0]);
    for pane in &leaves[1..] {
        tree.split_at(leaves[0], *pane, SplitDir::Horizontal);
    }
    tree
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
                    .pane_send_text(herdr_pane, &text)
                    .map_err(|e| anyhow!("pane.send_text 失败: {e}"))?;
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
            Task::ResizePane { target, cols, rows } => {
                if !self.panes.iter().any(|pane| pane.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                let cols = (*cols).max(2);
                let rows = (*rows).max(1);
                let unchanged = self
                    .panes
                    .iter()
                    .find(|pane| pane.id == *target)
                    .is_some_and(|pane| pane.cols == cols && pane.rows == rows);
                if unchanged {
                    return Ok(TaskOutcome::Done);
                }
                self.replace_observe_stream(*target, cols, rows)
                    .map_err(|err| anyhow!("Herdr pane observer resize 失败: {err}"))?;
                if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == *target) {
                    pane.cols = cols;
                    pane.rows = rows;
                }
                self.events.push_back(StateChange::PaneResized {
                    pane: *target,
                    cols,
                    rows,
                });
                Ok(TaskOutcome::Done)
            }
            Task::ResizeClient { .. } => Ok(TaskOutcome::Rejected {
                reason: "HerdrRuntime 使用 pane Surface resize".into(),
            }),
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
                let Some(tab) = self
                    .panes
                    .iter()
                    .find(|pane| pane.id == target)
                    .map(|pane| pane.tab)
                else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                };
                let Some(herdr_pane) = self.herdr_pane(target).map(ToOwned::to_owned) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 缺 Herdr id"),
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
                let authoritative_layout = result
                    .get("pane")
                    .and_then(|pane| pane.get("pane_id"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|pane_id| match self.session.pane_layout(pane_id) {
                        Ok(layout) => Some(layout),
                        Err(err) => {
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane_id,
                                error = %err,
                                "pane.split 后读取权威 layout 失败"
                            );
                            None
                        }
                    });
                self.apply_split_pane(&result, tab, *dir, authoritative_layout.as_ref());
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
        self.observe_tx = None;
        self.observe_rx = None;
        self.stop_forward();
        self.status = BackendStatus::Disconnected;
        self.events.push_back(StateChange::BackendStatusChanged(
            BackendStatus::Disconnected,
        ));
        Ok(())
    }

    fn list_worktrees(&self) -> Result<Vec<crate::core::model::backend::WorktreeInfo>> {
        self.worktrees()
    }

    fn create_worktree_spec(
        &self,
        spec: &crate::core::model::backend::WorktreeCreateSpec,
    ) -> Result<crate::core::workspace::spec::WorkspaceSpec> {
        self.create_worktree(spec)
    }

    fn open_worktree_spec(
        &self,
        path: &str,
    ) -> Result<crate::core::workspace::spec::WorkspaceSpec> {
        self.open_worktree(path)
    }
}

impl Drop for HerdrRuntime {
    fn drop(&mut self) {
        self.observe_streams.clear();
        self.observe_tx = None;
        self.observe_rx = None;
        self.stop_forward();
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
    fn apply_split_pane(
        &mut self,
        result: &serde_json::Value,
        tab: TabId,
        dir: SplitDir,
        layout: Option<&LayoutRecord>,
    ) {
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
        self.herdr_pane_to_pane.insert(pane_id.to_string(), pid);
        self.pane_to_herdr_pane.insert(pid, pane_id.to_string());
        let rect = layout.and_then(|layout| {
            layout
                .panes
                .iter()
                .find(|candidate| candidate.pane_id == pane_id)
                .map(|candidate| candidate.rect)
        });
        self.panes.push(PaneInfo {
            id: pid,
            tab,
            active: true,
            title: "herdr".into(),
            cols: rect.map(|rect| rect.width).unwrap_or(80),
            rows: rect.map(|rect| rect.height).unwrap_or(24),
        });
        self.events
            .push_back(StateChange::PaneAdded { pane: pid, tab });

        let applied_authoritative = layout
            .map(|layout| self.apply_layout_record(layout, true))
            .unwrap_or(false);
        if !applied_authoritative {
            if let Some(product_layout) = self.layouts.get_mut(&tab) {
                let base = product_layout.active;
                product_layout.tree.split_at(base, pid, dir);
                product_layout.active = pid;
            }
            if let Some(product_layout) = self.layouts.get(&tab) {
                self.events.push_back(StateChange::LayoutChanged {
                    tab,
                    layout: product_layout.clone(),
                });
            }
        }

        let active = self
            .layouts
            .get(&tab)
            .map(|layout| layout.active)
            .unwrap_or(pid);
        for pane in self.panes.iter_mut().filter(|pane| pane.tab == tab) {
            pane.active = pane.id == active;
        }
        self.active_pane = Some(active);
        self.events
            .push_back(StateChange::ActivePaneChanged { tab, pane: active });

        if self.observe_tx.is_some() {
            let (cols, rows) = self
                .panes
                .iter()
                .find(|pane| pane.id == pid)
                .map(|pane| (pane.cols, pane.rows))
                .unwrap_or((80, 24));
            if let Err(err) = self.replace_observe_stream(pid, cols, rows) {
                tracing::warn!(
                    target = "muxterm::herdr",
                    pane = %pane_id,
                    error = %err,
                    "新 split pane 的 observe 流启动失败"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::herdr::session::{LayoutPaneRecord, LayoutSplitRecord};

    /// Herdr public ids 使用 bijective base-32，不是十进制。用户真实 session
    /// 已出现 pP/pQ/pR；它们绝不能全部退化成 PaneId(0)。
    #[test]
    fn herdr_public_id_suffix_decodes_alphanumeric_ids() {
        assert_eq!(numeric_suffix("w2:p1"), 1);
        assert_eq!(numeric_suffix("w2:p9"), 9);
        assert_eq!(numeric_suffix("w2:pA"), 10);
        assert_eq!(numeric_suffix("w2:pP"), 22);
        assert_eq!(numeric_suffix("w2:pQ"), 23);
        assert_eq!(numeric_suffix("w2:pR"), 24);
        assert_eq!(numeric_suffix("w2:p0"), 32);
        assert_eq!(numeric_suffix("w2:p11"), 33);
    }

    #[test]
    fn herdr_layout_paths_preserve_nested_directions_and_ratios() {
        let rect = |x, y, width, height| LayoutRect {
            x,
            y,
            width,
            height,
        };
        let layout = LayoutRecord {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            zoomed: false,
            area: rect(0, 0, 100, 40),
            focused_pane_id: "w1:p2".into(),
            panes: vec![
                LayoutPaneRecord {
                    pane_id: "w1:p1".into(),
                    focused: false,
                    rect: rect(0, 0, 30, 10),
                },
                LayoutPaneRecord {
                    pane_id: "w1:p2".into(),
                    focused: true,
                    rect: rect(0, 10, 30, 30),
                },
                LayoutPaneRecord {
                    pane_id: "w1:p3".into(),
                    focused: false,
                    rect: rect(30, 0, 70, 40),
                },
            ],
            splits: vec![
                LayoutSplitRecord {
                    id: "split_0_root".into(),
                    path: vec![],
                    direction: LayoutSplitDirection::Right,
                    ratio: 0.3,
                    rect: rect(0, 0, 100, 40),
                },
                LayoutSplitRecord {
                    id: "split_1_0".into(),
                    path: vec![false],
                    direction: LayoutSplitDirection::Down,
                    ratio: 0.25,
                    rect: rect(0, 0, 30, 40),
                },
            ],
        };
        let pane_ids = HashMap::from([
            ("w1:p1".into(), PaneId(1)),
            ("w1:p2".into(), PaneId(2)),
            ("w1:p3".into(), PaneId(3)),
        ]);

        assert_eq!(
            layout_tree_from_record(&layout, &pane_ids),
            Some(LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 300,
                first: Box::new(LayoutNode::Split {
                    dir: SplitDir::Vertical,
                    ratio: 250,
                    first: Box::new(LayoutNode::Leaf(PaneId(1))),
                    second: Box::new(LayoutNode::Leaf(PaneId(2))),
                }),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            })
        );
    }
}
