//! HerdrRuntime：绑定一个 Herdr workspace 的 Runtime 视图。
//!
//! 一个 `HerdrSession`（Arc）可被多个 `HerdrRuntime` 共享；每个 Runtime
//! 只填一个 Muxterm Workspace。直播字节走 observe 流（client socket），
//! attach 快照用 `pane.read`；原始键盘字节走 `pane.send_text`，语义按键走
//! `pane.send_keys`。逐键输入禁止走会自动包 bracketed-paste 的 `pane.send_input`。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use crate::core::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES};
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{LayoutNode, SplitDir, TabLayout};
use crate::core::model::state::{
    BackendStatus, MutationKind, MutationResult, MutationStage, PaneAgentInfo, PaneAgentSession,
    PaneAgentSessionKind, PaneAgentStatus, PaneInfo, State, StateChange, TabInfo,
};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::input::KeyEvent;
use crate::core::types::{PaneId, TabId};

use super::events::{EventStream, EventStreamEvent};
use super::mutation::{MutationQueue, PendingMutation};
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
    /// 每 tab 的权威 active pane：只由 session.snapshot（apply_snapshot）与
    /// 本地已同步到 server 的焦点意图（SwitchPane / mutation settle）更新。
    /// `layout.updated` 事件流记录可能携带旧 focused_pane_id（事件与快照
    /// 两条通道无序），切 tab 恢复焦点时必须以本映射为准，否则晚到旧事件
    /// 会把恢复目标回退到创建前（agent e2e 可见）。
    snapshot_active: HashMap<TabId, PaneId>,
    /// 帧携带的 pane 尺寸（权威）：apply_snapshot 重建 panes 时优先使用，
    /// 避免 snapshot rect（可能滞后）覆盖帧刚更新的尺寸、造成 VTE 反复
    /// resize 内容重排（matrix ctrl_l CLB 漂移根因之一）。
    frame_sizes: HashMap<PaneId, (u16, u16)>,
    /// GTK 分配的 client viewport（ResizePane 写入）。Hello 用它，不用
    /// snapshot 的 split-cell rect（76×12 会让 htop Observe 永远缩在小格子里）。
    preferred_client_size: Option<(u16, u16)>,
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
    /// 异步 tab/pane mutation 队列（W5：同时至多一个 in-flight）。
    mutation_queue: MutationQueue,
    /// lifecycle generation：detach/shutdown 后晚到的 mutation 结果直接丢弃。
    lifecycle_generation: u64,
    /// §6.4 完成态焦点：settle Completed 后，事件流里可能还排着 pane.focus
    /// 生效前拍的旧快照（reader 线程与 mutation 探针两条 session.snapshot
    /// 通道未排序），晚到会把焦点回退到创建前。短窗口内钉住 settled 焦点，
    /// 直到用户焦点意图或窗口到期。
    focus_pin: Option<FocusPin>,
    /// SSH 远端 socket 转发进程（Drop/shutdown 时杀掉）。
    forward: Option<std::process::Child>,
}

/// settle Completed 后短暂钉住的权威焦点（产品 id）。
#[derive(Debug, Clone, Copy)]
struct FocusPin {
    tab: TabId,
    pane: PaneId,
    until: Instant,
}

