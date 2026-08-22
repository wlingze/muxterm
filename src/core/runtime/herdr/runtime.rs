//! HerdrRuntime：绑定一个 Herdr workspace 的 Runtime 视图。
//!
//! 一个 `HerdrSession`（Arc）可被多个 `HerdrRuntime` 共享；每个 Runtime
//! 只填一个 Muxterm Workspace。直播字节走 observe 流（client socket），
//! attach 快照用 `pane.read`；原始键盘字节走 `pane.send_text`，语义按键走
//! `pane.send_keys`。逐键输入禁止走会自动包 bracketed-paste 的 `pane.send_input`。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use crate::core::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES};
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{
    BackendStatus, PaneAgentInfo, PaneAgentSession, PaneAgentSessionKind, PaneAgentStatus,
    PaneInfo, State, StateChange, TabInfo,
};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::input::KeyEvent;
use crate::core::types::{PaneId, TabId};

use super::events::{EventStream, EventStreamEvent};
use super::observe::{ObserveStream, PaneStreamEvent, StreamMode, StreamStartResult};
use super::registry::{
    classify_stream_end, ControlRearm, FrameDecision, PaneStreamSlot, SlotAction, SlotState,
    SurfaceBaseline, FULL_FRAME_DEADLINE, INPUT_MAX_BYTES, INPUT_MAX_WRITES,
};
use super::session::{
    AgentRecord, HerdrAgentStatus, HerdrSession, LayoutRecord, LayoutRect, LayoutSplitDirection,
    SessionSnapshot,
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

/// Herdr headless/background snapshots use zero-sized rects until a client
/// provides a viewport. Keep that wire sentinel inside this adapter.
const DEFAULT_HERDR_COLS: u16 = 80;
const DEFAULT_HERDR_ROWS: u16 = 24;

/// 绑定一个 Herdr workspace 的 Runtime。
pub struct HerdrRuntime {
    session: Arc<HerdrSession>,
    workspace_id: String,
    workspace_name: String,
    /// Pool 前台/后台状态（`set_foreground` 驱动；决定 active pane 的
    /// desired mode 是否可以是 Control）。
    foreground: bool,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    layouts: HashMap<TabId, TabLayout>,
    outputs: HashMap<PaneId, Vec<u8>>,
    agents: HashMap<PaneId, PaneAgentInfo>,
    status: BackendStatus,
    active_tab: Option<TabId>,
    active_pane: Option<PaneId>,
    events: VecDeque<StateChange>,
    herdr_tab_to_tab: HashMap<String, TabId>,
    herdr_pane_to_pane: HashMap<String, PaneId>,
    tab_to_herdr_tab: HashMap<TabId, String>,
    pane_to_herdr_pane: HashMap<PaneId, String>,
    /// pane-keyed stream registry（W2：唯一所有权；取代 Vec<ObserveStream>）。
    stream_slots: HashMap<PaneId, PaneStreamSlot>,
    stream_rx: Option<mpsc::Receiver<PaneStreamEvent>>,
    stream_tx: Option<mpsc::Sender<PaneStreamEvent>>,
    /// start worker 完成结果（generation-tagged）。
    start_rx: Option<mpsc::Receiver<StreamStartResult>>,
    start_tx: Option<mpsc::Sender<StreamStartResult>>,
    event_rx: Option<mpsc::Receiver<EventStreamEvent>>,
    event_tx: Option<mpsc::Sender<EventStreamEvent>>,
    event_stream: Option<EventStream>,
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
            // 独立构造（CLI/测试直连）的 Runtime 就是自己的前台；Pool 打开后会
            // 立即 set_foreground(true)，后台切换再降 false。默认 true 保证
            // 直连场景的 active pane 持有 Control。
            foreground: true,
            tabs: vec![],
            panes: vec![],
            layouts: HashMap::new(),
            outputs: HashMap::new(),
            agents: HashMap::new(),
            status: BackendStatus::Disconnected,
            active_tab: None,
            active_pane: None,
            events: VecDeque::new(),
            herdr_tab_to_tab: HashMap::new(),
            herdr_pane_to_pane: HashMap::new(),
            tab_to_herdr_tab: HashMap::new(),
            pane_to_herdr_pane: HashMap::new(),
            stream_slots: HashMap::new(),
            stream_rx: None,
            stream_tx: None,
            start_rx: None,
            start_tx: None,
            event_rx: None,
            event_tx: None,
            event_stream: None,
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

    /// 测试/诊断：产品 TabId 对应的 Herdr public tab id。
    pub fn test_herdr_tab_id(&self, tab: TabId) -> Option<&str> {
        self.tab_to_herdr_tab.get(&tab).map(String::as_str)
    }

    /// 测试/诊断：产品 PaneId 对应的 Herdr public pane id。
    pub fn test_herdr_pane_id(&self, pane: PaneId) -> Option<&str> {
        self.pane_to_herdr_pane.get(&pane).map(String::as_str)
    }

    /// 测试/诊断：某 pane 的 stream 启动次数（按 transition 记录统计）。
    pub fn test_stream_starts(&self, pane: PaneId) -> u64 {
        self.stream_slots
            .get(&pane)
            .map(|slot| {
                slot.transitions
                    .iter()
                    .filter(|t| t.starts_with("start:"))
                    .count() as u64
            })
            .unwrap_or(0)
    }

    /// 测试/诊断：某 pane 的 control（takeover=true）启动次数。
    pub fn test_control_takeover_starts(&self, pane: PaneId) -> u64 {
        self.stream_slots
            .get(&pane)
            .map(|slot| {
                slot.transitions
                    .iter()
                    .filter(|t| t.starts_with("start:") && t.contains("takeover=true"))
                    .count() as u64
            })
            .unwrap_or(0)
    }

    /// 测试/诊断：某 pane 当前是否处于 takeover suppression。
    pub fn test_takeover_suppressed(&self, pane: PaneId) -> bool {
        self.stream_slots
            .get(&pane)
            .map(|slot| slot.control_rearm == ControlRearm::SuppressedAfterTakeover)
            .unwrap_or(false)
    }

    /// 测试/诊断：某 pane 当前实际 stream 模式。
    pub fn test_actual_mode(&self, pane: PaneId) -> Option<StreamMode> {
        self.stream_slots.get(&pane).and_then(|s| s.actual_mode)
    }

    /// 测试/诊断：某 pane 当前 slot 状态。
    pub fn test_slot_state(&self, pane: PaneId) -> Option<SlotState> {
        self.stream_slots.get(&pane).map(|s| s.state)
    }

    /// 把 snapshot 里属于本 workspace 的 tab/pane/layout 填进产品状态。
    fn apply_snapshot(&mut self, snap: &SessionSnapshot, initial: bool) -> bool {
        let Some(ws) = snap
            .workspaces
            .iter()
            .find(|w| w.workspace_id == self.workspace_id)
        else {
            return false;
        };
        self.workspace_name = ws.label.clone();
        let previous_pane_sizes = self
            .panes
            .iter()
            .filter_map(|pane| {
                self.pane_to_herdr_pane
                    .get(&pane.id)
                    .map(|herdr_pane| (herdr_pane.clone(), (pane.cols, pane.rows)))
            })
            .collect::<HashMap<_, _>>();
        let previous_layouts = std::mem::take(&mut self.layouts);
        // `focused_*` at snapshot root belongs to the globally focused Herdr
        // workspace. A Muxterm Runtime may bind a background workspace, so its
        // active tab must come from that workspace record instead.
        let active_herdr_tab = ws.active_tab_id.clone();

        self.tabs.clear();
        self.panes.clear();
        self.agents.clear();
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
            let previous = previous_pane_sizes.get(&pane.pane_id).copied();
            let (cols, rows) = pane_rect
                .map(|rect| normalize_pane_size(rect.width, rect.height, previous))
                .unwrap_or_else(|| normalize_pane_size(0, 0, previous));
            self.panes.push(PaneInfo {
                id,
                tab,
                active: false,
                title: pane
                    .label
                    .as_ref()
                    .or(pane.title.as_ref())
                    .or(pane.terminal_title_stripped.as_ref())
                    .or(pane.terminal_title.as_ref())
                    .cloned()
                    .or_else(|| {
                        pane.cwd.as_deref().map(|c| {
                            c.rsplit('/')
                                .next()
                                .filter(|s| !s.is_empty())
                                .unwrap_or("herdr")
                                .to_string()
                        })
                    })
                    .unwrap_or_else(|| "herdr".into()),
                cols,
                rows,
            });
        }

        let current_tabs = self.tabs.iter().map(|tab| tab.id).collect::<HashSet<_>>();
        self.layouts = previous_layouts
            .into_iter()
            .filter(|(tab, _)| current_tabs.contains(tab))
            .collect();

        for agent in snap
            .agents
            .iter()
            .filter(|agent| agent.workspace_id == self.workspace_id)
        {
            if let Some(pane) = self.herdr_pane_to_pane.get(&agent.pane_id).copied() {
                self.agents.insert(pane, product_agent(agent));
            }
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

        // active 标记。每个 layout 自带该 tab 的 focused pane；只取绑定
        // workspace 的 active tab 对应值，不能误用其它 workspace 的全局焦点。
        self.active_tab = active_herdr_tab
            .as_deref()
            .and_then(|tab| self.herdr_tab_to_tab.get(tab).copied())
            .or_else(|| {
                self.tabs
                    .iter()
                    .find(|tab| {
                        snap.tabs.iter().any(|source| {
                            source.workspace_id == self.workspace_id
                                && source.tab_id == self.tab_to_herdr_tab[&tab.id]
                                && source.focused
                        })
                    })
                    .map(|tab| tab.id)
            })
            .or_else(|| self.tabs.first().map(|tab| tab.id));
        for tab in &mut self.tabs {
            tab.active = Some(tab.id) == self.active_tab;
        }

        self.active_pane = self
            .active_tab
            .and_then(|tab| self.layouts.get(&tab).map(|layout| layout.active))
            .or_else(|| {
                let active_tab = self.active_tab?;
                self.panes
                    .iter()
                    .find(|pane| pane.tab == active_tab)
                    .map(|pane| pane.id)
            });
        for pane in &mut self.panes {
            pane.active = Some(pane.id) == self.active_pane;
        }

        if initial {
            // 事件：拓扑 + 激活 + agent bootstrap。
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
            for (pane, agent) in &self.agents {
                self.events.push_back(StateChange::PaneAgentChanged {
                    pane: *pane,
                    agent: Some(Box::new(agent.clone())),
                    initial: true,
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
        true
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
                let (cols, rows) = normalize_pane_size(
                    source_pane.rect.width,
                    source_pane.rect.height,
                    Some((info.cols, info.rows)),
                );
                info.cols = cols;
                info.rows = rows;
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

        // Protocol 19 reports zero-area layouts for background workspaces that
        // have no foreground viewport. Those records contain split metadata but
        // no pane placement, so rebuilding would incorrectly hit the horizontal
        // legacy fallback. Preserve the last authoritative tree and only accept
        // the focused pane when it is still one of that tree's leaves.
        if !layout_has_usable_geometry(layout) {
            if let Some(existing) = self.layouts.get(&tab).cloned() {
                let existing_leaves = existing.tree.leaves().into_iter().collect::<HashSet<_>>();
                let incoming_leaves = leaves.iter().copied().collect::<HashSet<_>>();
                if existing_leaves == incoming_leaves {
                    let active = self
                        .herdr_pane_to_pane
                        .get(&layout.focused_pane_id)
                        .copied()
                        .filter(|pane| existing.tree.contains(*pane))
                        .unwrap_or(existing.active);
                    let product_layout = TabLayout { active, ..existing };
                    self.layouts.insert(tab, product_layout.clone());
                    if emit_event {
                        self.events.push_back(StateChange::LayoutChanged {
                            tab,
                            layout: product_layout,
                        });
                    }
                    return true;
                }
            }
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
            self.seed_one_pane(pane, &herdr_pane);
        }
    }

    fn seed_one_pane(&mut self, pane: PaneId, herdr_pane: &str) {
        match self.session.pane_read_ansi(herdr_pane) {
            Ok(bytes) if !bytes.is_empty() => {
                self.outputs.insert(pane, bytes.clone());
                // pane.read 只进 Index（搜索/attention）；Surface 由
                // current-generation full frame 负责，禁止把无头快照当像素。
                self.events
                    .push_back(StateChange::PaneIndexSnapshot { pane, data: bytes });
                // attach 种子就位：该 generation 的首个 full 不得覆盖它。
                if let Some(slot) = self.stream_slots.get_mut(&pane) {
                    slot.seed_pending = true;
                }
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

    /// Subscribe to the complete global event set plus one scoped agent-status
    /// subscription per pane. The reader performs snapshot refreshes off the UI
    /// thread and sends only normalized data back to Runtime.
    fn start_event_stream(&mut self) -> Result<()> {
        if self.event_tx.is_none() {
            let (tx, rx) = mpsc::channel::<EventStreamEvent>();
            self.event_tx = Some(tx);
            self.event_rx = Some(rx);
        }
        let tx = self
            .event_tx
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("Herdr event channel 未启动"))?;
        let pane_ids = self
            .panes
            .iter()
            .filter_map(|pane| self.pane_to_herdr_pane.get(&pane.id).cloned())
            .collect::<Vec<_>>();
        self.event_stream = Some(EventStream::start(
            Arc::clone(&self.session),
            &self.workspace_id,
            &pane_ids,
            tx,
        )?);
        Ok(())
    }

    fn restart_event_stream(&mut self) {
        self.event_stream = None;
        if let Err(err) = self.start_event_stream() {
            tracing::warn!(
                target = "muxterm::herdr",
                workspace = %self.workspace_id,
                error = %err,
                "重建 Herdr event subscription 失败"
            );
        }
    }

    /// 初始化 stream channels（幂等）。
    fn ensure_stream_channels(&mut self) {
        if self.stream_tx.is_none() {
            let (tx, rx) = mpsc::channel::<PaneStreamEvent>();
            self.stream_tx = Some(tx);
            self.stream_rx = Some(rx);
        }
        if self.start_tx.is_none() {
            let (tx, rx) = mpsc::channel::<StreamStartResult>();
            self.start_tx = Some(tx);
            self.start_rx = Some(rx);
        }
    }

    /// 期望模式：Pool 前台 active pane = Control；其余 pane/后台 workspace = Observe。
    fn desired_mode_for(&self, pane: &PaneInfo) -> StreamMode {
        if self.foreground && Some(pane.id) == self.active_pane {
            StreamMode::Control
        } else {
            StreamMode::Observe
        }
    }

    /// 为每个 pane 建 registry slot 并按 foreground/active 计算 desired mode。
    fn bootstrap_stream_slots(&mut self) {
        self.ensure_stream_channels();
        for pane in &self.panes {
            let Some(herdr_pane) = self.pane_to_herdr_pane.get(&pane.id).cloned() else {
                continue;
            };
            let mode = self.desired_mode_for(pane);
            self.stream_slots
                .entry(pane.id)
                .or_insert_with(|| PaneStreamSlot::new(pane.id, herdr_pane, mode))
                .desired_mode = mode;
        }
        self.reconcile_stream_modes();
    }

    /// 唯一 mode transition 入口：先算全体 desired mode，再做最小变更。
    ///
    /// - 已关闭 pane：slot 直接 Stopped（旧事件不能再启动流）；
    /// - 新 pane：建 slot（`ensure_pane_initialized` 负责 seed/初始化）；
    /// - desired 与 actual 不同 → 先递增 generation、关旧流，再启动新流；
    /// - `SuppressedAfterTakeover` 的 pane effective 保持 Observe，不反抢；
    /// - `Degraded` 只由新用户 intent 重新武装（new_user_intent 会解除）。
    fn reconcile_stream_modes(&mut self) {
        let in_topology: HashSet<PaneId> = self.panes.iter().map(|p| p.id).collect();
        self.stream_slots.retain(|pane, slot| {
            if !in_topology.contains(pane) {
                slot.state = SlotState::Stopped;
                slot.stream = None;
                slot.actual_mode = None;
                slot.drop_pending_input("pane-closed");
                false
            } else {
                true
            }
        });
        for pane in &self.panes {
            let Some(herdr_pane) = self.pane_to_herdr_pane.get(&pane.id).cloned() else {
                continue;
            };
            let mode = self.desired_mode_for(pane);
            let slot = self
                .stream_slots
                .entry(pane.id)
                .or_insert_with(|| PaneStreamSlot::new(pane.id, herdr_pane, mode));
            slot.desired_mode = mode;
        }

        let transitions: Vec<(PaneId, StreamMode, bool)> = self
            .panes
            .iter()
            .filter_map(|pane| {
                let slot = self.stream_slots.get(&pane.id)?;
                if slot.state == SlotState::Stopped
                    || slot.state == SlotState::Degraded
                    || slot.has_inflight_start()
                {
                    return None;
                }
                let effective = if slot.control_rearm == ControlRearm::SuppressedAfterTakeover {
                    StreamMode::Observe
                } else {
                    slot.desired_mode
                };
                if slot.actual_mode == Some(effective) && slot.state == SlotState::Live {
                    return None;
                }
                let takeover = effective == StreamMode::Control && slot.may_takeover();
                Some((pane.id, effective, takeover))
            })
            .collect();
        for (pane, mode, takeover) in transitions {
            self.start_stream_replacing(pane, mode, takeover);
        }
    }

    /// 递增 generation → 关旧流（Drop 发 Detach）→ 启动新流（async worker；
    /// 调用线程只登记 Starting）。generation 递增必须先于旧流 shutdown。
    fn start_stream_replacing(&mut self, pane: PaneId, mode: StreamMode, takeover: bool) {
        let Some(slot) = self.stream_slots.get_mut(&pane) else {
            return;
        };
        slot.generation = slot.generation.saturating_add(1);
        slot.stream = None;
        slot.actual_mode = None;
        slot.state = SlotState::Starting;
        slot.started_at = Some(Instant::now());
        slot.last_event_ordinal = 0;
        slot.last_frame_seq = None;
        slot.surface_baseline = SurfaceBaseline::AwaitingFull;
        slot.live_since = None;
        slot.pre_full.clear();
        slot.pre_full_bytes = 0;
        if mode.is_control() && takeover {
            slot.takeover_attempted = true;
        }
        let generation = slot.generation;
        let target = slot.target.clone();
        let (cols, rows) = self
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| (p.cols, p.rows))
            .unwrap_or((80, 24));
        let socket = self.session.client_socket_path().to_path_buf();
        let (Some(event_tx), Some(start_tx)) = (
            self.stream_tx.as_ref().cloned(),
            self.start_tx.as_ref().cloned(),
        ) else {
            return;
        };
        tracing::info!(
            target = "muxterm::herdr",
            workspace = %self.workspace_id,
            pane = %pane,
            herdr_pane = %target,
            generation = generation,
            mode = ?mode,
            takeover = takeover,
            "start pane stream"
        );
        if let Some(slot) = self.stream_slots.get_mut(&pane) {
            slot.transitions.push(format!(
                "start:mode={mode:?}:takeover={takeover}:gen={generation}"
            ));
        }
        ObserveStream::start_async(
            socket, target, pane, generation, mode, takeover, cols, rows, event_tx, start_tx,
        );
    }

    /// 把产品焦点切到某 pane（本地 focus edge：新 control intent，可 takeover）。
    fn promote_focus_to(&mut self, pane: PaneId) {
        if let Some(slot) = self.stream_slots.get_mut(&pane) {
            slot.new_user_intent(true);
            if slot.state == SlotState::Degraded {
                slot.state = SlotState::Absent;
                slot.retry_count = 0;
                slot.retry_at = None;
                slot.transitions.push("rearm:focus".into());
            }
        }
        self.reconcile_stream_modes();
    }

    /// 向 pane 写原始输入：当前 control Live 直接发；Starting/Backoff 期间按
    /// intent-bound 队列暂存（256 write/64 KiB）；无 control/被 suppression 时
    /// 建立新 intent（真实 input）并 promote 一次。
    fn send_control_input(&mut self, pane: PaneId, data: &[u8]) -> Result<()> {
        // 向非 active pane 写输入前，产品焦点必须先切到该 pane。
        if self.active_pane != Some(pane) {
            self.execute(&Task::SwitchPane { target: pane })?;
        }
        let needs_reconcile = {
            let Some(slot) = self.stream_slots.get_mut(&pane) else {
                return Err(anyhow!("pane {pane} 不存在"));
            };
            let queued = match (slot.state, slot.actual_mode, slot.control_rearm) {
                (SlotState::Live, Some(StreamMode::Control), _) => {
                    let stream = slot
                        .stream
                        .as_mut()
                        .ok_or_else(|| anyhow!("pane {pane} control stream 缺失"))?;
                    stream.send_input(data)?;
                    return Ok(());
                }
                (SlotState::Starting | SlotState::Backoff, _, ControlRearm::Armed) => false,
                _ => {
                    // suppressed 或没有 control：真实 input 建立新 intent。
                    slot.new_user_intent(false);
                    if slot.state == SlotState::Degraded {
                        slot.state = SlotState::Absent;
                        slot.retry_count = 0;
                        slot.retry_at = None;
                        slot.transitions.push("rearm:input".into());
                    }
                    true
                }
            };
            if slot.queue_input(data.to_vec()).is_err() {
                return Err(anyhow!(
                    "pane {pane} input 队列溢出（{INPUT_MAX_WRITES} write/{INPUT_MAX_BYTES} B）"
                ));
            }
            queued
        };
        if needs_reconcile {
            self.reconcile_stream_modes();
        }
        Ok(())
    }

    /// resize 只发给 current control；Starting/Backoff 期间 latest-wins 暂存。
    fn resize_control_stream(&mut self, pane: PaneId, cols: u16, rows: u16) -> Result<()> {
        let (cols, rows) = normalize_pane_size(cols, rows, None);
        let Some(slot) = self.stream_slots.get_mut(&pane) else {
            return Err(anyhow!("pane {pane} 不存在"));
        };
        match (slot.state, slot.actual_mode) {
            (SlotState::Live, Some(StreamMode::Control)) => {
                let stream = slot
                    .stream
                    .as_mut()
                    .ok_or_else(|| anyhow!("pane {pane} control stream 缺失"))?;
                stream.resize(cols, rows)
            }
            (SlotState::Starting | SlotState::Backoff, _) => {
                slot.pending_resize = Some((cols, rows));
                Ok(())
            }
            _ => {
                let needs_reconcile = self.active_pane == Some(pane);
                slot.pending_resize = Some((cols, rows));
                if needs_reconcile {
                    // active pane 需要 control 才能收 resize；无 intent 时
                    // open/activate 语义（takeover=false）启动即可。
                    // 先结束 slot 借用再 reconcile。
                    return self.resize_pending_then_reconcile(pane, cols, rows);
                }
                Ok(())
            }
        }
    }

    /// 记录 pending resize 后触发 reconcile（避免 &mut 借用与 &mut self 冲突）。
    fn resize_pending_then_reconcile(&mut self, pane: PaneId, cols: u16, rows: u16) -> Result<()> {
        if let Some(slot) = self.stream_slots.get_mut(&pane) {
            slot.pending_resize = Some((cols, rows));
        }
        self.reconcile_stream_modes();
        Ok(())
    }

    /// 取 stream 事件：generation/ordinal/seq 过滤 + 退避 + takeover suppression。
    fn drain_stream(&mut self) {
        let now = Instant::now();
        let mut pending_actions: Vec<(PaneId, SlotAction, String)> = Vec::new();
        let Some(rx) = &self.stream_rx else {
            return;
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                PaneStreamEvent::Frame {
                    pane,
                    generation,
                    event_ordinal,
                    wire_seq,
                    bytes,
                    width,
                    height,
                    full,
                } => {
                    let Some(slot) = self.stream_slots.get_mut(&pane) else {
                        continue;
                    };
                    if !slot.is_current(generation) || !slot.accept_ordinal(event_ordinal) {
                        continue;
                    }
                    let decision = slot.decide_frame(wire_seq, full);
                    match decision {
                        FrameDecision::DropDuplicate => continue,
                        FrameDecision::GapFailure => {
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                wire_seq = wire_seq,
                                last_seq = ?slot.last_frame_seq,
                                "diff wire seq gap：generation 有界失败"
                            );
                            slot.state = SlotState::Backoff;
                            slot.stream = None;
                            slot.actual_mode = None;
                            pending_actions.push((pane, SlotAction::Retry, "wire-seq gap".into()));
                            continue;
                        }
                        FrameDecision::GapFullBaseline => {
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                wire_seq = wire_seq,
                                "full frame gap：重建 baseline"
                            );
                        }
                        FrameDecision::Apply => {}
                    }
                    if let Some(p) = self.panes.iter_mut().find(|p| p.id == pane) {
                        (p.cols, p.rows) =
                            normalize_pane_size(width, height, Some((p.cols, p.rows)));
                    }
                    if full {
                        // 只有 attach 种子（seed_pending）存在时，本 generation
                        // 首个 full（新 client 终端初始化）才保留旧 Index；否则
                        // full 直接替换当前帧（含 generation 切换后的新 full）。
                        let keep_seed = slot.seed_pending
                            && slot.surface_baseline == SurfaceBaseline::AwaitingFull;
                        slot.seed_pending = false;
                        slot.surface_baseline = SurfaceBaseline::Ready;
                        slot.live_since = Some(now);
                        if keep_seed {
                            self.outputs.entry(pane).or_insert_with(|| bytes.clone());
                            // 追赶 full 之前缓存的严格连续增量。
                            match slot.take_catchup_after_full(wire_seq) {
                                Ok(catchup) => {
                                    for data in catchup {
                                        append_capped(
                                            self.outputs.entry(pane).or_default(),
                                            &data,
                                            MAX_PANE_OUTPUT_BYTES,
                                        );
                                        self.events
                                            .push_back(StateChange::PaneOutput { pane, data });
                                    }
                                }
                                Err(_) => {
                                    slot.state = SlotState::Backoff;
                                    slot.stream = None;
                                    slot.actual_mode = None;
                                    pending_actions.push((
                                        pane,
                                        SlotAction::Retry,
                                        "pre-full catchup gap".into(),
                                    ));
                                    continue;
                                }
                            }
                        } else {
                            self.outputs.insert(pane, bytes.clone());
                        }
                        self.events
                            .push_back(StateChange::PaneFrame { pane, data: bytes });
                    } else if slot.surface_baseline == SurfaceBaseline::AwaitingFull {
                        // full 前 diff：不画进 Surface，只进有界队列。
                        if slot.queue_pre_full(wire_seq, bytes).is_err() {
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                "pre-full 队列溢出：generation 有界失败"
                            );
                            slot.state = SlotState::Backoff;
                            slot.stream = None;
                            slot.actual_mode = None;
                            pending_actions.push((
                                pane,
                                SlotAction::Retry,
                                "pre-full overflow".into(),
                            ));
                        }
                    } else {
                        append_capped(
                            self.outputs.entry(pane).or_default(),
                            &bytes,
                            MAX_PANE_OUTPUT_BYTES,
                        );
                        self.events
                            .push_back(StateChange::PaneOutput { pane, data: bytes });
                    }
                }
                PaneStreamEvent::Closed {
                    pane,
                    generation,
                    event_ordinal,
                    reason,
                } => {
                    let Some(slot) = self.stream_slots.get_mut(&pane) else {
                        continue;
                    };
                    if !slot.is_current(generation) || !slot.accept_ordinal(event_ordinal) {
                        continue;
                    }
                    let reason_str = reason.clone().unwrap_or_default();
                    let is_takeover = PaneStreamSlot::is_takeover(&reason_str);
                    let action = classify_stream_end(slot, is_takeover, false);
                    slot.stream = None;
                    slot.actual_mode = None;
                    match action {
                        SlotAction::TakenOver => {
                            slot.control_rearm = ControlRearm::SuppressedAfterTakeover;
                            slot.drop_pending_input("takeover");
                            slot.transitions.push("taken-over:suppress".into());
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                reason = %reason_str,
                                "control stream taken over：suppression + 降 Observe"
                            );
                            pending_actions.push((pane, SlotAction::TakenOver, reason_str));
                        }
                        SlotAction::Retry => {
                            slot.state = SlotState::Backoff;
                            pending_actions.push((pane, SlotAction::Retry, reason_str));
                        }
                        SlotAction::Degrade => {
                            slot.state = SlotState::Degraded;
                            slot.drop_pending_input("degraded");
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                reason = %reason_str,
                                "stream 第五次重试失败：Degraded"
                            );
                        }
                        SlotAction::Stop => {
                            slot.state = SlotState::Absent;
                        }
                    }
                }
                PaneStreamEvent::Error {
                    pane,
                    generation,
                    event_ordinal,
                    message,
                } => {
                    let Some(slot) = self.stream_slots.get_mut(&pane) else {
                        continue;
                    };
                    if !slot.is_current(generation) || !slot.accept_ordinal(event_ordinal) {
                        continue;
                    }
                    let is_takeover = PaneStreamSlot::is_takeover(&message);
                    let action = classify_stream_end(slot, is_takeover, false);
                    slot.stream = None;
                    slot.actual_mode = None;
                    match action {
                        SlotAction::TakenOver => {
                            slot.control_rearm = ControlRearm::SuppressedAfterTakeover;
                            slot.drop_pending_input("takeover");
                            slot.transitions.push("taken-over:suppress".into());
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                error = %message,
                                "control stream taken over：suppression + 降 Observe"
                            );
                            pending_actions.push((pane, SlotAction::TakenOver, message));
                        }
                        SlotAction::Retry => {
                            slot.state = SlotState::Backoff;
                            pending_actions.push((pane, SlotAction::Retry, message));
                        }
                        SlotAction::Degrade => {
                            slot.state = SlotState::Degraded;
                            slot.drop_pending_input("degraded");
                            tracing::warn!(
                                target = "muxterm::herdr",
                                pane = %pane,
                                generation = generation,
                                error = %message,
                                "stream 第五次重试失败：Degraded"
                            );
                        }
                        SlotAction::Stop => {
                            slot.state = SlotState::Absent;
                        }
                    }
                }
            }
        }
        for (pane, action, reason) in pending_actions {
            let Some(slot) = self.stream_slots.get_mut(&pane) else {
                continue;
            };
            match action {
                SlotAction::Retry => {
                    // 普通故障：有界退避。
                    if slot.schedule_retry(now).is_none() {
                        slot.state = SlotState::Degraded;
                        slot.drop_pending_input("degraded");
                        tracing::warn!(
                            target = "muxterm::herdr",
                            pane = %pane,
                            reason = %reason,
                            "自动 retry 次数耗尽：Degraded"
                        );
                    } else {
                        slot.state = SlotState::Backoff;
                    }
                }
                SlotAction::TakenOver => {
                    // 降 Observe：不反抢；reconcile 会启动 Observe 流。
                    slot.state = SlotState::Absent;
                }
                _ => {}
            }
        }
    }

    /// 处理 start worker 完成结果（generation-tagged）。
    fn drain_start_results(&mut self) {
        let Some(rx) = &self.start_rx else {
            return;
        };
        let results: Vec<StreamStartResult> = rx.try_iter().collect();
        for result in results {
            match result {
                StreamStartResult::Started {
                    pane,
                    generation,
                    stream,
                } => {
                    let Some(slot) = self.stream_slots.get_mut(&pane) else {
                        continue;
                    };
                    if !slot.is_current(generation) || slot.state != SlotState::Starting {
                        continue;
                    }
                    let mode = stream.mode();
                    tracing::info!(
                        target = "muxterm::herdr",
                        workspace = %self.workspace_id,
                        pane = %pane,
                        generation = generation,
                        mode = ?mode,
                        "pane stream started"
                    );
                    slot.stream = Some(stream);
                    slot.actual_mode = Some(mode);
                    slot.state = SlotState::Live;
                    if mode.is_control() {
                        // 握手成功后：先发 coalesced resize，再 flush input 恰好一次。
                        let resize = slot.pending_resize.take();
                        let inputs: Vec<Vec<u8>> = slot.pending_input.drain(..).collect();
                        slot.pending_input_bytes = 0;
                        if let Some((cols, rows)) = resize {
                            if let Some(stream) = slot.stream.as_mut() {
                                if let Err(err) = stream.resize(cols, rows) {
                                    tracing::warn!(
                                        target = "muxterm::herdr",
                                        pane = %pane,
                                        error = %err,
                                        "control handshake 后 resize 失败"
                                    );
                                }
                            }
                        }
                        for data in inputs {
                            if let Some(stream) = slot.stream.as_mut() {
                                if let Err(err) = stream.send_input(&data) {
                                    tracing::warn!(
                                        target = "muxterm::herdr",
                                        pane = %pane,
                                        error = %err,
                                        "control handshake 后 input flush 失败"
                                    );
                                }
                            }
                        }
                    }
                }
                StreamStartResult::Failed {
                    pane,
                    generation,
                    message,
                } => {
                    let Some(slot) = self.stream_slots.get_mut(&pane) else {
                        continue;
                    };
                    if !slot.is_current(generation) || slot.state != SlotState::Starting {
                        continue;
                    }
                    tracing::warn!(
                        target = "muxterm::herdr",
                        pane = %pane,
                        generation = generation,
                        error = %message,
                        "pane stream start 失败"
                    );
                    slot.state = SlotState::Backoff;
                    slot.actual_mode = None;
                    if slot.schedule_retry(Instant::now()).is_none() {
                        slot.state = SlotState::Degraded;
                        slot.drop_pending_input("degraded");
                    }
                }
            }
        }
    }

    /// 每 poll tick：启动到期的自动 retry（含稳定窗口恢复预算）。
    fn maybe_start_pending_retries(&mut self, now: Instant) {
        let due: Vec<PaneId> = self
            .stream_slots
            .iter()
            .filter(|(_, s)| {
                s.state == SlotState::Backoff && s.retry_at.is_some_and(|at| now >= at)
            })
            .map(|(p, _)| *p)
            .collect();
        for pane in due {
            let mode = {
                let slot = self.stream_slots.get_mut(&pane);
                let Some(slot) = slot else {
                    continue;
                };
                if slot.stable_window_elapsed(now) {
                    slot.reset_retry_budget();
                    slot.transitions.push("budget-reset:stable".into());
                }
                if slot.control_rearm == ControlRearm::SuppressedAfterTakeover {
                    StreamMode::Observe
                } else {
                    slot.desired_mode
                }
            };
            let takeover = mode.is_control() && {
                self.stream_slots
                    .get(&pane)
                    .is_some_and(|s| s.may_takeover())
            };
            self.start_stream_replacing(pane, mode, takeover);
        }
    }

    /// 每 poll tick：Starting 且 full 超时 → Degraded（保留旧像素，不 fallback）。
    fn degrade_stalled_streams(&mut self, now: Instant) {
        let stalled: Vec<PaneId> = self
            .stream_slots
            .iter()
            .filter(|(_, s)| {
                s.state == SlotState::Starting
                    && s.started_at
                        .is_some_and(|started| now.duration_since(started) >= FULL_FRAME_DEADLINE)
            })
            .map(|(p, _)| *p)
            .collect();
        for pane in stalled {
            if let Some(slot) = self.stream_slots.get_mut(&pane) {
                slot.state = SlotState::Degraded;
                slot.stream = None;
                slot.actual_mode = None;
                slot.drop_pending_input("full-timeout");
                tracing::warn!(
                    target = "muxterm::herdr",
                    pane = %pane,
                    "首个 full frame 超时：Degraded（保留旧像素）"
                );
            }
        }
    }

    /// 停止全部流（detach/shutdown/drop）。
    fn stop_all_streams(&mut self) {
        for slot in self.stream_slots.values_mut() {
            slot.state = SlotState::Stopped;
            slot.stream = None;
            slot.actual_mode = None;
            slot.drop_pending_input("detach");
        }
        self.stream_tx = None;
        self.stream_rx = None;
        self.start_tx = None;
        self.start_rx = None;
    }

    fn drain_event_stream(&mut self) {
        let pending = self
            .event_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut dead = false;
        for event in pending {
            match event {
                EventStreamEvent::Snapshot { cause, snapshot } => {
                    tracing::trace!(
                        target = "muxterm::herdr",
                        workspace = %self.workspace_id,
                        event = ?cause,
                        "apply Herdr event snapshot"
                    );
                    self.reconcile_snapshot(&snapshot);
                }
                EventStreamEvent::Layout(layout) => {
                    self.apply_layout_record(&layout, true);
                }
                EventStreamEvent::Closed => {
                    tracing::info!(
                        target = "muxterm::herdr",
                        workspace = %self.workspace_id,
                        "Herdr event subscription closed"
                    );
                    // 订阅已死：必须重建，否则 pane.agent_status_changed
                    // 等事件永久丢失（done/blocked 通知收不到）。
                    dead = true;
                }
                EventStreamEvent::Error(message) => {
                    tracing::warn!(
                        target = "muxterm::herdr",
                        workspace = %self.workspace_id,
                        error = %message,
                        "Herdr event subscription error"
                    );
                    dead = true;
                }
            }
        }
        if dead {
            self.restart_event_stream();
        }
    }

    /// Apply a post-connect session snapshot as a diff in the shared Runtime
    /// model. Structural events, titles, focus, layouts and full agent records
    /// all converge here; no Herdr event spelling escapes this module.
    fn reconcile_snapshot(&mut self, snap: &SessionSnapshot) {
        let old_name = self.workspace_name.clone();
        let old_tabs: HashMap<TabId, TabInfo> =
            self.tabs.iter().cloned().map(|tab| (tab.id, tab)).collect();
        let old_panes: HashMap<PaneId, PaneInfo> = self
            .panes
            .iter()
            .cloned()
            .map(|pane| (pane.id, pane))
            .collect();
        let old_layouts = self.layouts.clone();
        let old_agents = self.agents.clone();
        let old_active_tab = self.active_tab;
        let old_active_pane = self.active_pane;

        if !snap
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == self.workspace_id)
        {
            for pane in old_agents.keys() {
                self.events.push_back(StateChange::PaneAgentChanged {
                    pane: *pane,
                    agent: None,
                    initial: false,
                });
            }
            for pane in old_panes.keys() {
                self.events
                    .push_back(StateChange::PaneClosed { pane: *pane });
            }
            for tab in old_tabs.keys() {
                self.events.push_back(StateChange::TabClosed { tab: *tab });
            }
            self.tabs.clear();
            self.panes.clear();
            self.layouts.clear();
            self.agents.clear();
            self.outputs.clear();
            self.herdr_tab_to_tab.clear();
            self.herdr_pane_to_pane.clear();
            self.tab_to_herdr_tab.clear();
            self.pane_to_herdr_pane.clear();
            self.active_tab = None;
            self.active_pane = None;
            self.stop_all_streams();
            self.restart_event_stream();
            return;
        }

        if !self.apply_snapshot(snap, false) {
            return;
        }

        let new_tabs: HashMap<TabId, TabInfo> =
            self.tabs.iter().cloned().map(|tab| (tab.id, tab)).collect();
        let new_panes: HashMap<PaneId, PaneInfo> = self
            .panes
            .iter()
            .cloned()
            .map(|pane| (pane.id, pane))
            .collect();
        let old_pane_ids = old_panes.keys().copied().collect::<HashSet<_>>();
        let new_pane_ids = new_panes.keys().copied().collect::<HashSet<_>>();

        if old_name != self.workspace_name {
            self.events.push_back(StateChange::WorkspaceRenamed {
                name: self.workspace_name.clone(),
            });
        }

        for pane in old_panes
            .keys()
            .filter(|pane| !new_panes.contains_key(pane))
        {
            self.events
                .push_back(StateChange::PaneClosed { pane: *pane });
        }
        for tab in old_tabs.keys().filter(|tab| !new_tabs.contains_key(tab)) {
            self.events.push_back(StateChange::TabClosed { tab: *tab });
        }
        for tab in self
            .tabs
            .iter()
            .filter(|tab| !old_tabs.contains_key(&tab.id))
        {
            self.events.push_back(StateChange::TabAdded { tab: tab.id });
        }
        for tab in &self.tabs {
            if let Some(old) = old_tabs.get(&tab.id) {
                if old.name != tab.name {
                    self.events.push_back(StateChange::TabRenamed {
                        tab: tab.id,
                        name: tab.name.clone(),
                    });
                }
            }
        }
        for pane in &self.panes {
            match old_panes.get(&pane.id) {
                None => self.events.push_back(StateChange::PaneAdded {
                    pane: pane.id,
                    tab: pane.tab,
                }),
                Some(old) if old.tab != pane.tab => {
                    self.events
                        .push_back(StateChange::PaneClosed { pane: pane.id });
                    self.events.push_back(StateChange::PaneAdded {
                        pane: pane.id,
                        tab: pane.tab,
                    });
                }
                Some(old) => {
                    if old.title != pane.title {
                        self.events.push_back(StateChange::PaneTitleChanged {
                            pane: pane.id,
                            title: pane.title.clone(),
                        });
                    }
                    if old.cols != pane.cols || old.rows != pane.rows {
                        self.events.push_back(StateChange::PaneResized {
                            pane: pane.id,
                            cols: pane.cols,
                            rows: pane.rows,
                        });
                    }
                }
            }
        }
        for (tab, layout) in &self.layouts {
            if old_layouts.get(tab) != Some(layout) {
                self.events.push_back(StateChange::LayoutChanged {
                    tab: *tab,
                    layout: layout.clone(),
                });
            }
        }

        let all_agent_panes = old_agents
            .keys()
            .chain(self.agents.keys())
            .copied()
            .collect::<HashSet<_>>();
        for pane in all_agent_panes {
            if old_agents.get(&pane) != self.agents.get(&pane) {
                // Switching between a lifecycle hook and screen detection is a
                // new authority bootstrap, not a user-visible agent transition.
                // Herdr can briefly report Done while the detector settles to
                // Working; marking the handoff initial prevents that transient
                // state from generating a duplicate completion notification.
                let initial = agent_source_handoff_is_bootstrap(
                    old_agents
                        .get(&pane)
                        .map(|agent| agent.screen_detection_skipped),
                    self.agents
                        .get(&pane)
                        .map(|agent| agent.screen_detection_skipped),
                );
                self.events.push_back(StateChange::PaneAgentChanged {
                    pane,
                    agent: self.agents.get(&pane).cloned().map(Box::new),
                    initial,
                });
            }
        }
        if old_active_tab != self.active_tab {
            if let Some(tab) = self.active_tab {
                self.events.push_back(StateChange::ActiveTabChanged { tab });
            }
        }
        if old_active_pane != self.active_pane {
            if let Some((tab, pane)) = self.active_tab.zip(self.active_pane) {
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab, pane });
            }
        }

        self.outputs.retain(|pane, _| new_pane_ids.contains(pane));
        for pane in new_pane_ids.difference(&old_pane_ids).copied() {
            let Some(herdr_pane) = self.pane_to_herdr_pane.get(&pane).cloned() else {
                continue;
            };
            self.seed_one_pane(pane, &herdr_pane);
            // 新 pane 走统一 registry slot（W2）；mutation 收敛见 W5。
            let mode = self.desired_mode_for(
                self.panes
                    .iter()
                    .find(|candidate| candidate.id == pane)
                    .unwrap_or_else(|| panic!("新 pane {pane} 必须已在 panes 里")),
            );
            self.stream_slots
                .entry(pane)
                .or_insert_with(|| PaneStreamSlot::new(pane, herdr_pane, mode))
                .desired_mode = mode;
        }
        self.reconcile_stream_modes();
        if old_pane_ids != new_pane_ids {
            self.restart_event_stream();
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

/// 把 Herdr 的零尺寸 wire sentinel 收敛成通用 Runtime 可用的终端尺寸。
fn normalize_pane_size(width: u16, height: u16, previous: Option<(u16, u16)>) -> (u16, u16) {
    let previous = previous.unwrap_or((DEFAULT_HERDR_COLS, DEFAULT_HERDR_ROWS));
    let previous_cols = if previous.0 == 0 {
        DEFAULT_HERDR_COLS
    } else {
        previous.0
    };
    let previous_rows = if previous.1 == 0 {
        DEFAULT_HERDR_ROWS
    } else {
        previous.1
    };
    (
        if width == 0 { previous_cols } else { width },
        if height == 0 { previous_rows } else { height },
    )
}

fn layout_has_usable_geometry(layout: &LayoutRecord) -> bool {
    layout.area.width > 0
        && layout.area.height > 0
        && layout
            .panes
            .iter()
            .all(|pane| pane.rect.width > 0 && pane.rect.height > 0)
        && layout
            .splits
            .iter()
            .all(|split| split.rect.width > 0 && split.rect.height > 0)
}

fn product_agent(agent: &AgentRecord) -> PaneAgentInfo {
    PaneAgentInfo {
        terminal_id: agent.terminal_id.clone(),
        name: agent.name.clone(),
        kind: agent.agent.clone(),
        title: agent.title.clone(),
        terminal_title: agent.terminal_title.clone(),
        terminal_title_stripped: agent.terminal_title_stripped.clone(),
        display_name: agent.display_agent.clone(),
        status: match &agent.agent_status {
            HerdrAgentStatus::Idle => PaneAgentStatus::Idle,
            HerdrAgentStatus::Working => PaneAgentStatus::Working,
            HerdrAgentStatus::Blocked => PaneAgentStatus::Blocked,
            HerdrAgentStatus::Done => PaneAgentStatus::Done,
            HerdrAgentStatus::Unknown(_) => PaneAgentStatus::Unknown,
        },
        screen_detection_skipped: agent.screen_detection_skipped,
        state_labels: agent.state_labels.clone(),
        tokens: agent.tokens.clone(),
        session: agent
            .agent_session
            .as_ref()
            .map(|session| PaneAgentSession {
                source: session.source.clone(),
                agent: session.agent.clone(),
                kind: match session.kind.as_str() {
                    "id" => PaneAgentSessionKind::Id,
                    "path" => PaneAgentSessionKind::Path,
                    other => PaneAgentSessionKind::Unknown(other.to_string()),
                },
                value: session.value.clone(),
            }),
        focused: agent.focused,
        launch_pending: agent.launch_pending,
        interactive_ready: agent.interactive_ready,
        state_change_seq: agent.state_change_seq,
        cwd: agent.cwd.clone(),
        foreground_cwd: agent.foreground_cwd.clone(),
        revision: agent.revision,
    }
}

fn agent_source_handoff_is_bootstrap(old: Option<bool>, new: Option<bool>) -> bool {
    matches!((old, new), (Some(old), Some(new)) if old != new)
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

    fn pane_agent(&self, pane: &PaneId) -> Option<&PaneAgentInfo> {
        self.agents.get(pane)
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

    fn set_foreground(&mut self, foreground: bool) {
        if self.foreground == foreground {
            return;
        }
        self.foreground = foreground;
        tracing::info!(
            target = "muxterm::herdr",
            workspace = %self.workspace_id,
            foreground = foreground,
            "Pool foreground 切换"
        );
        self.reconcile_stream_modes();
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
        if !self.apply_snapshot(&snap, true) {
            return Err(anyhow!(
                "Herdr workspace {} 不在 session.snapshot 中",
                self.workspace_id
            ));
        }
        if self.panes.is_empty() {
            return Err(anyhow!(
                "Herdr workspace {} 在 snapshot 里没有 pane",
                self.workspace_id
            ));
        }
        self.seed_pane_read();
        self.bootstrap_stream_slots();
        self.start_event_stream()
            .context("Herdr events.subscribe 失败")?;

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
                if self.herdr_pane(*target).is_none() {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 不存在"),
                    });
                }
                self.send_control_input(*target, data)
                    .map_err(|e| anyhow!("terminal control input 失败: {e}"))?;
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
                self.resize_control_stream(*target, cols, rows)
                    .map_err(|err| anyhow!("Herdr pane control resize 失败: {err}"))?;
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
                let Some(herdr_pane) = self.herdr_pane(*target).map(ToOwned::to_owned) else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("pane {target} 缺 Herdr id"),
                    });
                };
                self.session
                    .call("pane.focus", serde_json::json!({ "pane_id": herdr_pane }))
                    .map_err(|e| anyhow!("pane.focus 失败: {e}"))?;
                let tab_changed = self.active_tab != Some(tab);
                for t in self.tabs.iter_mut() {
                    t.active = t.id == tab;
                }
                self.active_tab = Some(tab);
                for p in self.panes.iter_mut() {
                    p.active = p.id == *target;
                }
                if let Some(l) = self.layouts.get_mut(&tab) {
                    l.active = *target;
                }
                self.active_pane = Some(*target);
                if tab_changed {
                    self.events.push_back(StateChange::ActiveTabChanged { tab });
                }
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab, pane: *target });
                // 真实本地 focus edge：新 control intent（可 takeover 一次）。
                self.promote_focus_to(*target);
                Ok(TaskOutcome::Done)
            }
            Task::SwitchTab { target } => {
                if !self.tabs.iter().any(|t| t.id == *target) {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 不存在"),
                    });
                }
                let Some(herdr_tab) = self.tab_to_herdr_tab.get(target).cloned() else {
                    return Ok(TaskOutcome::Rejected {
                        reason: format!("tab {target} 缺 Herdr id"),
                    });
                };
                self.session
                    .call("tab.focus", serde_json::json!({ "tab_id": herdr_tab }))
                    .map_err(|e| anyhow!("tab.focus 失败: {e}"))?;
                for t in self.tabs.iter_mut() {
                    t.active = t.id == *target;
                }
                self.active_tab = Some(*target);
                if let Some(pane) = self.layouts.get(target).map(|layout| layout.active) {
                    for candidate in self.panes.iter_mut() {
                        candidate.active = candidate.id == pane;
                    }
                    self.active_pane = Some(pane);
                    self.events
                        .push_back(StateChange::ActivePaneChanged { tab: *target, pane });
                }
                self.events
                    .push_back(StateChange::ActiveTabChanged { tab: *target });
                // tab 切换也是本地 focus edge：新 active pane 获得 control intent。
                if let Some(pane) = self.active_pane {
                    self.promote_focus_to(pane);
                }
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
                            "focus": true,
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
                let created_herdr_pane = result
                    .get("pane")
                    .and_then(|pane| pane.get("pane_id"))
                    .and_then(serde_json::Value::as_str);
                if let Some(pane_id) = created_herdr_pane {
                    self.session
                        .call("pane.focus", serde_json::json!({ "pane_id": pane_id }))
                        .map_err(|e| anyhow!("pane.split 后 pane.focus 失败: {e}"))?;
                }
                let authoritative_layout = created_herdr_pane.and_then(|pane_id| {
                    match self.session.pane_layout(pane_id) {
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
                    }
                });
                self.apply_split_pane(&result, tab, *dir, authoritative_layout.as_ref());
                Ok(TaskOutcome::Done)
            }
            Task::Detach => {
                // detach = 客户端断开连接（保留服务端 session）。必须主动
                // 关闭全部 stream/event 流：否则 reopen 的新 runtime
                // takeover 杀掉旧流后，旧流的 Error/Closed 会触发自动重建
                // （新流又被踢掉），流互踢导致服务端内容反复重置。
                self.event_stream = None;
                self.stop_all_streams();
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
        self.drain_event_stream();
        self.drain_stream();
        self.drain_start_results();
        let now = Instant::now();
        self.degrade_stalled_streams(now);
        self.maybe_start_pending_retries(now);
        self.reconcile_stream_modes();
        self.events.drain(..).collect()
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.event_stream = None;
        self.event_tx = None;
        self.event_rx = None;
        self.stop_all_streams();
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
        self.event_stream = None;
        self.event_tx = None;
        self.event_rx = None;
        self.stop_all_streams();
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
        for tab in &mut self.tabs {
            tab.active = false;
        }
        for pane in &mut self.panes {
            pane.active = false;
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
            self.active_tab = Some(id);
            self.active_pane = Some(pid);
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
        let (cols, rows) = normalize_pane_size(
            rect.map(|rect| rect.width).unwrap_or(0),
            rect.map(|rect| rect.height).unwrap_or(0),
            None,
        );
        self.panes.push(PaneInfo {
            id: pid,
            tab,
            active: true,
            title: "herdr".into(),
            cols,
            rows,
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

        // 新 split pane 走统一 slot bootstrap（W2 registry；W5 改为
        // ensure_pane_initialized + mutation 收敛）。
        self.bootstrap_stream_slots();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::herdr::session::{LayoutPaneRecord, LayoutSplitRecord};
    use crate::core::runtime::herdr::wire::{read_message, ClientMessage, MAX_FRAME_SIZE};

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

    /// Herdr protocol 19 uses a zero pane rect for a background workspace
    /// before that workspace has a foreground viewport. The normalized
    /// Runtime model must never expose that wire sentinel to Workspace.
    #[test]
    fn zero_sized_background_layout_uses_last_or_default_terminal_size() {
        assert_eq!(normalize_pane_size(0, 0, None), (80, 24));
        assert_eq!(normalize_pane_size(0, 0, Some((132, 41))), (132, 41));
        assert_eq!(normalize_pane_size(54, 23, Some((132, 41))), (54, 23));
        assert_eq!(normalize_pane_size(0, 0, Some((0, 0))), (80, 24));
    }

    #[test]
    fn agent_detection_source_handoff_is_a_notification_free_bootstrap() {
        assert!(agent_source_handoff_is_bootstrap(Some(true), Some(false)));
        assert!(agent_source_handoff_is_bootstrap(Some(false), Some(true)));
        assert!(!agent_source_handoff_is_bootstrap(Some(false), Some(false)));
        assert!(!agent_source_handoff_is_bootstrap(Some(true), Some(true)));
        assert!(!agent_source_handoff_is_bootstrap(None, Some(false)));
        assert!(!agent_source_handoff_is_bootstrap(Some(false), None));
    }

    /// A zero-area background snapshot carries no pane placement information.
    /// It may update focus, but must not turn an existing vertical split into
    /// the legacy horizontal fallback or erase the last usable pane sizes.
    #[test]
    fn zero_sized_background_layout_preserves_existing_tree_and_geometry() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        let tab = TabId(1);
        let first = PaneId(1);
        let second = PaneId(2);
        runtime.herdr_tab_to_tab.insert("w1:t1".into(), tab);
        runtime.herdr_pane_to_pane.insert("w1:p1".into(), first);
        runtime.herdr_pane_to_pane.insert("w1:p2".into(), second);
        runtime.panes = vec![
            PaneInfo {
                id: first,
                tab,
                active: true,
                title: "first".into(),
                cols: 90,
                rows: 30,
            },
            PaneInfo {
                id: second,
                tab,
                active: false,
                title: "second".into(),
                cols: 90,
                rows: 30,
            },
        ];
        let expected_tree = LayoutNode::Split {
            dir: SplitDir::Vertical,
            ratio: 500,
            first: Box::new(LayoutNode::Leaf(first)),
            second: Box::new(LayoutNode::Leaf(second)),
        };
        runtime.layouts.insert(
            tab,
            TabLayout {
                tab,
                tree: expected_tree.clone(),
                active: first,
            },
        );
        let zero = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let layout = LayoutRecord {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            zoomed: false,
            area: zero,
            focused_pane_id: "w1:p2".into(),
            panes: vec![
                LayoutPaneRecord {
                    pane_id: "w1:p1".into(),
                    focused: false,
                    rect: zero,
                },
                LayoutPaneRecord {
                    pane_id: "w1:p2".into(),
                    focused: true,
                    rect: zero,
                },
            ],
            splits: vec![LayoutSplitRecord {
                id: "split_0_root".into(),
                path: vec![],
                direction: LayoutSplitDirection::Down,
                ratio: 0.5,
                rect: zero,
            }],
        };

        assert!(runtime.apply_layout_record(&layout, false));
        let normalized = runtime.layouts.get(&tab).expect("layout must remain");
        assert_eq!(normalized.tree, expected_tree);
        assert_eq!(normalized.active, second);
        assert!(runtime
            .panes
            .iter()
            .all(|pane| (pane.cols, pane.rows) == (90, 30)));
    }

    /// `terminal.frame(full=true)` 是 observer 当前屏幕的完整 ANSI 重绘，
    /// 不是可追加的历史增量。Surface 仍需收到原始帧，但 Runtime 的 attach
    /// seed 快照必须替换旧 full frame，否则切 tab 会把数 MB 重复画面重灌 VTE。
    #[test]
    fn full_observe_frame_replaces_seed_buffer_before_incremental_bytes() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        runtime.status = BackendStatus::Connected;
        let pane = PaneId(1);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "pane".into(),
            cols: 80,
            rows: 24,
        });
        runtime.pane_to_herdr_pane.insert(pane, "w1:p1".into());
        let (tx, rx) = super::super::observe::channel();
        runtime.stream_tx = Some(tx.clone());
        runtime.stream_rx = Some(rx);
        let mut slot = PaneStreamSlot::new(pane, "w1:p1", StreamMode::Observe);
        slot.generation = 7;
        slot.state = SlotState::Live;
        slot.actual_mode = Some(StreamMode::Observe);
        runtime.stream_slots.insert(pane, slot);

        // generation 7 的首个 full frame（event ordinal 1、wire seq 1）。
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 7,
            event_ordinal: 1,
            wire_seq: 1,
            bytes: b"FULL_ONE".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        runtime.drain_stream();
        assert_eq!(runtime.outputs.get(&pane).unwrap(), b"FULL_ONE");

        // 第二个 full（seq 2）：必须替换 seed buffer。
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 7,
            event_ordinal: 2,
            wire_seq: 2,
            bytes: b"FULL_TWO".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        runtime.drain_stream();
        assert_eq!(
            runtime.outputs.get(&pane).unwrap(),
            b"FULL_TWO",
            "第二个 full frame 必须替换 seed buffer，禁止 FULL_ONEFULL_TWO"
        );

        // diff（seq 3）：增量追加。
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 7,
            event_ordinal: 3,
            wire_seq: 3,
            bytes: b"_DIFF".to_vec(),
            width: 80,
            height: 24,
            full: false,
        })
        .unwrap();
        runtime.drain_stream();
        assert_eq!(runtime.outputs.get(&pane).unwrap(), b"FULL_TWO_DIFF");

        let output_events = runtime
            .events
            .iter()
            .filter_map(|event| match event {
                StateChange::PaneFrame { data, .. } => Some((true, data.as_slice())),
                StateChange::PaneOutput { data, .. } => Some((false, data.as_slice())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            output_events,
            vec![
                (true, b"FULL_ONE".as_slice()),
                (true, b"FULL_TWO".as_slice()),
                (false, b"_DIFF".as_slice())
            ],
            "Runtime 必须保留 full/diff 语义，同时按顺序交付每个原始 ANSI frame"
        );
    }

    /// 旧 generation 的 Frame/Closed/Error 全部丢弃（stale 事件零副作用）。
    #[test]
    fn stale_generation_events_are_dropped() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        runtime.status = BackendStatus::Connected;
        let pane = PaneId(1);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "pane".into(),
            cols: 80,
            rows: 24,
        });
        runtime.pane_to_herdr_pane.insert(pane, "w1:p1".into());
        let (tx, rx) = super::super::observe::channel();
        runtime.stream_tx = Some(tx.clone());
        runtime.stream_rx = Some(rx);
        let mut slot = PaneStreamSlot::new(pane, "w1:p1", StreamMode::Observe);
        slot.generation = 2;
        slot.state = SlotState::Live;
        slot.actual_mode = Some(StreamMode::Observe);
        runtime.stream_slots.insert(pane, slot);

        // 旧 generation 1 的事件：Frame/Closed/Error 全部不得生效。
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 1,
            event_ordinal: 1,
            wire_seq: 1,
            bytes: b"STALE_FRAME".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        tx.send(PaneStreamEvent::Closed {
            pane,
            generation: 1,
            event_ordinal: 2,
            reason: Some("stale close".into()),
        })
        .unwrap();
        tx.send(PaneStreamEvent::Error {
            pane,
            generation: 1,
            event_ordinal: 3,
            message: "stale error".into(),
        })
        .unwrap();
        runtime.drain_stream();
        assert!(
            runtime.outputs.get(&pane).is_none(),
            "stale generation 不得写入 outputs"
        );
        assert_eq!(
            runtime.stream_slots.get(&pane).unwrap().state,
            SlotState::Live,
            "stale Closed/Error 不得把 current generation 打进 Backoff"
        );
        assert_eq!(runtime.stream_slots.get(&pane).unwrap().retry_count, 0);
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

    /// herdr server 握手模拟：读 Hello/ControlTerminal，回 Welcome；
    /// 返回保留缓冲的 reader（后续 Input 用同一 reader 读）。
    fn mock_observe_handshake(
        stream: &mut std::os::unix::net::UnixStream,
    ) -> std::io::BufReader<std::os::unix::net::UnixStream> {
        use std::io::Write;
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let _hello: ClientMessage =
            read_message(&mut reader, MAX_FRAME_SIZE).expect("读 Hello 失败");
        // Welcome bincode payload: [0,19,1,0]（version=19, encoding=TerminalAnsi, error=None）。
        stream
            .write_all(b"\x04\x00\x00\x00\x00\x13\x01\x00")
            .expect("写 Welcome 失败");
        stream.flush().unwrap();
        let _control: ClientMessage =
            read_message(&mut reader, MAX_FRAME_SIZE).expect("读 ControlTerminal 失败");
        reader
    }

    /// 共享场景：起一个 mock herdr server，建立真实 control 流，发
    /// Error/Closed 事件，断言死流被移除、进入有界退避（不立即重建）、
    /// retry 到期后自动重建且重建后的流能送达输入。
    fn run_observe_removal_scenario(use_error: bool) {
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!(
            "muxterm-test-observe-{}-{use_error}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let api_socket = dir.join("herdr.sock");
        let session = Arc::new(HerdrSession::new("test", &api_socket));
        let client_socket = session.client_socket_path().to_path_buf();
        let listener = UnixListener::bind(&client_socket).unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            // 第一次连接（初始 control 流）。
            let (mut first, _) = listener.accept().expect("accept 初始流失败");
            let first_reader = mock_observe_handshake(&mut first);
            ready_tx.send(0usize).unwrap();
            // 第二次连接：retry 到期后的自动重建。
            let (mut second, _) = listener.accept().expect("accept 重建流失败");
            let mut second_reader = mock_observe_handshake(&mut second);
            ready_tx.send(1usize).unwrap();
            let input: ClientMessage =
                read_message(&mut second_reader, MAX_FRAME_SIZE).expect("读 Input 失败");
            assert!(matches!(input, ClientMessage::Input { .. }));
            // 保持连接，避免测试结束前 reader 线程 EOF。
            std::thread::sleep(Duration::from_secs(1));
            drop(first_reader);
            drop(second_reader);
        });

        let mut runtime = HerdrRuntime::new(session, "w1");
        runtime.status = BackendStatus::Connected;
        runtime.foreground = true;
        let pane = PaneId(1);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "pane".into(),
            cols: 80,
            rows: 24,
        });
        runtime.active_pane = Some(pane);
        runtime.pane_to_herdr_pane.insert(pane, "w1:p1".into());
        runtime.ensure_stream_channels();
        let tx = runtime.stream_tx.as_ref().cloned().unwrap();

        // 初始 control 流（generation 1；open/activate 语义 takeover=false）。
        let generation = 1u64;
        let stream = ObserveStream::start(
            &client_socket,
            "w1:p1",
            pane,
            generation,
            StreamMode::Control,
            false,
            80,
            24,
            tx.clone(),
        )
        .expect("初始 control 流启动失败");
        let mut slot = PaneStreamSlot::new(pane, "w1:p1", StreamMode::Control);
        slot.generation = generation;
        slot.state = SlotState::Live;
        slot.actual_mode = Some(StreamMode::Control);
        slot.stream = Some(stream);
        runtime.stream_slots.insert(pane, slot);
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(3)),
            Ok(0),
            "server 必须先完成初始握手"
        );

        // 模拟 reader 线程退出：Error（读帧失败）或 Closed（EOF）。
        let event = if use_error {
            PaneStreamEvent::Error {
                pane,
                generation,
                event_ordinal: 1,
                message: "读 Herdr 帧长度失败".into(),
            }
        } else {
            PaneStreamEvent::Closed {
                pane,
                generation,
                event_ordinal: 1,
                reason: Some("模拟关闭".into()),
            }
        };
        tx.send(event).unwrap();
        runtime.drain_stream();
        // 死流必须移除并进入有界退避，**不是**立即重建。
        let slot = runtime.stream_slots.get(&pane).expect("slot 必须存在");
        assert!(
            slot.stream.is_none(),
            "{} 后死流必须移除",
            if use_error { "Error" } else { "Closed" }
        );
        assert_eq!(
            slot.state,
            SlotState::Backoff,
            "普通故障必须进入 Backoff，不能立即重建"
        );
        assert_eq!(slot.retry_count, 1, "第一次普通故障安排一次 retry");
        assert!(slot.retry_at.is_some(), "retry 必须安排在将来时点");
        assert!(
            ready_rx.try_recv().is_err(),
            "未到 retry 时点前不得重建（禁止无界立即重建）"
        );

        // 模拟时间流逝：retry_at 到期 → maybe_start_pending_retries 重建。
        runtime.stream_slots.get_mut(&pane).unwrap().retry_at =
            Some(Instant::now() - Duration::from_millis(1));
        runtime.maybe_start_pending_retries(Instant::now());
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(3)),
            Ok(1),
            "retry 到期后必须自动重建：server 应收到第二次连接"
        );
        // worker 的 Started 结果可能稍晚于 server 的 ready 信号：轮询到 Live。
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            runtime.drain_start_results();
            let slot = runtime.stream_slots.get(&pane).unwrap();
            if slot.state == SlotState::Live {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "start worker 未在时限内完成（state={:?}）",
                slot.state
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let slot = runtime.stream_slots.get(&pane).unwrap();
        assert_eq!(slot.state, SlotState::Live, "重建后新流必须 Live");
        assert!(slot.stream.is_some(), "重建后新流必须存在");

        // 重建后的 control 流必须能送达输入。
        runtime.send_control_input(pane, b"x").unwrap();

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn observe_error_removes_dead_stream_and_bounded_retry() {
        run_observe_removal_scenario(true);
    }

    #[test]
    fn observe_closed_removes_dead_stream_and_bounded_retry() {
        run_observe_removal_scenario(false);
    }

    /// mock API server：只应答 pane.read，返回固定 ANSI 文本。
    fn mock_pane_read_server(socket_path: &std::path::Path, text: &'static str) {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;
        let listener = UnixListener::bind(socket_path).expect("bind mock API socket 失败");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let method = v["method"].as_str().unwrap_or("");
                let id = v["id"].clone();
                let resp = match method {
                    "pane.read" => serde_json::json!({
                        "id": id,
                        "result": { "read": { "text": text } },
                    }),
                    "ping" => serde_json::json!({
                        "id": id,
                        "result": { "type": "pong" },
                    }),
                    _ => serde_json::json!({
                        "id": id,
                        "error": format!("unknown {method}"),
                    }),
                };
                let _ = stream.write_all((resp.to_string() + "\n").as_bytes());
                let _ = stream.flush();
            }
        });
    }

    /// W3：`pane.read` 只产生 `PaneIndexSnapshot`（Index 面），不得产生
    /// `PaneFrame/PaneOutput/PaneSnapshot`（Surface 面）。
    #[test]
    fn pane_read_seeds_index_without_surface_event() {
        let dir = std::env::temp_dir().join(format!("muxterm-test-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let api_socket = dir.join("herdr.sock");
        let seed_text = "\x1b[2J\x1b[HSEED_INDEX_TOKEN\r\n";
        mock_pane_read_server(&api_socket, seed_text);

        let mut runtime = HerdrRuntime::new(Arc::new(HerdrSession::new("test", &api_socket)), "w1");
        let pane = PaneId(1);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "pane".into(),
            cols: 80,
            rows: 24,
        });
        runtime.pane_to_herdr_pane.insert(pane, "w1:p1".into());
        runtime.seed_one_pane(pane, "w1:p1");

        let surface_events = runtime
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    StateChange::PaneFrame { .. }
                        | StateChange::PaneOutput { .. }
                        | StateChange::PaneSnapshot { .. }
                )
            })
            .count();
        assert_eq!(
            surface_events, 0,
            "pane.read 不得产生任何 Surface 事件（PaneFrame/PaneOutput/PaneSnapshot）"
        );
        assert!(
            runtime.events.iter().any(|event| {
                matches!(
                    event,
                    StateChange::PaneIndexSnapshot { pane: p, .. } if *p == pane
                )
            }),
            "pane.read 必须产生 PaneIndexSnapshot"
        );
        assert_eq!(
            runtime.outputs.get(&pane).map(Vec::as_slice),
            Some(seed_text.as_bytes()),
            "Index 快照字节必须进 outputs"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W3：generation 切换时保留旧像素（outputs），直到新 generation 的
    /// full frame 到达才替换；旧 generation 的 full 不得覆盖当前 Index。
    #[test]
    fn generation_change_keeps_old_surface_until_new_full() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        runtime.status = BackendStatus::Connected;
        let pane = PaneId(1);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "pane".into(),
            cols: 80,
            rows: 24,
        });
        runtime.pane_to_herdr_pane.insert(pane, "w1:p1".into());
        let (tx, rx) = super::super::observe::channel();
        runtime.stream_tx = Some(tx.clone());
        runtime.stream_rx = Some(rx);
        let mut slot = PaneStreamSlot::new(pane, "w1:p1", StreamMode::Observe);
        slot.generation = 1;
        slot.state = SlotState::Live;
        slot.actual_mode = Some(StreamMode::Observe);
        runtime.stream_slots.insert(pane, slot);

        // generation 1 的首个 full：写入 outputs。
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 1,
            event_ordinal: 1,
            wire_seq: 1,
            bytes: b"GEN1_FULL".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        runtime.drain_stream();
        assert_eq!(runtime.outputs.get(&pane).unwrap(), b"GEN1_FULL");

        // 切换到 generation 2（模拟 promote/demote）：旧像素保留。
        runtime.start_stream_replacing(pane, StreamMode::Observe, false);
        assert_eq!(
            runtime.outputs.get(&pane).unwrap(),
            b"GEN1_FULL",
            "generation 切换不得清空旧像素"
        );

        // generation 1 的迟到 full：必须丢弃（stale），不得覆盖当前 Index。
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 1,
            event_ordinal: 2,
            wire_seq: 2,
            bytes: b"STALE_GEN1_FULL".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        runtime.drain_stream();
        assert_eq!(
            runtime.outputs.get(&pane).unwrap(),
            b"GEN1_FULL",
            "stale generation full 不得覆盖当前 Index"
        );

        // generation 2 的 full：替换旧像素。
        let gen2 = runtime.stream_slots.get(&pane).unwrap().generation;
        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: gen2,
            event_ordinal: 1,
            wire_seq: 1,
            bytes: b"GEN2_FULL".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        runtime.drain_stream();
        assert_eq!(
            runtime.outputs.get(&pane).unwrap(),
            b"GEN2_FULL",
            "新 generation full 到达后才替换旧像素"
        );
    }

    /// W3：full frame 超时（fake clock）→ Degraded，且不 fallback 重放
    /// pane.read（outputs 保持未播种）。
    #[test]
    fn full_frame_timeout_after_five_seconds_is_degraded_without_index_fallback() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        runtime.status = BackendStatus::Connected;
        let pane = PaneId(1);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "pane".into(),
            cols: 80,
            rows: 24,
        });
        runtime.pane_to_herdr_pane.insert(pane, "w1:p1".into());
        runtime.ensure_stream_channels();
        let mut slot = PaneStreamSlot::new(pane, "w1:p1", StreamMode::Observe);
        slot.generation = 1;
        slot.state = SlotState::Starting;
        slot.started_at = Some(Instant::now() - std::time::Duration::from_secs(6));
        runtime.stream_slots.insert(pane, slot);

        runtime.degrade_stalled_streams(Instant::now());
        assert_eq!(
            runtime.test_slot_state(pane),
            Some(SlotState::Degraded),
            "5 秒无 full 必须 Degraded"
        );
        assert_eq!(
            runtime.outputs.get(&pane),
            None,
            "Degraded 不得 fallback 重放 pane.read"
        );
        assert_eq!(runtime.test_stream_starts(pane), 0, "超时不触发重试启动");
    }
}