/// 钉住窗口：必须盖过 reader 晚到旧快照的传输延迟；
/// 用户显式切 pane/tab 会立即清除（见 `clear_focus_pin`）。
const FOCUS_PIN_TTL: Duration = Duration::from_secs(5);

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
            snapshot_active: HashMap::new(),
            frame_sizes: HashMap::new(),
            preferred_client_size: None,
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
            mutation_queue: MutationQueue::new(),
            lifecycle_generation: 0,
            focus_pin: None,
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
            // Herdr 快照可能不返回 label（如新建 tab 尚未命名）：
            // 回退到 public id 后缀的十进制形式（tA → "10"），保证 tab 始终可辨识。
            let name = if tab.label.trim().is_empty() {
                numeric_suffix(&tab.tab_id).to_string()
            } else {
                tab.label.clone()
            };
            self.tabs.push(TabInfo {
                id,
                name,
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
            // 帧尺寸权威：snapshot rect 可能滞后（resize 前的旧值），若帧
            // 已携带新尺寸则优先（frame_sizes 由 drain_stream 维护），避免
            // VTE 在 snapshot 与帧之间反复 resize 重排内容。
            let frame = self.frame_sizes.get(&id).copied();
            let (cols, rows) = match frame {
                Some((fc, fr)) if fc > 0 && fr > 0 => (fc, fr),
                _ => match previous {
                    Some((pc, pr)) if pc > 0 && pr > 0 => (pc, pr),
                    _ => pane_rect
                        .map(|rect| normalize_pane_size(rect.width, rect.height, None))
                        .unwrap_or_else(|| normalize_pane_size(0, 0, None)),
                },
            };
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
        self.snapshot_active
            .retain(|tab, _| current_tabs.contains(tab));

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
        // 快照重建后，权威表 = 每个 layout 记录的 focused pane（apply_layout_record
        // 的 snapshot 路径已写入）；只保留仍存在的 tab。
        self.snapshot_active
            .retain(|tab, _| self.layouts.contains_key(tab));

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
        // 晚到旧快照可能把 active 回退到 mutation 创建前：在 diff 计算**之前**
        // 把焦点钉回 settled 值（否则 reconcile 会把错误的 ActiveTabChanged /
        // ActivePaneChanged 发给 GUI，再 clamp 已来不及）。
        let pending_focus = self.pending_expected_focus();
        if let Some((pin_tab, pin_pane)) = self.pinned_focus().or(pending_focus) {
            if pending_focus.is_some() && self.pinned_focus().is_none() {
                self.focus_pin = Some(FocusPin {
                    tab: pin_tab,
                    pane: pin_pane,
                    until: Instant::now() + FOCUS_PIN_TTL,
                });
            }
            self.active_tab = Some(pin_tab);
            self.active_pane = Some(pin_pane);
            if let Some(layout) = self.layouts.get_mut(&pin_tab) {
                layout.active = pin_pane;
            }
        }
        for tab in &mut self.tabs {
            tab.active = Some(tab.id) == self.active_tab;
        }
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
            // 尺寸只由帧事件（drain_stream Frame）更新：layout 事件流与帧流
            // 交替到达，layout rect（54x23）会覆盖帧刚更新的 resize 后尺寸
            // （86x20），造成 VTE 反复 resize 重排内容（ctrl_l CLB 漂移根因）。
            let _ = pane;
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
                    if !emit_event {
                        self.snapshot_active.insert(tab, active);
                    }
                    let product_layout = TabLayout { active, ..existing };
                    self.layouts.insert(tab, product_layout.clone());
                    self.resync_active_from_layout(tab, active, emit_event);
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
        if !emit_event {
            self.snapshot_active.insert(tab, active);
        }
        let product_layout = TabLayout { tab, tree, active };
        self.layouts.insert(tab, product_layout.clone());
        self.resync_active_from_layout(tab, active, emit_event);
        if emit_event {
            self.events.push_back(StateChange::LayoutChanged {
                tab,
                layout: product_layout,
            });
        }
        true
    }

    /// 切 tab 时恢复目标 pane：权威快照优先（snapshot_active），layout 兜底。
    /// `layout.updated` 事件流记录与快照两条通道无序，晚到旧事件会把
    /// focused_pane_id 回退到 split 之前，因此恢复绝不能读事件污染的 layout。
    fn restore_active_for(&self, target: TabId) -> Option<PaneId> {
        self.snapshot_active
            .get(&target)
            .copied()
            .or_else(|| self.layouts.get(&target).map(|layout| layout.active))
    }

    /// `apply_layout_record` 只改 `layouts[tab].active`；若该 tab 正是 active tab，
    /// 必须同步 `active_pane`（否则事件流的 Layout 与 snapshot 乱序时，
    /// 产品 active pane 与 layout.active 会发散，SSH 权威契约可见）。
    /// 钉住窗口内，用钉值覆盖 incoming focused（旧 Layout 事件晚到不得回退）。
    fn resync_active_from_layout(&mut self, tab: TabId, active: PaneId, emit_event: bool) {
        let pin = self.pinned_focus();
        let effective = pin.map_or(
            active,
            |(pin_tab, pin_pane)| {
                if pin_tab == tab {
                    pin_pane
                } else {
                    active
                }
            },
        );
        if self.active_tab == Some(tab) && self.active_pane != Some(effective) {
            let old = self.active_pane;
            self.active_pane = Some(effective);
            for pane in &mut self.panes {
                pane.active = Some(pane.id) == self.active_pane;
            }
            if emit_event && old != Some(effective) {
                self.events.push_back(StateChange::ActivePaneChanged {
                    tab,
                    pane: effective,
                });
            }
        }
        if let Some((pin_tab, pin_pane)) = pin {
            if pin_tab == tab {
                if let Some(layout) = self.layouts.get_mut(&tab) {
                    layout.active = pin_pane;
                }
            }
        }
        // Layout 事件也可能晚到（reader 线程独立拍快照）：钉住 settled 焦点。
        self.enforce_focus_pin();
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

    /// 期望模式：in-flight mutation 已知目标时，目标 pane 独占 Control；
    /// 否则 Pool 前台 active pane = Control，其余 pane/后台 workspace = Observe。
    ///
    /// GTK preferred 未到前一律 Observe：Control Hello 默认 80×24 会对长期
    /// 运行的 htop 发 SIGWINCH，把 PTY 锁死在小屏（dogfood 2030 单 pane）。
    fn desired_mode_for(&self, pane: &PaneInfo) -> StreamMode {
        // pane.split/tab.create 的直接响应可能早于权威 snapshot。只要
        // 已知道目标 Herdr pane，就先按 mutation intent 计算最终 mode，
        // 避免新 pane 先 Observe、旧 active 仍 Control，随后反复 takeover。
        if let Some(expected_focus) = self
            .mutation_queue
            .in_flight()
            .and_then(|pending| pending.expected_focus.as_deref())
        {
            let is_expected = self
                .pane_to_herdr_pane
                .get(&pane.id)
                .is_some_and(|herdr_pane| herdr_pane == expected_focus);
            return if is_expected {
                StreamMode::Control
            } else {
                StreamMode::Observe
            };
        }
        if self.preferred_client_size.is_none() {
            return StreamMode::Observe;
        }
        if self.foreground && Some(pane.id) == self.active_pane {
            StreamMode::Control
        } else {
            StreamMode::Observe
        }
    }

    /// 为每个 pane 建 registry slot，但不启动异步 stream。
    ///
    /// attach 需要先把 `pane.read` 写入 seed_pending，再允许 worker 发出首个
    /// full frame；否则首帧可能在 seed 标记建立前到达并覆盖历史 Index。
    fn initialize_stream_slots(&mut self) {
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
        let (cols, rows) = self.hello_client_size(pane);
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
        // 用户焦点意图 = 新权威：钉住该 pane（旧的 settle 钉作废），
        // 直到 TTL 到期或下一次焦点意图/新 mutation 派发。不能只清除：
        // 否则晚到旧快照会把焦点弹回上一个 pane（e2e 矩阵可见）。
        let tab = self
            .panes
            .iter()
            .find(|candidate| candidate.id == pane)
            .map(|candidate| candidate.tab);
        self.focus_pin = tab.map(|tab| FocusPin {
            tab,
            pane,
            until: Instant::now() + FOCUS_PIN_TTL,
        });
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

    /// Hello / Observe viewport 尺寸：GTK preferred → pending_resize → 默认。
    /// 绝不用 snapshot split-cell rect（否则 htop 会锁在 76×12）。
    fn hello_client_size(&self, pane: PaneId) -> (u16, u16) {
        if let Some(slot) = self.stream_slots.get(&pane) {
            if let Some((cols, rows)) = slot.pending_resize {
                return normalize_pane_size(cols, rows, None);
            }
        }
        if let Some((cols, rows)) = self.preferred_client_size {
            return normalize_pane_size(cols, rows, None);
        }
        (DEFAULT_HERDR_COLS, DEFAULT_HERDR_ROWS)
    }

    /// resize 发给 current control（PTY）与 Observe（client viewport）。
    /// Starting/Backoff 期间 latest-wins 暂存。
    /// 首次写入 preferred 后 reconcile，把 deferred Observe 升到 Control。
    fn resize_control_stream(&mut self, pane: PaneId, cols: u16, rows: u16) -> Result<()> {
        let first_preferred = self.preferred_client_size.is_none();
        let (cols, rows) = normalize_pane_size(cols, rows, None);
        self.preferred_client_size = Some((cols, rows));
        let Some(slot) = self.stream_slots.get_mut(&pane) else {
            if first_preferred {
                self.reconcile_stream_modes();
            }
            return Err(anyhow!("pane {pane} 不存在"));
        };
        let result = match (slot.state, slot.actual_mode) {
            (SlotState::Live, Some(StreamMode::Control | StreamMode::Observe)) => {
                let stream = slot
                    .stream
                    .as_mut()
                    .ok_or_else(|| anyhow!("pane {pane} stream 缺失"))?;
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
                    // active pane 需要 control 才能收 PTY resize；无 intent 时
                    // open/activate 语义（takeover=false）启动即可。
                    // 先结束 slot 借用再 reconcile。
                    return self.resize_pending_then_reconcile(pane, cols, rows);
                }
                Ok(())
            }
        };
        if first_preferred {
            self.reconcile_stream_modes();
        }
        result
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
                        let prev = (p.cols, p.rows);
                        (p.cols, p.rows) =
                            normalize_pane_size(width, height, Some((p.cols, p.rows)));
                        self.frame_sizes.insert(pane, (p.cols, p.rows));
                        if (p.cols, p.rows) != prev {
                            self.events.push_back(StateChange::PaneResized {
                                pane,
                                cols: p.cols,
                                rows: p.rows,
                            });
                        }
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
                        let index_snapshot = if keep_seed {
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
                            // PaneFrame 会替换 Workspace 的 Surface/Index buffer；
                            // attach 的 pane.read 内容仍是搜索事实源，需在 full
                            // 之后再播种一次，不能让非活动 pane 的 baseline 抹掉
                            // 已存在的历史 token。
                            self.outputs.get(&pane).cloned()
                        } else {
                            self.outputs.insert(pane, bytes.clone());
                            None
                        };
                        self.events
                            .push_back(StateChange::PaneFrame { pane, data: bytes });
                        if let Some(data) = index_snapshot {
                            self.events
                                .push_back(StateChange::PaneIndexSnapshot { pane, data });
                        }
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
                    // Control：PTY SIGWINCH；Observe：client viewport。两者都要
                    // flush pending_resize，否则 Hello 用默认尺寸后永远卡在小屏。
                    let resize = slot.pending_resize.take();
                    let inputs: Vec<Vec<u8>> = if mode.is_control() {
                        let inputs: Vec<Vec<u8>> = slot.pending_input.drain(..).collect();
                        slot.pending_input_bytes = 0;
                        inputs
                    } else {
                        Vec::new()
                    };
                    if let Some((cols, rows)) = resize {
                        if let Some(stream) = slot.stream.as_mut() {
                            if let Err(err) = stream.resize(cols, rows) {
                                tracing::warn!(
                                    target = "muxterm::herdr",
                                    pane = %pane,
                                    error = %err,
                                    "stream handshake 后 resize 失败"
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
        // 晚到旧快照可能把焦点回退到 mutation 创建前：钉住 settled 焦点。
        self.enforce_focus_pin();
    }

    /// 幂等初始化一个新 pane（W5：所有发现新 pane 的入口都走这里）。
    ///
    /// 1. 建/复用 Herdr id ↔ 产品 PaneId 映射；
    /// 2. 建/更新 PaneInfo 与 Layout 关系；
    /// 3. 建 registry slot（存在则 reconcile，不重复 push）；
    /// 4. 为 Index 请求一次 seed（重复结果按 generation/初始化状态去重）；
    /// 5. 更新 event subscription 的 pane scope；
    /// 6. 仅在产品状态确实首次出现时发 PaneAdded。
    fn ensure_pane_initialized(&mut self, herdr_pane: &str, tab: TabId) -> PaneId {
        let pane = if let Some(existing) = self.herdr_pane_to_pane.get(herdr_pane) {
            *existing
        } else {
            let id = PaneId(numeric_suffix(herdr_pane));
            self.herdr_pane_to_pane.insert(herdr_pane.to_string(), id);
            self.pane_to_herdr_pane.insert(id, herdr_pane.to_string());
            id
        };
        if !self.panes.iter().any(|p| p.id == pane) {
            self.panes.push(PaneInfo {
                id: pane,
                tab,
                active: false,
                title: "herdr".into(),
                cols: 80,
                rows: 24,
            });
            self.events.push_back(StateChange::PaneAdded { pane, tab });
        }
        let mode = self.desired_mode_for(
            self.panes
                .iter()
                .find(|candidate| candidate.id == pane)
                .unwrap_or_else(|| panic!("新 pane {pane} 必须已在 panes 里")),
        );
        self.stream_slots
            .entry(pane)
            .or_insert_with(|| PaneStreamSlot::new(pane, herdr_pane.to_string(), mode))
            .desired_mode = mode;
        self.seed_one_pane(pane, herdr_pane);
        pane
    }

    /// 派发队头 mutation：记录 dispatch-time baseline 后发 socket 请求。
    /// 返回 Err 表示派发失败（settle 为 Failed）。
    fn dispatch_next_mutation(&mut self, now: Instant) -> Result<()> {
        // 新 mutation 派发 = 新权威意图：旧 settle 焦点钉作废，
        // 本 mutation 的 settle 会重新钉住（否则旧钉会把下一轮
        // mutation 的收敛焦点钳回上一个 pane，5 秒内永不收敛）。
        self.clear_focus_pin();
        let tabs: HashSet<String> = self
            .tabs
            .iter()
            .filter_map(|t| self.tab_to_herdr_tab.get(&t.id).cloned())
            .collect();
        let panes: HashSet<String> = self
            .panes
            .iter()
            .filter_map(|p| self.pane_to_herdr_pane.get(&p.id).cloned())
            .collect();
        // 必须在**队列里的真实项**上写 dispatched_at，再快照派发参数；
        // 不能 clone 后只标记副本（副本不写回 -> has_in_flight 永远 false，
        // 同一 mutation 每个 tick 重复派发，服务端会创建重复 tab）。
        let (operation_id, kind, new_tab_name, target_pane, split_dir) = {
            let Some(head) = self.mutation_queue.head_mut() else {
                return Ok(());
            };
            if head.dispatched_at.is_some() {
                return Ok(());
            }
            head.mark_dispatched(self.lifecycle_generation, tabs, panes, now);
            (
                head.mutation_id,
                head.kind,
                head.new_tab_name.clone(),
                head.target_pane.clone(),
                head.split_dir,
            )
        };
        let result = match kind {
            MutationKind::NewTab => {
                let mut params = serde_json::json!({
                    "workspace_id": self.workspace_id,
                    "focus": true,
                });
                // name=None 时完全省略 label（禁止空字符串覆盖权威数字名）。
                if let Some(name) = &new_tab_name {
                    params["label"] = serde_json::json!(name);
                }
                self.session.call("tab.create", params)
            }
            MutationKind::SplitPane => {
                let target = target_pane
                    .clone()
                    .ok_or_else(|| anyhow!("SplitPane 缺 target pane"))?;
                let direction = match split_dir {
                    Some(SplitDir::Horizontal) => "right",
                    Some(SplitDir::Vertical) => "down",
                    None => return Err(anyhow!("SplitPane 缺 direction")),
                };
                self.session.call(
                    "pane.split",
                    serde_json::json!({
                        "pane_id": target,
                        "direction": direction,
                    }),
                )
            }
        };
        match result {
            Ok(value) => {
                // 响应只填 expected ids，不直接推最终拓扑。
                if let Some(head) = self.mutation_queue.head_mut() {
                    if head.mutation_id == operation_id {
                        if let Some(tab_id) = value
                            .get("tab")
                            .and_then(|t| t.get("tab_id"))
                            .and_then(serde_json::Value::as_str)
                        {
                            head.expected_tab = Some(tab_id.to_string());
                        }
                        if let Some(pane_id) = value
                            .get("pane")
                            .and_then(|p| p.get("pane_id"))
                            .and_then(serde_json::Value::as_str)
                        {
                            head.expected_pane = Some(pane_id.to_string());
                        }
                        if let Some(pane_id) = value
                            .get("root_pane")
                            .and_then(|p| p.get("pane_id"))
                            .and_then(serde_json::Value::as_str)
                        {
                            head.expected_pane = Some(pane_id.to_string());
                        }
                        // pane.split 响应通常不带 tab 字段：created pane 必然属于
                        // 入队时记录的 target_tab（§6.1/§6.4）。
                        if kind == MutationKind::SplitPane && head.expected_tab.is_none() {
                            head.expected_tab = head.target_tab.clone();
                        }
                        // `expected_focus` is the wire-level pane identity
                        // whose stream must become Control before snapshot
                        // reconciliation.  Both tab.create and pane.split
                        // return (or expose) the newly created root pane.
                        head.expected_focus = head.expected_pane.clone();
                    }
                }
                // §6.4：pane.split 完成后必须在同一 worker 内请求 pane.focus，
                // 否则 Herdr 权威焦点不会落到新 pane，收敛条件永不成立。
                if kind == MutationKind::SplitPane {
                    if let Some(created_pane) = self
                        .mutation_queue
                        .in_flight()
                        .and_then(|head| head.expected_pane.clone())
                    {
                        self.session
                            .call("pane.focus", serde_json::json!({ "pane_id": created_pane }))?;
                    }
                }
                // 派发即钉住期望焦点：mutation 窗口内（settle 前）reader 晚到
                // 旧快照若把 active 弹回旧 pane，desired_mode 会在 Control/
                // Observe 间反复横跳，reconcile 不断 restart 流（generation
                // 递增、AwaitingFull 重置），5 秒内永远到不了 Ready。
                // 产品 id 用 bijective 解码直接算（拓扑里可能还没映射）。
                if let Some(head) = self.mutation_queue.in_flight() {
                    if head.mutation_id == operation_id {
                        if let (Some(tab_id), Some(pane_id)) =
                            (&head.expected_tab, &head.expected_pane)
                        {
                            let tab = TabId(numeric_suffix(tab_id));
                            let pane = PaneId(numeric_suffix(pane_id));
                            self.focus_pin = Some(FocusPin {
                                tab,
                                pane,
                                until: Instant::now() + FOCUS_PIN_TTL,
                            });
                            self.snapshot_active.insert(tab, pane);
                        }
                    }
                }
                Ok(())
            }
            Err(err) => {
                self.settle_mutation(
                    operation_id,
                    kind,
                    MutationResult::Failed {
                        stage: MutationStage::Dispatch,
                        reason: err.to_string(),
                    },
                );
                Err(err)
            }
        }
    }

    /// 发送一次 MutationSettled（同一 operation 恰好一次）并弹出队头。
    fn settle_mutation(&mut self, operation_id: u64, kind: MutationKind, result: MutationResult) {
        // Completed 即权威终态（§6.4）：钉住新 pane 焦点，防止 reader 晚到
        // 的旧快照把焦点回退到创建前（事件通道与 mutation 探针无序）。
        if result == MutationResult::Completed {
            if let Some(head) = self.mutation_queue.in_flight() {
                if let (Some(tab_id), Some(pane_id)) = (&head.expected_tab, &head.expected_pane) {
                    if let (Some(tab), Some(pane)) = (
                        self.herdr_tab_to_tab.get(tab_id).copied(),
                        self.herdr_pane_to_pane.get(pane_id).copied(),
                    ) {
                        self.focus_pin = Some(FocusPin {
                            tab,
                            pane,
                            until: Instant::now() + FOCUS_PIN_TTL,
                        });
                        self.snapshot_active.insert(tab, pane);
                    }
                }
            }
        }
        self.events.push_back(StateChange::MutationSettled {
            operation_id,
            kind,
            result,
        });
        self.mutation_queue.pop_head();
    }

    /// 用户显式焦点意图（切 pane/tab、焦点提升）：立即清除 settle 焦点钉，
    /// 新意图是权威，不能被钉住窗口挡住。
    fn clear_focus_pin(&mut self) {
        self.focus_pin = None;
    }

    /// 钉住窗口内，把焦点钉回 settled 的 pane（旧快照晚到不得回退）。
    fn enforce_focus_pin(&mut self) {
        let Some(pin) = self.focus_pin else {
            return;
        };
        if Instant::now() >= pin.until {
            self.focus_pin = None;
            return;
        }
        let pane_alive = self.panes.iter().any(|pane| pane.id == pin.pane);
        if !pane_alive {
            self.focus_pin = None;
            return;
        }
        if self.active_tab != Some(pin.tab) || self.active_pane != Some(pin.pane) {
            self.active_tab = Some(pin.tab);
            self.active_pane = Some(pin.pane);
            for pane in &mut self.panes {
                pane.active = pane.id == pin.pane;
            }
            if let Some(layout) = self.layouts.get_mut(&pin.tab) {
                layout.active = pin.pane;
            }
        }
    }

    /// 当前生效的焦点钉（未过期且 pane 仍在拓扑）；None = 无钉。
    fn pinned_focus(&self) -> Option<(TabId, PaneId)> {
        let pin = self.focus_pin?;
        if Instant::now() >= pin.until {
            return None;
        }
        if !self.panes.iter().any(|pane| pane.id == pin.pane) {
            return None;
        }
        Some((pin.tab, pin.pane))
    }

    /// Resolve the in-flight mutation's expected wire pane once a snapshot has
    /// installed its product mapping.  This is intentionally separate from
    /// `pinned_focus`: before the new pane appears, the focus pin cannot be
    /// considered alive, but the mutation intent still controls stream modes.
    fn pending_expected_focus(&self) -> Option<(TabId, PaneId)> {
        let expected = self
            .mutation_queue
            .in_flight()
            .and_then(|pending| pending.expected_focus.as_ref())?;
        let pane = self.herdr_pane_to_pane.get(expected).copied()?;
        let tab = self
            .panes
            .iter()
            .find(|candidate| candidate.id == pane)?
            .tab;
        Some((tab, pane))
    }

    /// 检查队头 mutation 是否已权威收敛（§6.4 完成条件）。
    fn mutation_converged(&self, pending: &PendingMutation) -> bool {
        let Some(expected_tab) = &pending.expected_tab else {
            return false;
        };
        let Some(expected_pane) = &pending.expected_pane else {
            return false;
        };
        // snapshot 已包含 created tab/root pane。
        if !self.herdr_tab_to_tab.contains_key(expected_tab) {
            return false;
        }
        if !self.herdr_pane_to_pane.contains_key(expected_pane) {
            return false;
        }
        // 新 pane 的 registry slot 必须 Live 且 full baseline Ready。
        let Some(pane) = self.herdr_pane_to_pane.get(expected_pane) else {
            return false;
        };
        let Some(slot) = self.stream_slots.get(pane) else {
            return false;
        };
        if slot.state != SlotState::Live || slot.surface_baseline != SurfaceBaseline::Ready {
            return false;
        }
        // Herdr active tab/focused pane 与产品 active 一致。
        let Some(tab) = self.herdr_tab_to_tab.get(expected_tab) else {
            return false;
        };
        if self.active_tab != Some(*tab) || self.active_pane != Some(*pane) {
            return false;
        }
        // layout 的 active 也一致。
        if self.layouts.get(tab).map(|l| l.active).unwrap_or(PaneId(0)) != *pane {
            return false;
        }
        true
    }

    /// 每 poll tick 驱动 mutation 队列：派发 → probe → 收敛 → settle。
    fn tick_mutations(&mut self, now: Instant) {
        // detach/shutdown 后：清空队列并丢弃晚到结果。
        if self.status != BackendStatus::Connected {
            if !self.mutation_queue.queue.is_empty() {
                let drained: Vec<PendingMutation> = self.mutation_queue.queue.drain(..).collect();
                for pending in drained {
                    self.events.push_back(StateChange::MutationSettled {
                        operation_id: pending.mutation_id,
                        kind: pending.kind,
                        result: MutationResult::Failed {
                            stage: MutationStage::Dispatch,
                            reason: "runtime 已 detach/shutdown".into(),
                        },
                    });
                }
            }
            return;
        }
        // 派发下一个等待项（同时至多一个 in-flight）。
        if !self.mutation_queue.has_in_flight() && self.mutation_queue.has_pending() {
            if let Err(err) = self.dispatch_next_mutation(now) {
                tracing::warn!(
                    target = "muxterm::herdr",
                    error = %err,
                    "mutation 派发失败（已 settle Failed）"
                );
            }
        }
        let Some(pending) = self.mutation_queue.in_flight().cloned() else {
            return;
        };
        let operation_id = pending.mutation_id;
        let kind = pending.kind;
        // 端到端 deadline 到期：唯一一次阶段化失败。
        if pending.expired(now) {
            self.settle_mutation(
                operation_id,
                kind,
                MutationResult::Failed {
                    stage: MutationStage::AuthorityConvergence,
                    reason: "5 秒内未权威收敛".into(),
                },
            );
            return;
        }
        // 权威收敛：Completed。
        if self.mutation_converged(&pending) {
            self.settle_mutation(operation_id, kind, MutationResult::Completed);
            return;
        }
        // probe：到点请求 snapshot refresh（至多一个 in-flight probe）。
        let probe_due = self.mutation_queue.in_flight().is_some_and(|head| {
            head.next_probe_at
                .is_some_and(|at| now >= at && !head.probe_in_flight(now))
        });
        if probe_due {
            if let Ok(snap) = self.session.snapshot() {
                self.reconcile_snapshot(&snap);
            }
            if let Some(head) = self.mutation_queue.head_mut() {
                // probe 序列耗尽（advance 返回 None）时清空 next_probe_at，
                // 之后只等 deadline 到期统一失败。
                if head.advance_probe(now).is_none() {
                    head.next_probe_at = None;
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
        // 先建 slot，再读取 attach seed，最后才启动异步 stream；这样
        // seed_one_pane 能标记 seed_pending，首个 full 不会抹掉历史 Index。
        self.initialize_stream_slots();
        self.seed_pane_read();
        self.reconcile_stream_modes();
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
                // PaneInfo 可能已随 Observe frame 对齐，但仍要写 preferred 并
                // 推 wire Resize：否则 early-return 会让 Control Hello 卡在 80×24。
                let first_preferred = self.preferred_client_size.is_none();
                match self.resize_control_stream(*target, cols, rows) {
                    Ok(()) => {}
                    Err(_) if !self.stream_slots.contains_key(target) => {
                        self.preferred_client_size = Some((cols, rows));
                        if first_preferred {
                            self.reconcile_stream_modes();
                        }
                    }
                    Err(err) => {
                        return Err(anyhow!("Herdr pane control resize 失败: {err}"));
                    }
                }
                let unchanged = self
                    .panes
                    .iter()
                    .find(|pane| pane.id == *target)
                    .is_some_and(|pane| pane.cols == cols && pane.rows == rows);
                if unchanged {
                    return Ok(TaskOutcome::Done);
                }
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
                // SwitchPane 已同步 pane.focus 到 server：本地焦点意图即权威。
                self.snapshot_active.insert(tab, *target);
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
                // 恢复目标必须以权威快照为准（snapshot_active），不能信任
                // layout.updated 事件流记录：事件与快照两条通道无序，晚到旧
                // 事件会把 focused_pane_id 回退到 split 之前（agent e2e）。
                if let Some(pane) = self.restore_active_for(*target) {
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
                // 异步 mutation：入队返回 Accepted，最终由 MutationSettled 收敛。
                let now = Instant::now();
                let operation_id = match self.mutation_queue.enqueue(MutationKind::NewTab, now) {
                    Ok(id) => id,
                    Err(_) => {
                        return Ok(TaskOutcome::Rejected {
                            reason: "mutation 队列满（32 项）".into(),
                        });
                    }
                };
                // 请求参数随 mutation 保存（派发时才真正发 socket）。
                // 按 id 定位：队头可能是仍在 in-flight 的旧 mutation。
                let pending = self
                    .mutation_queue
                    .by_id_mut(operation_id)
                    .expect("刚入队必须存在");
                pending.new_tab_name = name.clone();
                pending.expected_tab = None;
                pending.expected_pane = None;
                pending.expected_focus = None;
                tracing::info!(
                    target = "muxterm::herdr",
                    workspace = %self.workspace_id,
                    operation_id = operation_id,
                    "NewTab 入队（异步收敛）"
                );
                Ok(TaskOutcome::Accepted { operation_id })
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
                let now = Instant::now();
                let operation_id = match self.mutation_queue.enqueue(MutationKind::SplitPane, now) {
                    Ok(id) => id,
                    Err(_) => {
                        return Ok(TaskOutcome::Rejected {
                            reason: "mutation 队列满（32 项）".into(),
                        });
                    }
                };
                let pending = self
                    .mutation_queue
                    .by_id_mut(operation_id)
                    .expect("刚入队必须存在");
                pending.target_tab = Some(self.tab_to_herdr_tab[&tab].clone());
                pending.target_pane = Some(herdr_pane);
                pending.split_dir = Some(*dir);
                pending.expected_tab = None;
                pending.expected_pane = None;
                pending.expected_focus = None;
                tracing::info!(
                    target = "muxterm::herdr",
                    workspace = %self.workspace_id,
                    operation_id = operation_id,
                    "SplitPane 入队（异步收敛）"
                );
                Ok(TaskOutcome::Accepted { operation_id })
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
        self.tick_mutations(now);
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

impl HerdrRuntime {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::herdr::session::{
        HerdrAgentStatus, LayoutPaneRecord, LayoutSplitRecord, TabRecord, WorkspaceRecord,
    };
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

    /// Herdr 快照可能不返回 tab label（新建 tab 尚未命名时）。
    /// 此时 UI 必须回退到 public id 后缀的十进制形式（tA → "10"），
    /// 而不是显示空字符串或原始 bijective 编码。
    #[test]
    fn empty_tab_label_falls_back_to_decimal_suffix() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        let snap = SessionSnapshot {
            version: "0.8.0".into(),
            protocol: 19,
            focused_workspace_id: Some("w1".into()),
            focused_tab_id: Some("w1:tA".into()),
            focused_pane_id: Some("w1:p1".into()),
            workspaces: vec![WorkspaceRecord {
                workspace_id: "w1".into(),
                number: 1,
                label: "ws".into(),
                focused: true,
                pane_count: 1,
                tab_count: 2,
                active_tab_id: Some("w1:tA".into()),
                agent_status: HerdrAgentStatus::Idle,
                tokens: Default::default(),
                worktree: None,
            }],
            tabs: vec![
                TabRecord {
                    tab_id: "w1:tA".into(),
                    workspace_id: "w1".into(),
                    number: 1,
                    label: String::new(),
                    focused: true,
                    pane_count: 1,
                    agent_status: HerdrAgentStatus::Idle,
                },
                TabRecord {
                    tab_id: "w1:tB".into(),
                    workspace_id: "w1".into(),
                    number: 2,
                    label: "named".into(),
                    focused: false,
                    pane_count: 1,
                    agent_status: HerdrAgentStatus::Idle,
                },
            ],
            panes: vec![],
            layouts: vec![],
            agents: vec![],
        };
        assert!(runtime.apply_snapshot(&snap, true));
        let names: Vec<&str> = runtime.tabs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["10", "named"],
            "空 label 回退十进制后缀，非空 label 原样保留"
        );
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

    /// dogfood 0826：snapshot rect 76×12 不得变成 Hello；否则 htop Observe 锁死。
    #[test]
    fn hello_client_size_ignores_snapshot_split_rect() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w2",
        );
        let pane = PaneId(5);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "htop".into(),
            cols: 76,
            rows: 12,
        });
        runtime.stream_slots.insert(
            pane,
            PaneStreamSlot::new(pane, "w2:p5", StreamMode::Observe),
        );
        assert_eq!(
            runtime.hello_client_size(pane),
            (DEFAULT_HERDR_COLS, DEFAULT_HERDR_ROWS),
            "无 GTK preferred 时 Hello 必须是默认 viewport，不是 split rect"
        );
        runtime.preferred_client_size = Some((132, 41));
        assert_eq!(runtime.hello_client_size(pane), (132, 41));
        if let Some(slot) = runtime.stream_slots.get_mut(&pane) {
            slot.pending_resize = Some((140, 50));
        }
        assert_eq!(
            runtime.hello_client_size(pane),
            (140, 50),
            "pending_resize 优先于 preferred"
        );
    }

    /// dogfood 2030：GTK preferred 未到前不得 Control Hello（默认 80×24 SIGWINCH）。
    #[test]
    fn desired_mode_stays_observe_until_preferred_client_size() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w2",
        );
        runtime.foreground = true;
        let pane = PaneId(5);
        runtime.active_pane = Some(pane);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "htop".into(),
            cols: 76,
            rows: 12,
        });
        assert_eq!(
            runtime.desired_mode_for(&runtime.panes[0]),
            StreamMode::Observe,
            "无 preferred 时 active 也只能 Observe"
        );
        runtime.preferred_client_size = Some((132, 41));
        assert_eq!(
            runtime.desired_mode_for(&runtime.panes[0]),
            StreamMode::Control
        );
    }

    /// PaneInfo 已与 frame 对齐时 ResizePane 仍必须写入 preferred（否则卡 80×24）。
    #[test]
    fn resize_pane_records_preferred_even_when_paneinfo_unchanged() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w2",
        );
        let pane = PaneId(5);
        runtime.active_pane = Some(pane);
        runtime.foreground = true;
        runtime.status = BackendStatus::Connected;
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "htop".into(),
            cols: 132,
            rows: 41,
        });
        runtime.stream_slots.insert(
            pane,
            PaneStreamSlot::new(pane, "w2:p5", StreamMode::Observe),
        );
        assert!(runtime.preferred_client_size.is_none());
        let outcome = runtime
            .execute(&Task::ResizePane {
                target: pane,
                cols: 132,
                rows: 41,
            })
            .expect("resize");
        assert!(matches!(outcome, TaskOutcome::Done));
        assert_eq!(runtime.preferred_client_size, Some((132, 41)));
        assert_eq!(
            runtime.desired_mode_for(&runtime.panes[0]),
            StreamMode::Control,
            "preferred 写入后 active 应变 Control"
        );
    }

    /// 帧尺寸变化必须先发 PaneResized，再让 Surface 吃 PaneFrame。
    #[test]
    fn frame_size_change_emits_pane_resized_before_frame() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w2",
        );
        runtime.status = BackendStatus::Connected;
        let pane = PaneId(5);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "htop".into(),
            cols: 76,
            rows: 12,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.stream_tx = Some(tx);
        runtime.stream_rx = Some(rx);
        let mut slot = PaneStreamSlot::new(pane, "w2:p5", StreamMode::Observe);
        slot.generation = 1;
        slot.state = SlotState::Live;
        slot.actual_mode = Some(StreamMode::Observe);
        slot.surface_baseline = SurfaceBaseline::AwaitingFull;
        runtime.stream_slots.insert(pane, slot);

        runtime
            .stream_tx
            .as_ref()
            .unwrap()
            .send(PaneStreamEvent::Frame {
                pane,
                generation: 1,
                event_ordinal: 1,
                wire_seq: 1,
                bytes: b"HTOP".to_vec(),
                width: 132,
                height: 41,
                full: true,
            })
            .unwrap();
        runtime.drain_stream();

        let mut saw_resized = false;
        let mut saw_frame_after = false;
        for ev in &runtime.events {
            match ev {
                StateChange::PaneResized {
                    pane: p,
                    cols: 132,
                    rows: 41,
                } if *p == pane => {
                    saw_resized = true;
                }
                StateChange::PaneFrame { pane: p, .. } if *p == pane => {
                    assert!(saw_resized, "PaneResized 必须先于 PaneFrame");
                    saw_frame_after = true;
                }
                _ => {}
            }
        }
        assert!(saw_resized && saw_frame_after);
        assert_eq!(
            runtime
                .panes
                .iter()
                .find(|p| p.id == pane)
                .map(|p| (p.cols, p.rows)),
            Some((132, 41))
        );
    }

    #[test]
    fn observe_live_resize_updates_preferred_client_size() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w2",
        );
        let pane = PaneId(5);
        runtime.panes.push(PaneInfo {
            id: pane,
            tab: TabId(1),
            active: true,
            title: "htop".into(),
            cols: 76,
            rows: 12,
        });
        let mut slot = PaneStreamSlot::new(pane, "w2:p5", StreamMode::Observe);
        slot.generation = 1;
        slot.state = SlotState::Starting;
        slot.actual_mode = Some(StreamMode::Observe);
        runtime.stream_slots.insert(pane, slot);

        // Starting：latest-wins 暂存；不得因 snapshot 尺寸 early-return。
        runtime
            .resize_control_stream(pane, 132, 41)
            .expect("observe starting resize");
        assert_eq!(runtime.preferred_client_size, Some((132, 41)));
        assert_eq!(
            runtime
                .stream_slots
                .get(&pane)
                .and_then(|s| s.pending_resize),
            Some((132, 41))
        );
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

    #[test]
    fn pending_mutation_focus_wins_over_stale_active_pane_for_stream_mode() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        let tab = TabId(1);
        let old = PaneId(7);
        let created = PaneId(23);
        runtime.foreground = true;
        runtime.active_pane = Some(old);
        runtime.panes = vec![
            PaneInfo {
                id: old,
                tab,
                active: true,
                title: "old".into(),
                cols: 80,
                rows: 24,
            },
            PaneInfo {
                id: created,
                tab,
                active: false,
                title: "created".into(),
                cols: 80,
                rows: 24,
            },
        ];
        runtime.pane_to_herdr_pane.insert(old, "w1:p7".into());
        runtime.pane_to_herdr_pane.insert(created, "w1:pQ".into());
        let id = runtime
            .mutation_queue
            .enqueue(MutationKind::SplitPane, Instant::now())
            .expect("mutation 入队");
        let pending = runtime
            .mutation_queue
            .by_id_mut(id)
            .expect("pending mutation");
        pending.dispatched_at = Some(Instant::now());
        pending.expected_focus = Some("w1:pQ".into());

        assert_eq!(
            runtime.desired_mode_for(&runtime.panes[0]),
            StreamMode::Observe
        );
        assert_eq!(
            runtime.desired_mode_for(&runtime.panes[1]),
            StreamMode::Control
        );
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

    /// W9 回归：`layout.updated` 事件流记录可能晚到并携带旧 focused_pane_id
    /// （事件与快照两条通道无序）。切 tab 恢复焦点必须用权威快照值
    /// （snapshot_active），不能读被晚到旧事件污染的 layouts[tab].active。
    #[test]
    fn switch_tab_restores_authoritative_snapshot_active_not_stale_event() {
        let mut runtime = HerdrRuntime::new(
            Arc::new(HerdrSession::new("test", "/tmp/muxterm-no-socket")),
            "w1",
        );
        let tab1 = TabId(1);
        let tab2 = TabId(2);
        runtime.herdr_tab_to_tab.insert("w1:t1".into(), tab1);
        runtime.herdr_tab_to_tab.insert("w1:t2".into(), tab2);
        for (herdr, product) in [
            ("w1:p1", PaneId(1)),
            ("w1:p2", PaneId(2)),
            ("w1:p3", PaneId(3)),
            ("w1:p4", PaneId(4)),
        ] {
            runtime.herdr_pane_to_pane.insert(herdr.into(), product);
        }
        runtime.tabs = vec![
            TabInfo {
                id: tab1,
                name: "t1".into(),
                active: false,
            },
            TabInfo {
                id: tab2,
                name: "t2".into(),
                active: false,
            },
        ];
        runtime.panes = vec![
            PaneInfo {
                id: PaneId(1),
                tab: tab1,
                active: false,
                title: "p1".into(),
                cols: 80,
                rows: 24,
            },
            PaneInfo {
                id: PaneId(2),
                tab: tab2,
                active: false,
                title: "p2".into(),
                cols: 54,
                rows: 23,
            },
            PaneInfo {
                id: PaneId(3),
                tab: tab2,
                active: false,
                title: "p3".into(),
                cols: 54,
                rows: 11,
            },
            PaneInfo {
                id: PaneId(4),
                tab: tab2,
                active: false,
                title: "p4".into(),
                cols: 54,
                rows: 11,
            },
        ];
        let left = LayoutRect {
            x: 0,
            y: 0,
            width: 54,
            height: 23,
        };
        let right_top = LayoutRect {
            x: 54,
            y: 0,
            width: 54,
            height: 11,
        };
        let right_bottom = LayoutRect {
            x: 54,
            y: 12,
            width: 54,
            height: 11,
        };
        let root_rect = LayoutRect {
            x: 0,
            y: 0,
            width: 108,
            height: 23,
        };
        let record = |focused: &str| LayoutRecord {
            workspace_id: "w1".into(),
            tab_id: "w1:t2".into(),
            zoomed: false,
            area: root_rect,
            focused_pane_id: focused.into(),
            panes: vec![
                LayoutPaneRecord {
                    pane_id: "w1:p2".into(),
                    focused: false,
                    rect: left,
                },
                LayoutPaneRecord {
                    pane_id: "w1:p3".into(),
                    focused: false,
                    rect: right_top,
                },
                LayoutPaneRecord {
                    pane_id: "w1:p4".into(),
                    focused: true,
                    rect: right_bottom,
                },
            ],
            splits: vec![
                LayoutSplitRecord {
                    id: "split_root".into(),
                    path: vec![],
                    direction: LayoutSplitDirection::Right,
                    ratio: 0.5,
                    rect: root_rect,
                },
                LayoutSplitRecord {
                    id: "split_right".into(),
                    path: vec![true],
                    direction: LayoutSplitDirection::Down,
                    ratio: 0.5,
                    rect: root_rect,
                },
            ],
        };

        // 快照路径：权威 focused = p4。
        assert!(runtime.apply_layout_record(&record("w1:p4"), false));
        assert_eq!(
            runtime.snapshot_active.get(&tab2).copied(),
            Some(PaneId(4)),
            "快照路径必须记录权威 active"
        );
        assert_eq!(runtime.layouts.get(&tab2).unwrap().active, PaneId(4));

        // 晚到旧事件：focused 回退到 p2（split 之前的值）。
        assert!(runtime.apply_layout_record(&record("w1:p2"), true));
        assert_eq!(
            runtime.layouts.get(&tab2).unwrap().active,
            PaneId(2),
            "事件路径可更新 UI 布局 active（本身不报错）"
        );
        assert_eq!(
            runtime.snapshot_active.get(&tab2).copied(),
            Some(PaneId(4)),
            "晚到事件不得污染权威 active"
        );
        assert_eq!(
            runtime.restore_active_for(tab2),
            Some(PaneId(4)),
            "切 tab 恢复必须用权威快照值，而不是被旧事件回退的 layout.active"
        );
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

    /// attach 的 `pane.read` 快照属于 Index 面；首个 observer full 只初始化
    /// Surface，不能把历史 Index 种子抹掉。full 之后必须重新播种一次，确保
    /// Workspace 最终保留 attach 前已经存在的内容。
    #[test]
    fn attach_seed_is_replayed_after_first_full_frame() {
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
        slot.surface_baseline = SurfaceBaseline::AwaitingFull;
        slot.seed_pending = true;
        runtime.stream_slots.insert(pane, slot);
        runtime.outputs.insert(pane, b"ATTACH_HISTORY".to_vec());

        tx.send(PaneStreamEvent::Frame {
            pane,
            generation: 1,
            event_ordinal: 1,
            wire_seq: 1,
            bytes: b"CURRENT_SCREEN".to_vec(),
            width: 80,
            height: 24,
            full: true,
        })
        .unwrap();
        runtime.drain_stream();

        assert_eq!(
            runtime.outputs.get(&pane).map(Vec::as_slice),
            Some(b"ATTACH_HISTORY".as_slice()),
            "首个 full 不得覆盖 attach 的历史 Index 快照"
        );
        assert!(!runtime.stream_slots.get(&pane).unwrap().seed_pending);
        let events = runtime
            .events
            .iter()
            .filter_map(|event| match event {
                StateChange::PaneFrame { pane: p, data } if *p == pane => {
                    Some(("frame", data.as_slice()))
                }
                StateChange::PaneIndexSnapshot { pane: p, data } if *p == pane => {
                    Some(("index", data.as_slice()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            vec![
                ("frame", b"CURRENT_SCREEN".as_slice()),
                ("index", b"ATTACH_HISTORY".as_slice()),
            ],
            "首个 full 后必须按 PaneFrame→PaneIndexSnapshot 顺序恢复 attach 快照"
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
            !runtime.outputs.contains_key(&pane),
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
        runtime.stream_slots.insert(
            pane,
            PaneStreamSlot::new(pane, "w1:p1", StreamMode::Observe),
        );
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
        assert!(
            runtime.stream_slots.get(&pane).unwrap().seed_pending,
            "attach seed 必须在 stream slot 建立后标记 seed_pending"
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
