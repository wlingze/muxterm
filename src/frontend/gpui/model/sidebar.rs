//! 侧栏 / 快速面板 / Tab 活动共用的工作区与 Agent 投影。
//!
//! 移植自 `chrome/WorkspaceSidebarModel.swift`。
//!
//! 规则要点：
//! - Agents 只投影 Core/runtime 已标记的数据，前端不按 processName 猜身份；
//! - 固定聚合槽（Shells / Agents）与正在打开的工作区不进入活动聚合；
//! - 非 `connected` 的工作区仍留在工作区列表，但不贡献活动行；
//! - `paneId` 跨工作区会重复，所以所有 key 必须带 workspaceId。

use std::collections::{HashMap, HashSet};

use super::attention::{
    AttentionAlert, AttentionSnapshot, AttentionVisibilityKey, PaneAttention, PaneAttentionStatus,
};

/// 固定聚合槽的 workspace id。它们永远不会变成 Core Workspace。
pub const AGGREGATE_SHELLS: &str = "muxterm.frontend.aggregate.shells";
/// 固定聚合槽：Agents。
pub const AGGREGATE_AGENTS: &str = "muxterm.frontend.aggregate.agents";

/// 后端连接状态，常量与 `muxterm.h` 一致。
pub mod backend_status {
    pub const DISCONNECTED: u32 = 0;
    pub const CONNECTING: u32 = 1;
    pub const CONNECTED: u32 = 2;
    pub const ERROR: u32 = 3;
    pub const EXITED: u32 = 4;

    /// 中文注释：状态文字只用于展示。
    pub fn label(status: u32) -> &'static str {
        match status {
            DISCONNECTED => "disconnected",
            CONNECTING => "connecting",
            CONNECTED => "connected",
            ERROR => "error",
            EXITED => "exited",
            _ => "unknown",
        }
    }

    /// 只有已连接的工作区贡献活动聚合。
    pub fn includes_activity(status: u32) -> bool {
        status == CONNECTED
    }
}

/// 工作区快捷键编号：只有前 9 个有快捷键，重复 id 首次出现者胜出。
pub const MAXIMUM_SHORTCUT: usize = 9;

/// 按打开顺序生成 1-based 编号。
pub fn shortcut_index_by_workspace_id(ordered_ids: &[&str]) -> HashMap<String, u32> {
    let mut result = HashMap::new();
    for (offset, id) in ordered_ids.iter().take(MAXIMUM_SHORTCUT).enumerate() {
        result.entry((*id).to_string()).or_insert(offset as u32 + 1);
    }
    result
}

/// 机器名与工作区名一起展示，不能只藏在 tooltip 里。
pub fn display_title(name: &str, transport: &str) -> String {
    let machine = transport.trim();
    if machine.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {machine}")
    }
}

/// Agents / Commands / Attention 共用的四态标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Indicator {
    /// 排序/亮度最低。
    Idle,
    Working,
    Blocked,
    Done,
}

impl Indicator {
    /// 圆点标记：idle 空心，其余实心。
    pub fn marker(self) -> &'static str {
        if self == Indicator::Idle {
            "○"
        } else {
            "●"
        }
    }

    /// 状态成块的排序权重：done 最先，idle 最后。
    pub fn activity_rank(self) -> u8 {
        match self {
            Indicator::Done => 0,
            Indicator::Blocked => 1,
            Indicator::Working => 2,
            Indicator::Idle => 3,
        }
    }

    /// 中文注释：展示用的状态词。
    pub fn status_label(self) -> &'static str {
        match self {
            Indicator::Done => "done",
            Indicator::Blocked => "blocked",
            Indicator::Working => "working",
            Indicator::Idle => "idle",
        }
    }
}

/// Core 发出的 runtime-neutral agent 生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    Unknown,
}

impl AgentStatus {
    /// 中文注释：展示用的状态词；unknown 显示为 idle。
    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Idle | AgentStatus::Unknown => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
        }
    }
}

/// 单个 pane 的 agent 快照。runtime 私有协议字段不会到达这一层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAgent {
    pub pane_id: u32,
    pub display_name: Option<String>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub status: AgentStatus,
    pub state_change_seq: u64,
    pub revision: u64,
}

impl PaneAgent {
    /// 只填必需字段的构造器，测试用。
    pub fn new(pane_id: u32, status: AgentStatus) -> Self {
        Self {
            pane_id,
            display_name: None,
            title: None,
            name: None,
            kind: None,
            status,
            state_change_seq: 0,
            revision: 0,
        }
    }

    fn with_status(&self, status: AgentStatus) -> Self {
        let mut next = self.clone();
        next.status = status;
        next
    }
}

/// 版本向量：状态变更序号优先，其次修订号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentVersion {
    state_change_seq: u64,
    revision: u64,
}

impl AgentVersion {
    fn is_known(self) -> bool {
        self.state_change_seq != 0 || self.revision != 0
    }

    /// 新版本是否可覆盖当前版本。
    ///
    /// - 当前版本未知：一律接受（旧 Core 不报版本号）；
    /// - 新版本未知、当前已知：拒绝，防止旧数据回滚；
    /// - 否则按序号优先、修订号兜底比较。
    fn accepts(self, current: AgentVersion) -> bool {
        if !current.is_known() {
            return true;
        }
        if !self.is_known() {
            return false;
        }
        self.state_change_seq > current.state_change_seq
            || (self.state_change_seq == current.state_change_seq
                && self.revision >= current.revision)
    }
}

/// 缓存 key：工作区 + pane。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentKey {
    pub workspace_id: String,
    pub pane_id: u32,
}

/// pane 级 agent 身份缓存。
///
/// Runtime 快照可能在 agent 退出后不再上报它，但 Agents 侧栏要保留最后的
/// 身份直到 pane 本身关闭；此时状态标为 `unknown`，由 attention 快照在可用
/// 时补充显示状态。
#[derive(Debug, Default)]
pub struct AgentRegistry {
    agents: HashMap<AgentKey, PaneAgent>,
    versions: HashMap<AgentKey, AgentVersion>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 观察一次快照。`None` 表示 runtime 暂时没有该 pane 的 agent 权威信息。
    pub fn observe(&mut self, workspace_id: &str, pane_id: u32, agent: Option<PaneAgent>) {
        let key = AgentKey {
            workspace_id: workspace_id.to_string(),
            pane_id,
        };
        match agent {
            Some(agent) => {
                let incoming = AgentVersion {
                    state_change_seq: agent.state_change_seq,
                    revision: agent.revision,
                };
                let current = self.versions.get(&key).copied().or_else(|| {
                    self.agents.get(&key).map(|prev| AgentVersion {
                        state_change_seq: prev.state_change_seq,
                        revision: prev.revision,
                    })
                });
                if let Some(current) = current {
                    if !incoming.accepts(current) {
                        return;
                    }
                }
                if incoming.is_known() {
                    self.versions.insert(key.clone(), incoming);
                }
                self.agents.insert(key, agent);
            }
            None => {
                // 不能猜成 idle：保留 identity，状态降为 unknown。
                if let Some(previous) = self.agents.get(&key) {
                    let next = previous.with_status(AgentStatus::Unknown);
                    self.agents.insert(key, next);
                }
            }
        }
    }

    /// pane 关闭时删除身份。
    pub fn remove_pane(&mut self, workspace_id: &str, pane_id: u32) {
        let key = AgentKey {
            workspace_id: workspace_id.to_string(),
            pane_id,
        };
        self.agents.remove(&key);
        self.versions.remove(&key);
    }

    /// 按工作区取快照，按 pane id 排序。侧栏必须用这个。
    pub fn snapshot(&self, workspace_id: &str) -> Vec<PaneAgent> {
        let mut out: Vec<PaneAgent> = self
            .agents
            .iter()
            .filter(|(key, _)| key.workspace_id == workspace_id)
            .map(|(_, agent)| agent.clone())
            .collect();
        out.sort_by_key(|agent| agent.pane_id);
        out
    }

    /// 全量快照仅用于诊断。
    pub fn all(&self) -> Vec<PaneAgent> {
        let mut out: Vec<PaneAgent> = self.agents.values().cloned().collect();
        out.sort_by(|a, b| {
            a.pane_id.cmp(&b.pane_id).then_with(|| {
                a.name
                    .clone()
                    .unwrap_or_default()
                    .cmp(&b.name.clone().unwrap_or_default())
            })
        });
        out
    }
}

/// 侧栏与快速面板共用的 Runtime 更新按钮状态。
///
/// Swift 侧直接存 SF Symbol 名；gpui 没有 SF Symbols，所以这里保留语义枚举，
/// 原始符号名只在 parity 测试里核对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSymbol {
    Download,
    Retry,
    Success,
}

impl UpdateSymbol {
    /// macOS SF Symbol 名，用作 parity 证据。
    pub fn sf_symbol(self) -> &'static str {
        match self {
            UpdateSymbol::Download => "arrow.down.circle",
            UpdateSymbol::Retry => "exclamationmark.arrow.circlepath",
            UpdateSymbol::Success => "checkmark.circle",
        }
    }
}

/// 按钮色调。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateTint {
    Secondary,
    Orange,
    Green,
}

/// 已解析的更新按钮状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateButtonState {
    pub visible: bool,
    pub enabled: bool,
    pub busy: bool,
    pub symbol: UpdateSymbol,
    pub tint: UpdateTint,
    pub accessibility_label: String,
    pub tooltip: String,
}

impl UpdateButtonState {
    /// 成功态不能继续显示成可点击的下载箭头。
    pub fn resolve(can_update: bool, phase: Option<&str>, message: Option<&str>) -> Self {
        let phase = phase.unwrap_or("idle");
        let busy = matches!(
            phase,
            "checking" | "installing" | "handoff" | "reconnecting"
        );
        let failed = phase == "failed";
        let succeeded = phase == "succeeded";
        let symbol = if failed {
            UpdateSymbol::Retry
        } else if succeeded {
            UpdateSymbol::Success
        } else {
            UpdateSymbol::Download
        };
        let tint = if failed {
            UpdateTint::Orange
        } else if succeeded {
            UpdateTint::Green
        } else {
            UpdateTint::Secondary
        };
        let label = if succeeded {
            "Runtime is up to date"
        } else {
            "Update runtime and reconnect"
        };
        let tooltip = if succeeded {
            "Runtime is up to date".to_string()
        } else if let Some(message) = message.filter(|m| !m.is_empty()) {
            message.to_string()
        } else {
            "Update runtime and reconnect".to_string()
        };
        Self {
            visible: can_update,
            enabled: can_update && !busy && !succeeded,
            busy: can_update && busy,
            symbol,
            tint,
            accessibility_label: if can_update { label } else { "" }.to_string(),
            tooltip,
        }
    }
}

/// 主窗口侧栏里的一行工作区。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub workspace_id: String,
    pub name: String,
    pub runtime: String,
    pub transport: String,
    pub is_active: bool,
    pub backend_status: u32,
    pub shortcut: Option<u32>,
    /// 固定聚合槽不能关闭，也不参与真实工作区的拖动改序。
    pub is_closable: bool,
    pub is_reorderable: bool,
    /// 纯前端打开态投影，永远不会变成 Core 工作区。
    pub opening_stage: Option<String>,
    pub opening_target_id: Option<String>,
    pub can_update_runtime: bool,
    pub runtime_update_phase: Option<String>,
    pub runtime_update_message: Option<String>,
    pub structured_agents: Vec<PaneAgent>,
    /// 以当前 tab 排序生成的 1-based 编号；pane id 只用于内部跳转。
    pub tab_number_by_pane: HashMap<u32, u32>,
    /// pane 所属的稳定 tab id，侧栏点击时直接复用。
    pub tab_id_by_pane: HashMap<u32, u32>,
}

impl SidebarItem {
    /// 只填必需字段的构造器。
    pub fn new(workspace_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            name: name.into(),
            runtime: String::new(),
            transport: String::new(),
            is_active: false,
            backend_status: backend_status::CONNECTED,
            shortcut: None,
            is_closable: true,
            is_reorderable: true,
            opening_stage: None,
            opening_target_id: None,
            can_update_runtime: false,
            runtime_update_phase: None,
            runtime_update_message: None,
            structured_agents: Vec::new(),
            tab_number_by_pane: HashMap::new(),
            tab_id_by_pane: HashMap::new(),
        }
    }

    /// 中文注释：链式构造器，测试里读起来更接近 Swift 的命名参数。
    pub fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    /// 机器名与工作区名一起展示。
    pub fn display_title(&self) -> String {
        display_title(&self.name, &self.transport)
    }

    /// 工作区列表与快速面板共用的切换标识。
    ///
    /// 固定聚合槽用字母地标，真实工作区才用数字编号。
    pub fn shortcut_text(&self) -> Option<String> {
        match self.workspace_id.as_str() {
            AGGREGATE_SHELLS => Some("S".to_string()),
            AGGREGATE_AGENTS => Some("A".to_string()),
            _ => self.shortcut.map(|n| n.to_string()),
        }
    }

    /// 中文注释：是否为固定聚合槽。
    pub fn is_aggregate(&self) -> bool {
        self.workspace_id == AGGREGATE_SHELLS || self.workspace_id == AGGREGATE_AGENTS
    }

    /// 中文注释：是否正在打开。
    pub fn is_opening(&self) -> bool {
        self.opening_stage.is_some()
    }

    /// 中文注释：快捷键提示文本。
    pub fn keyboard_shortcut_text(&self) -> Option<String> {
        match self.workspace_id.as_str() {
            AGGREGATE_SHELLS => Some("Cmd-Ctrl-S".to_string()),
            AGGREGATE_AGENTS => Some("Cmd-Ctrl-A".to_string()),
            _ => self.shortcut.map(|n| format!("Cmd-Ctrl-{n}")),
        }
    }

    /// 该行是否参与活动聚合（Agents / Commands / Attention / Tab 活动）。
    fn contributes_activity(&self) -> bool {
        !self.is_aggregate()
            && !self.is_opening()
            && backend_status::includes_activity(self.backend_status)
    }
}

/// 侧栏 Agents 区的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentItem {
    pub workspace_id: String,
    pub tab_id: Option<u32>,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: Indicator,
    /// 排序与第二行展示用的名字。
    pub agent_name: String,
    /// Runtime 提供的 agent/session 标题；与 agent_name 分开，供聚合 tab 展示。
    pub session_title: Option<String>,
    pub tab_number: Option<u32>,
}

/// 侧栏 Commands 区的一行：运行中或未读的普通命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandItem {
    pub workspace_id: String,
    pub tab_id: Option<u32>,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: Indicator,
}

/// 投影到真实 Runtime tab 上的活动身份。
///
/// tab id 只在单个工作区内唯一，所以调用方必须同时保留两个字段。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TabActivityKey {
    pub workspace_id: String,
    pub tab_id: u32,
}

/// pane key：所有投影共用，必须带 workspace。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PaneKey {
    workspace_id: String,
    pane_id: u32,
}

/// 侧栏跳转：pane id 跨工作区会重复，fallback 必须限定目标工作区。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenePaneIndex {
    pub workspace_ids: HashSet<String>,
    pub pane_ids: HashSet<u32>,
    pub is_open: bool,
}

/// replica id 暂时对不上时，只在 pane 确实属于目标工作区时允许直接跳。
pub fn target_scene_contains_pane(
    workspace_id: &str,
    pane_id: u32,
    scenes: &[ScenePaneIndex],
) -> Option<ScenePaneIndex> {
    scenes
        .iter()
        .find(|scene| {
            scene.is_open
                && scene.workspace_ids.iter().any(|id| id == workspace_id)
                && scene.pane_ids.contains(&pane_id)
        })
        .cloned()
}

/// 取第一个非空（trim 后）字符串。
fn first_nonempty(values: &[Option<&str>]) -> Option<String> {
    values.iter().find_map(|value| {
        let trimmed = (*value)?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// 非空进程名。
fn non_empty_process_name(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `status · agentName · Tab N`。
fn agent_detail(status: &str, agent_name: &str, tab_number: Option<u32>) -> String {
    let mut values = vec![status.to_string(), agent_name.to_string()];
    if let Some(tab) = tab_number.filter(|n| *n > 0) {
        values.push(format!("Tab {tab}"));
    }
    values.join(" · ")
}

/// structured agent 的状态词：attention 优先，否则用 runtime 状态。
fn status_label_for_agent(status: AgentStatus, attention: Option<&PaneAttention>) -> &'static str {
    match attention {
        Some(attention) => Indicator::from_attention_status(attention.status).status_label(),
        None => status.label(),
    }
}

/// structured agent 的指示标记：attention 优先。
fn indicator_for_agent(status: AgentStatus, attention: Option<&PaneAttention>) -> Indicator {
    match attention {
        Some(attention) => indicator_for_attention(attention),
        None => match status {
            AgentStatus::Working => Indicator::Working,
            AgentStatus::Blocked => Indicator::Blocked,
            AgentStatus::Done => Indicator::Done,
            AgentStatus::Idle | AgentStatus::Unknown => Indicator::Idle,
        },
    }
}

/// attention 指示标记：已读的 done 降为 idle。
fn indicator_for_attention(attention: &PaneAttention) -> Indicator {
    match attention.status {
        PaneAttentionStatus::Working => Indicator::Working,
        PaneAttentionStatus::Blocked => Indicator::Blocked,
        PaneAttentionStatus::Done => {
            if attention.acknowledged {
                Indicator::Idle
            } else {
                Indicator::Done
            }
        }
        PaneAttentionStatus::Idle | PaneAttentionStatus::Unknown => Indicator::Idle,
    }
}

impl Indicator {
    /// 把 attention 状态映射成四态标记（不改已读语义）。
    fn from_attention_status(status: PaneAttentionStatus) -> Self {
        match status {
            PaneAttentionStatus::Working => Indicator::Working,
            PaneAttentionStatus::Blocked => Indicator::Blocked,
            PaneAttentionStatus::Done => Indicator::Done,
            PaneAttentionStatus::Idle | PaneAttentionStatus::Unknown => Indicator::Idle,
        }
    }
}

/// 把 attention 快照展开成 (工作区, pane) → 条目。
fn attention_by_pane(attention: Option<&AttentionSnapshot>) -> HashMap<PaneKey, PaneAttention> {
    let mut map = HashMap::new();
    for workspace in attention
        .map(|a| a.workspaces.as_slice())
        .unwrap_or_default()
    {
        for pane in &workspace.panes {
            map.insert(
                PaneKey {
                    workspace_id: workspace.workspace_id.clone(),
                    pane_id: pane.pane_id,
                },
                pane.clone(),
            );
        }
    }
    map
}

/// 投影 Agents 区。
///
/// 只投影 Core/runtime 已标记的数据：structured agent，以及 attention 里
/// `process_is_agent` / 已知 agent 可执行文件的 pane。前端不按 processName 猜。
pub fn agents(workspaces: &[SidebarItem], attention: Option<&AttentionSnapshot>) -> Vec<AgentItem> {
    let by_pane = attention_by_pane(attention);
    let mut result: Vec<AgentItem> = Vec::new();

    for workspace in workspaces.iter().filter(|w| w.contributes_activity()) {
        let mut structured_pane_ids: HashSet<u32> = HashSet::new();

        for agent in &workspace.structured_agents {
            structured_pane_ids.insert(agent.pane_id);
            let key = PaneKey {
                workspace_id: workspace.workspace_id.clone(),
                pane_id: agent.pane_id,
            };
            let pane_attention = by_pane.get(&key);
            let agent_name = first_nonempty(&[
                agent.display_name.as_deref(),
                agent.name.as_deref(),
                agent.kind.as_deref(),
                agent.title.as_deref(),
            ])
            .unwrap_or_else(|| "Agent".to_string());
            let tab_number = workspace.tab_number_by_pane.get(&agent.pane_id).copied();
            let tab_id = workspace.tab_id_by_pane.get(&agent.pane_id).copied();
            result.push(AgentItem {
                workspace_id: workspace.workspace_id.clone(),
                tab_id,
                pane_id: agent.pane_id,
                title: workspace.display_title(),
                detail: agent_detail(
                    status_label_for_agent(agent.status, pane_attention),
                    &agent_name,
                    tab_number,
                ),
                indicator: indicator_for_agent(agent.status, pane_attention),
                agent_name,
                session_title: agent.title.clone(),
                tab_number,
            });
        }

        let generic = attention
            .and_then(|a| {
                a.workspaces
                    .iter()
                    .find(|w| w.workspace_id == workspace.workspace_id)
            })
            .map(|w| w.panes.as_slice())
            .unwrap_or_default();

        for pane in generic
            .iter()
            .filter(|p| !structured_pane_ids.contains(&p.pane_id))
        {
            if !AttentionAlert::is_agent(pane) {
                continue;
            }
            let name = AttentionAlert::agent_name(pane)
                .or_else(|| first_nonempty(&[pane.process_name.as_deref()]))
                .unwrap_or_else(|| "Agent".to_string());
            let tab_number = workspace.tab_number_by_pane.get(&pane.pane_id).copied();
            let tab_id = workspace.tab_id_by_pane.get(&pane.pane_id).copied();
            result.push(AgentItem {
                workspace_id: workspace.workspace_id.clone(),
                tab_id,
                pane_id: pane.pane_id,
                title: workspace.display_title(),
                detail: agent_detail(
                    Indicator::from_attention_status(pane.status).status_label(),
                    &name,
                    tab_number,
                ),
                indicator: indicator_for_attention(pane),
                agent_name: name,
                session_title: None,
                tab_number,
            });
        }
    }

    // 状态成块；块内按工作区快捷键 / 标题 / tab / pane 固定，只更新状态时
    // 条目在块间移动，块内相对顺序不变。
    let shortcuts: HashMap<&str, Option<u32>> = workspaces
        .iter()
        .map(|w| (w.workspace_id.as_str(), w.shortcut))
        .collect();
    let order = |a: Option<u32>, b: Option<u32>| a.unwrap_or(u32::MAX).cmp(&b.unwrap_or(u32::MAX));

    result.sort_by(|lhs, rhs| {
        lhs.indicator
            .activity_rank()
            .cmp(&rhs.indicator.activity_rank())
            .then_with(|| {
                order(
                    shortcuts.get(lhs.workspace_id.as_str()).copied().flatten(),
                    shortcuts.get(rhs.workspace_id.as_str()).copied().flatten(),
                )
            })
            .then_with(|| lhs.title.to_lowercase().cmp(&rhs.title.to_lowercase()))
            .then_with(|| order(lhs.tab_number, rhs.tab_number))
            .then_with(|| lhs.pane_id.cmp(&rhs.pane_id))
            .then_with(|| {
                lhs.agent_name
                    .to_lowercase()
                    .cmp(&rhs.agent_name.to_lowercase())
            })
    });
    result
}

/// 投影 Commands 区。
///
/// Agent（structured 或已知进程名）留在 Agents；普通命令只在运行中，或
/// blocked/done 未读时可见。
pub fn commands(
    workspaces: &[SidebarItem],
    attention: Option<&AttentionSnapshot>,
) -> Vec<CommandItem> {
    let active: Vec<&SidebarItem> = workspaces
        .iter()
        .filter(|w| w.contributes_activity())
        .collect();

    // Agents 已有的 pane 不再重复出现在 Commands。
    let mut agent_panes: HashSet<PaneKey> = HashSet::new();
    for workspace in &active {
        for agent in &workspace.structured_agents {
            agent_panes.insert(PaneKey {
                workspace_id: workspace.workspace_id.clone(),
                pane_id: agent.pane_id,
            });
        }
        if let Some(panes) = attention
            .and_then(|a| {
                a.workspaces
                    .iter()
                    .find(|w| w.workspace_id == workspace.workspace_id)
            })
            .map(|w| w.panes.as_slice())
        {
            for pane in panes.iter().filter(|p| AttentionAlert::is_agent(p)) {
                agent_panes.insert(PaneKey {
                    workspace_id: workspace.workspace_id.clone(),
                    pane_id: pane.pane_id,
                });
            }
        }
    }

    let mut result = Vec::new();
    for workspace in active {
        let Some(panes) = attention
            .and_then(|a| {
                a.workspaces
                    .iter()
                    .find(|w| w.workspace_id == workspace.workspace_id)
            })
            .map(|w| w.panes.as_slice())
        else {
            continue;
        };
        for pane in panes {
            if !AttentionAlert::includes(pane) {
                continue;
            }
            let key = PaneKey {
                workspace_id: workspace.workspace_id.clone(),
                pane_id: pane.pane_id,
            };
            if agent_panes.contains(&key) {
                continue;
            }
            let Some(title) = non_empty_process_name(pane.process_name.as_deref()) else {
                continue;
            };
            let tab_number = workspace.tab_number_by_pane.get(&pane.pane_id).copied();
            let tab_id = workspace.tab_id_by_pane.get(&pane.pane_id).copied();
            let detail = agent_detail(
                Indicator::from_attention_status(pane.status).status_label(),
                &title,
                tab_number,
            );
            match pane.status {
                PaneAttentionStatus::Working => result.push(CommandItem {
                    workspace_id: workspace.workspace_id.clone(),
                    tab_id,
                    pane_id: pane.pane_id,
                    title: workspace.display_title(),
                    detail,
                    indicator: Indicator::Working,
                }),
                PaneAttentionStatus::Blocked | PaneAttentionStatus::Done => {
                    if pane.acknowledged {
                        continue;
                    }
                    result.push(CommandItem {
                        workspace_id: workspace.workspace_id.clone(),
                        tab_id,
                        pane_id: pane.pane_id,
                        title: workspace.display_title(),
                        detail,
                        indicator: if pane.status == PaneAttentionStatus::Blocked {
                            Indicator::Blocked
                        } else {
                            Indicator::Done
                        },
                    });
                }
                PaneAttentionStatus::Unknown | PaneAttentionStatus::Idle => continue,
            }
        }
    }
    result
}

/// 把 Agents/Commands 活动投影到 tab 上。
///
/// idle 不点亮 tab；同一个 tab 多个 pane 时按 done > blocked > working > idle
/// 的既有优先级取最高者。`hidden` 只作用于 Attention 的隐藏身份。
pub fn tab_activities(
    workspaces: &[SidebarItem],
    attention: Option<&AttentionSnapshot>,
    hidden: &HashSet<AttentionVisibilityKey>,
) -> HashMap<TabActivityKey, Indicator> {
    let mut result: HashMap<TabActivityKey, Indicator> = HashMap::new();

    let mut insert = |workspace_id: &str,
                      tab_id: Option<u32>,
                      pane_id: u32,
                      indicator: Indicator,
                      respects_hidden: bool| {
        let Some(tab_id) = tab_id else { return };
        if indicator == Indicator::Idle {
            return;
        }
        if respects_hidden
            && hidden.contains(&AttentionVisibilityKey {
                workspace_id: workspace_id.to_string(),
                pane_id,
            })
        {
            return;
        }
        let key = TabActivityKey {
            workspace_id: workspace_id.to_string(),
            tab_id,
        };
        if let Some(current) = result.get(&key) {
            if current.activity_rank() <= indicator.activity_rank() {
                return;
            }
        }
        result.insert(key, indicator);
    };

    for item in agents(workspaces, attention) {
        // Agents 常驻全部身份，但 idle 不点亮 tab；隐藏只作用于 Attention。
        insert(
            &item.workspace_id,
            item.tab_id,
            item.pane_id,
            item.indicator,
            false,
        );
    }
    for item in commands(workspaces, attention) {
        insert(
            &item.workspace_id,
            item.tab_id,
            item.pane_id,
            item.indicator,
            true,
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::attention::{
        attention_rows, AttentionSnapshot, PaneAttention, PaneAttentionStatus, WorkspaceAttention,
    };

    /// 构造一个 attention pane。
    fn pane(pane_id: u32, status: PaneAttentionStatus) -> PaneAttention {
        PaneAttention {
            pane_id,
            status,
            ..PaneAttention::default()
        }
    }

    /// 单个工作区的注意力快照。
    fn snapshot_for(workspace: &SidebarItem, panes: Vec<PaneAttention>) -> AttentionSnapshot {
        AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![WorkspaceAttention {
                workspace_id: workspace.workspace_id.clone(),
                name: workspace.name.clone(),
                transport: workspace.transport.clone(),
                ..WorkspaceAttention::new(workspace.workspace_id.clone(), panes)
            }],
        }
    }

    /// 构造一个 tmux 工作区，便于复用。
    fn tmux_workspace(id: &str, name: &str, transport: &str) -> SidebarItem {
        SidebarItem::new(id, name).with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = transport.to_string();
            item.is_active = true;
        })
    }

    /// 对照 `testRuntimeUpdateSucceededStateIsDistinctFromTheDownloadAction`。
    #[test]
    fn runtime_update_succeeded_state_is_distinct_from_the_download_action() {
        let succeeded = UpdateButtonState::resolve(true, Some("succeeded"), Some("Updated"));
        assert!(succeeded.visible);
        assert!(!succeeded.enabled);
        assert!(!succeeded.busy);
        assert_eq!(succeeded.symbol.sf_symbol(), "checkmark.circle");
        assert_eq!(succeeded.tint, UpdateTint::Green);
        assert_eq!(succeeded.accessibility_label, "Runtime is up to date");

        let available = UpdateButtonState::resolve(true, Some("idle"), None);
        assert!(available.enabled);
        assert_eq!(available.symbol.sf_symbol(), "arrow.down.circle");
        assert_eq!(available.tint, UpdateTint::Secondary);

        let failed = UpdateButtonState::resolve(true, Some("failed"), Some("download failed"));
        assert!(failed.enabled);
        assert_eq!(
            failed.symbol.sf_symbol(),
            "exclamationmark.arrow.circlepath"
        );
        assert_eq!(failed.tint, UpdateTint::Orange);
        assert_eq!(failed.tooltip, "download failed");
    }

    /// 对照 `testAggregateWorkspaceRowsNeverEnterActivityProjections`。
    #[test]
    fn aggregate_workspace_rows_never_enter_activity_projections() {
        let aggregate = SidebarItem::new(AGGREGATE_AGENTS, "Agents").with(|item| {
            item.runtime = "aggregate".to_string();
            item.transport = "all".to_string();
            item.is_active = true;
            item.is_closable = false;
            item.is_reorderable = false;
        });
        let attention = AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![WorkspaceAttention {
                working: 1,
                ..WorkspaceAttention::new(
                    AGGREGATE_AGENTS,
                    vec![PaneAttention {
                        last_line: "frontend projection".to_string(),
                        seq: 1,
                        process_name: Some("codex".to_string()),
                        process_is_agent: true,
                        ..pane(1, PaneAttentionStatus::Working)
                    }],
                )
            }],
        };
        let workspaces = std::slice::from_ref(&aggregate);
        assert!(agents(workspaces, Some(&attention)).is_empty());
        assert!(commands(workspaces, Some(&attention)).is_empty());
        assert!(attention_rows(&attention, workspaces, "").is_empty());
    }

    /// 对照 `testDisconnectedWorkspaceRemainsAvailableButLeavesActivityAggregates`。
    #[test]
    fn disconnected_workspace_remains_available_but_leaves_activity_aggregates() {
        let disconnected =
            SidebarItem::new("ssh/ryzen/default/tmux//work/remote", "remote").with(|i| {
                i.runtime = "tmux".to_string();
                i.transport = "ryzen".to_string();
                i.is_active = true;
                i.backend_status = backend_status::EXITED;
            });
        let attention = snapshot_for(
            &disconnected,
            vec![PaneAttention {
                last_line: "stale agent".to_string(),
                seq: 1,
                process_name: Some("codex".to_string()),
                process_is_agent: true,
                ..pane(1, PaneAttentionStatus::Working)
            }],
        );
        let workspaces = std::slice::from_ref(&disconnected);
        assert_eq!(disconnected.name, "remote", "工作区列表仍显示断连项");
        assert_eq!(disconnected.backend_status, backend_status::EXITED);
        assert!(agents(workspaces, Some(&attention)).is_empty());
        assert!(commands(workspaces, Some(&attention)).is_empty());
        assert!(
            attention_rows(&attention, workspaces, "").is_empty(),
            "断连工作区的警报不能只留在 Attention"
        );
    }

    /// 对照 `testMachineIdentityMatchesAgentsCommandsAndSearchableAttention`。
    #[test]
    fn machine_identity_matches_agents_commands_and_searchable_attention() {
        let machines = ["local", "ryzen"];
        let workspaces: Vec<SidebarItem> = machines
            .iter()
            .map(|machine| {
                tmux_workspace(machine, "muxterm", machine).with(|item| {
                    item.is_active = false;
                    item.tab_number_by_pane.insert(1, 1);
                    item.tab_number_by_pane.insert(2, 2);
                })
            })
            .collect();
        let attention = AttentionSnapshot {
            blocked_count: 0,
            workspaces: machines
                .iter()
                .map(|machine| WorkspaceAttention {
                    name: "muxterm".to_string(),
                    transport: machine.to_string(),
                    working: 2,
                    ..WorkspaceAttention::new(
                        *machine,
                        vec![
                            PaneAttention {
                                seq: 1,
                                process_name: Some("codex".to_string()),
                                process_is_agent: true,
                                agent_name: Some("Codex".to_string()),
                                ..pane(1, PaneAttentionStatus::Working)
                            },
                            PaneAttention {
                                seq: 2,
                                process_name: Some("cargo test".to_string()),
                                ..pane(2, PaneAttentionStatus::Working)
                            },
                        ],
                    )
                })
                .collect(),
        };
        let expected = ["muxterm · local", "muxterm · ryzen"];

        let agent_items = agents(&workspaces, Some(&attention));
        for title in expected {
            assert!(
                agent_items.iter().any(|item| item.title == title),
                "Agents 缺少 {title}"
            );
        }

        let command_items = commands(&workspaces, Some(&attention));
        for title in expected {
            assert!(
                command_items.iter().any(|item| item.title == title),
                "Commands 缺少 {title}"
            );
        }
        for detail in &["working · cargo test · Tab 2"] {
            assert!(
                command_items.iter().any(|item| item.detail == *detail),
                "Commands 缺少 {detail}"
            );
        }

        for machine in machines {
            let rows = attention_rows(&attention, &workspaces, machine);
            assert_eq!(rows.len(), 2);
            let titles: HashSet<String> = rows.iter().map(|row| row.title()).collect();
            assert!(titles.contains(&format!("muxterm  cargo  {machine}  ~")));
            assert!(titles.contains(&format!("muxterm  codex  {machine}  ~")));
        }
    }

    /// 对照 `testWorkspaceShortcutIndexesUseOpenedOrderAndStopAtNine`。
    #[test]
    fn workspace_shortcut_indexes_use_opened_order_and_stop_at_nine() {
        let ids = [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ];
        let index = shortcut_index_by_workspace_id(&ids);
        assert_eq!(index.get("one"), Some(&1));
        assert_eq!(index.get("nine"), Some(&9));
        assert_eq!(index.get("ten"), None, "第十个不得占用快捷键");
    }

    /// 对照 `testStructuredAgentRegistryKeepsReadIdentityUntilPaneCloses`。
    #[test]
    fn structured_agent_registry_keeps_read_identity_until_pane_closes() {
        let workspace_id = "local@@agents@herdr@w2";
        let mut registry = AgentRegistry::new();
        registry.observe(
            workspace_id,
            4,
            Some(PaneAgent {
                display_name: Some("Codex".to_string()),
                title: Some("Review muxterm".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(4, AgentStatus::Working)
            }),
        );
        registry.observe(workspace_id, 4, None);
        let snapshot = registry.snapshot(workspace_id);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].display_name.as_deref(), Some("Codex"));
        assert_eq!(snapshot[0].status, AgentStatus::Unknown);

        let workspace = SidebarItem::new(workspace_id, "muxterm").with(|item| {
            item.runtime = "herdr".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.structured_agents = snapshot;
        });
        let projected = agents(std::slice::from_ref(&workspace), None);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].title, "muxterm · local");
        assert_eq!(projected[0].detail, "idle · Codex");
        assert_eq!(projected[0].indicator, Indicator::Idle);

        registry.remove_pane(workspace_id, 4);
        assert!(registry.snapshot(workspace_id).is_empty());
    }

    /// 对照 `testStructuredAgentRegistryRejectsOlderVersion`。
    #[test]
    fn structured_agent_registry_rejects_older_version() {
        let workspace_id = "local@@agents@herdr@w2";
        let mut registry = AgentRegistry::new();
        registry.observe(
            workspace_id,
            4,
            Some(PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                state_change_seq: 11,
                revision: 11,
                ..PaneAgent::new(4, AgentStatus::Working)
            }),
        );
        registry.observe(
            workspace_id,
            4,
            Some(PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Idle,
                state_change_seq: 10,
                revision: 99,
                ..PaneAgent::new(4, AgentStatus::Idle)
            }),
        );
        assert_eq!(
            registry.snapshot(workspace_id)[0].status,
            AgentStatus::Working,
            "序号更小的快照不得覆盖"
        );

        registry.observe(
            workspace_id,
            4,
            Some(PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Idle,
                state_change_seq: 12,
                revision: 1,
                ..PaneAgent::new(4, AgentStatus::Idle)
            }),
        );
        assert_eq!(registry.snapshot(workspace_id)[0].status, AgentStatus::Idle);
    }

    /// 对照 `testStructuredAgentRegistryIsolatesSamePaneIdAcrossWorkspaces`。
    #[test]
    fn structured_agent_registry_isolates_same_pane_id_across_workspaces() {
        let mut registry = AgentRegistry::new();
        registry.observe(
            "legion@local",
            1,
            Some(PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(1, AgentStatus::Working)
            }),
        );
        registry.observe("muxterm@local", 1, None);

        assert_eq!(
            registry
                .snapshot("legion@local")
                .iter()
                .map(|a| a.name.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["codex"]
        );
        assert!(
            registry.snapshot("muxterm@local").is_empty(),
            "未上报 agent 的工作区不得继承同 paneId 的身份"
        );

        let muxterm = SidebarItem::new("muxterm@local", "muxterm").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.tab_number_by_pane.insert(1, 1);
            item.tab_number_by_pane.insert(2, 2);
            item.tab_number_by_pane.insert(3, 3);
        });
        let attention = snapshot_for(
            &muxterm,
            vec![
                PaneAttention {
                    status: PaneAttentionStatus::Idle,
                    acknowledged: true,
                    seq: 1,
                    process_name: Some("zsh".to_string()),
                    ..pane(1, PaneAttentionStatus::Idle)
                },
                PaneAttention {
                    status: PaneAttentionStatus::Idle,
                    acknowledged: true,
                    seq: 2,
                    process_name: Some("zsh".to_string()),
                    ..pane(3, PaneAttentionStatus::Idle)
                },
            ],
        );
        assert!(
            agents(std::slice::from_ref(&muxterm), Some(&attention)).is_empty(),
            "空闲 Tab 不得凭跨工作区 paneId 碰撞冒出 Codex"
        );
    }

    /// 对照 `testUnknownStructuredStatusUsesAttentionIndicator`。
    #[test]
    fn unknown_structured_status_uses_attention_indicator() {
        let workspace = SidebarItem::new("local@@agents@herdr@w2", "muxterm").with(|item| {
            item.runtime = "herdr".to_string();
            item.transport = "local".to_string();
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Unknown,
                ..PaneAgent::new(4, AgentStatus::Unknown)
            }];
        });
        let attention = snapshot_for(
            &workspace,
            vec![PaneAttention {
                status: PaneAttentionStatus::Working,
                acknowledged: true,
                last_line: "running".to_string(),
                seq: 1,
                process_name: Some("codex".to_string()),
                ..pane(4, PaneAttentionStatus::Working)
            }],
        );
        let item = &agents(std::slice::from_ref(&workspace), Some(&attention))[0];
        assert_eq!(item.detail, "working · Codex");
        assert_eq!(item.indicator, Indicator::Working);
    }

    /// 对照 `testAgentStatusFollowsAttentionWhenStructuredStaysWorking`。
    #[test]
    fn agent_status_follows_attention_when_structured_stays_working() {
        let workspace = SidebarItem::new("local@@dev@tmux@dev", "muxterm").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(4, AgentStatus::Working)
            }];
        });
        let attention = snapshot_for(
            &workspace,
            vec![PaneAttention {
                seq: 2,
                process_name: Some("codex".to_string()),
                last_line: "complete".to_string(),
                ..pane(4, PaneAttentionStatus::Done)
            }],
        );
        let item = &agents(std::slice::from_ref(&workspace), Some(&attention))[0];
        assert_eq!(item.detail, "done · Codex");
        assert_eq!(item.indicator, Indicator::Done);
    }

    /// 对照 `testSidebarTargetsCarryStableTabIDs`。
    #[test]
    fn sidebar_targets_carry_stable_tab_ids() {
        let workspace = SidebarItem::new("local@@dev@tmux@dev", "dev").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(4, AgentStatus::Working)
            }];
            item.tab_number_by_pane.insert(4, 2);
            item.tab_number_by_pane.insert(8, 3);
            item.tab_id_by_pane.insert(4, 42);
            item.tab_id_by_pane.insert(8, 84);
        });
        let attention = snapshot_for(
            &workspace,
            vec![
                PaneAttention {
                    seq: 1,
                    process_name: Some("codex".to_string()),
                    last_line: "agent".to_string(),
                    ..pane(4, PaneAttentionStatus::Working)
                },
                PaneAttention {
                    seq: 2,
                    process_name: Some("cargo".to_string()),
                    last_line: "command".to_string(),
                    ..pane(8, PaneAttentionStatus::Working)
                },
            ],
        );
        let workspaces = std::slice::from_ref(&workspace);
        assert_eq!(
            agents(workspaces, Some(&attention))
                .iter()
                .map(|i| i.tab_id)
                .collect::<Vec<_>>(),
            vec![Some(42)]
        );
        assert_eq!(
            commands(workspaces, Some(&attention))
                .iter()
                .map(|i| i.tab_id)
                .collect::<Vec<_>>(),
            vec![Some(84)]
        );
    }

    /// 对照 `testProjectsStructuredHerdrAndTmuxAgentsAcrossWorkspaces`。
    #[test]
    fn projects_structured_herdr_and_tmux_agents_across_workspaces() {
        let herdr = SidebarItem::new("local@@agents@herdr@w2", "muxterm").with(|item| {
            item.runtime = "herdr".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.shortcut = Some(1);
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                title: Some("Review muxterm".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(4, AgentStatus::Working)
            }];
            item.tab_number_by_pane.insert(4, 2);
        });
        let tmux = SidebarItem::new("local@@dev@tmux@dev", "dev").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.shortcut = Some(2);
            item.tab_number_by_pane.insert(9, 1);
        });
        let attention = AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![
                WorkspaceAttention {
                    working: 1,
                    ..WorkspaceAttention::new(
                        "local@@agents@herdr@w2",
                        vec![PaneAttention {
                            status: PaneAttentionStatus::Working,
                            acknowledged: true,
                            last_line: "running".to_string(),
                            seq: 1,
                            process_name: Some("codex".to_string()),
                            ..pane(4, PaneAttentionStatus::Working)
                        }],
                    )
                },
                WorkspaceAttention {
                    working: 1,
                    ..WorkspaceAttention::new(
                        "local@@dev@tmux@dev",
                        vec![PaneAttention {
                            status: PaneAttentionStatus::Working,
                            acknowledged: true,
                            last_line: "running".to_string(),
                            seq: 2,
                            process_name: Some("pi".to_string()),
                            process_is_agent: true,
                            agent_name: Some("pi".to_string()),
                            ..pane(9, PaneAttentionStatus::Working)
                        }],
                    )
                },
            ],
        };
        let items = agents(&[herdr.clone(), tmux.clone()], Some(&attention));
        // 同状态块内按 workspace shortcut 固定。
        assert_eq!(
            items.iter().map(|i| i.title.clone()).collect::<Vec<_>>(),
            vec!["muxterm · local", "dev · local"]
        );
        assert_eq!(
            items.iter().map(|i| i.detail.clone()).collect::<Vec<_>>(),
            vec!["working · Codex · Tab 2", "working · pi · Tab 1"]
        );
        assert_eq!(
            items.iter().map(|i| i.indicator).collect::<Vec<_>>(),
            vec![Indicator::Working, Indicator::Working]
        );
        assert_eq!(
            items
                .iter()
                .map(|i| i.workspace_id.clone())
                .collect::<Vec<_>>(),
            vec![herdr.workspace_id, tmux.workspace_id]
        );
    }

    /// 对照 `testReadStructuredAgentLeavesAgentsAndAttentionTogether`。
    #[test]
    fn read_structured_agent_leaves_agents_and_attention_together() {
        let workspace_id = "local@@agents@herdr@w2";
        let workspace = SidebarItem::new(workspace_id, "muxterm").with(|item| {
            item.runtime = "herdr".to_string();
            item.transport = "local".to_string();
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Done,
                ..PaneAgent::new(4, AgentStatus::Done)
            }];
            item.tab_number_by_pane.insert(4, 2);
        });
        let attention = snapshot_for(
            &workspace,
            vec![PaneAttention {
                status: PaneAttentionStatus::Done,
                acknowledged: true,
                last_line: "complete".to_string(),
                seq: 3,
                process_name: Some("codex".to_string()),
                ..pane(4, PaneAttentionStatus::Done)
            }],
        );
        let workspaces = std::slice::from_ref(&workspace);
        let agent_items = agents(workspaces, Some(&attention));
        assert_eq!(
            agent_items.iter().map(|i| i.pane_id).collect::<Vec<_>>(),
            vec![4],
            "已读完成的 agent 仍必须留在 Agents"
        );
        assert_eq!(
            agent_items.iter().map(|i| i.indicator).collect::<Vec<_>>(),
            vec![Indicator::Idle]
        );
        assert!(
            attention_rows(&attention, workspaces, "").is_empty(),
            "已读完成项离开 Attention"
        );
    }

    /// 对照 `testCommandsSeparateRunningAndUnreadCommandsFromAgents`。
    #[test]
    fn commands_separate_running_and_unread_commands_from_agents() {
        let workspace = SidebarItem::new("local@@dev@tmux@dev", "dev").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.shortcut = Some(1);
        });
        let attention = snapshot_for(
            &workspace,
            vec![
                PaneAttention {
                    acknowledged: true,
                    last_line: "building".to_string(),
                    seq: 1,
                    process_name: Some("cargo test".to_string()),
                    ..pane(1, PaneAttentionStatus::Working)
                },
                PaneAttention {
                    last_line: "finished".to_string(),
                    seq: 2,
                    process_name: Some("sleep".to_string()),
                    ..pane(2, PaneAttentionStatus::Done)
                },
                PaneAttention {
                    acknowledged: true,
                    last_line: "read".to_string(),
                    seq: 3,
                    process_name: Some("make".to_string()),
                    ..pane(3, PaneAttentionStatus::Done)
                },
                PaneAttention {
                    acknowledged: true,
                    last_line: "codex".to_string(),
                    seq: 4,
                    process_name: Some("codex".to_string()),
                    process_is_agent: true,
                    agent_name: Some("codex".to_string()),
                    ..pane(4, PaneAttentionStatus::Working)
                },
            ],
        );
        let items = commands(std::slice::from_ref(&workspace), Some(&attention));
        assert_eq!(
            items.iter().map(|i| i.title.clone()).collect::<Vec<_>>(),
            vec!["dev · local", "dev · local"]
        );
        assert_eq!(
            items.iter().map(|i| i.detail.clone()).collect::<Vec<_>>(),
            vec!["working · cargo test", "done · sleep"]
        );
        assert_eq!(
            items.iter().map(|i| i.pane_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            items.iter().map(|i| i.indicator).collect::<Vec<_>>(),
            vec![Indicator::Working, Indicator::Done]
        );
    }

    /// 对照 `testCoreClassifiedWrapperAgentGoesToAgentsNotCommands`。
    #[test]
    fn core_classified_wrapper_agent_goes_to_agents_not_commands() {
        let workspace = SidebarItem::new("local@@dev@tmux@dev", "dev").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.tab_number_by_pane.insert(7, 1);
        });
        let attention = snapshot_for(
            &workspace,
            vec![PaneAttention {
                acknowledged: true,
                last_line: "agent".to_string(),
                seq: 9,
                process_name: Some("cursor".to_string()),
                process_is_agent: true,
                ..pane(7, PaneAttentionStatus::Working)
            }],
        );
        let workspaces = std::slice::from_ref(&workspace);
        let agent_items = agents(workspaces, Some(&attention));
        assert_eq!(
            agent_items
                .iter()
                .map(|i| i.title.clone())
                .collect::<Vec<_>>(),
            vec!["dev · local"]
        );
        assert_eq!(
            agent_items
                .iter()
                .map(|i| i.detail.clone())
                .collect::<Vec<_>>(),
            vec!["working · cursor · Tab 1"]
        );
        assert!(commands(workspaces, Some(&attention)).is_empty());
    }

    /// 对照 `testAgentsSortByStatusBlocksThenStableTabPane`。
    #[test]
    fn agents_sort_by_status_blocks_then_stable_tab_pane() {
        let workspace = SidebarItem::new("local@@dev@tmux@dev", "dev").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "local".to_string();
            item.is_active = true;
            item.shortcut = Some(1);
            item.tab_number_by_pane.insert(1, 1);
            item.tab_number_by_pane.insert(2, 1);
            item.tab_number_by_pane.insert(3, 2);
            item.tab_number_by_pane.insert(4, 3);
        });
        let make = |pane_id, status, process: &str| PaneAttention {
            status,
            acknowledged: true,
            seq: pane_id as u64,
            process_name: Some(process.to_string()),
            process_is_agent: true,
            ..pane(pane_id, status)
        };
        let attention = snapshot_for(
            &workspace,
            vec![
                make(1, PaneAttentionStatus::Working, "codex"),
                PaneAttention {
                    acknowledged: false,
                    ..make(2, PaneAttentionStatus::Done, "droid")
                },
                make(3, PaneAttentionStatus::Idle, "amp"),
                make(4, PaneAttentionStatus::Working, "cursor"),
            ],
        );
        let workspaces = std::slice::from_ref(&workspace);
        let items = agents(workspaces, Some(&attention));
        assert_eq!(
            items.iter().map(|i| i.indicator).collect::<Vec<_>>(),
            vec![
                Indicator::Done,
                Indicator::Working,
                Indicator::Working,
                Indicator::Idle
            ]
        );
        assert_eq!(
            items
                .iter()
                .map(|i| i.agent_name.clone())
                .collect::<Vec<_>>(),
            vec!["droid", "codex", "cursor", "amp"]
        );
        assert_eq!(
            items.iter().map(|i| i.pane_id).collect::<Vec<_>>(),
            vec![2, 1, 4, 3]
        );

        // 仅状态变化：条目在块间移动，同块内相对顺序不变。
        let flipped = snapshot_for(
            &workspace,
            vec![
                make(1, PaneAttentionStatus::Idle, "codex"),
                PaneAttention {
                    acknowledged: false,
                    ..make(2, PaneAttentionStatus::Done, "droid")
                },
                make(3, PaneAttentionStatus::Idle, "amp"),
                make(4, PaneAttentionStatus::Working, "cursor"),
            ],
        );
        let after = agents(workspaces, Some(&flipped));
        assert_eq!(
            after.iter().map(|i| i.indicator).collect::<Vec<_>>(),
            vec![
                Indicator::Done,
                Indicator::Working,
                Indicator::Idle,
                Indicator::Idle
            ]
        );
        assert_eq!(
            after
                .iter()
                .map(|i| i.agent_name.clone())
                .collect::<Vec<_>>(),
            vec!["droid", "cursor", "codex", "amp"]
        );
        assert_eq!(
            after.iter().map(|i| i.pane_id).collect::<Vec<_>>(),
            vec![2, 4, 1, 3]
        );
        assert_eq!(
            attention_rows(&flipped, workspaces, "")
                .iter()
                .map(|row| row.pane.pane_id)
                .collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    /// 对照 `testAgentAlertsMatchAttentionAndLeaveCommandsAlone`。
    ///
    /// Swift 版本还断言了 `AggregateWorkspaceProjection.agentTabs`，那个模型
    /// 属于聚合工作区波次，尚未移植，此处不重复断言。
    #[test]
    fn agent_alerts_match_attention_and_leave_commands_alone() {
        let workspace =
            SidebarItem::new("yaklang-workspace:w1@ryzen", "yaklang-workspace").with(|item| {
                item.runtime = "herdr".to_string();
                item.transport = "ryzen".to_string();
                item.is_active = true;
                for (pane, tab) in [(3, 3), (4, 4), (5, 5), (9, 9)] {
                    item.tab_number_by_pane.insert(pane, tab);
                    item.tab_id_by_pane.insert(pane, tab * 10);
                }
            });
        let attention = AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![WorkspaceAttention {
                name: "yaklang-workspace".to_string(),
                transport: "ryzen".to_string(),
                path: "~/Developer/yaklang-workspace".to_string(),
                working: 3,
                ..WorkspaceAttention::new(
                    "yaklang-workspace:w1@ryzen",
                    vec![
                        PaneAttention {
                            last_line: "running".to_string(),
                            seq: 3,
                            process_name: Some("pi".to_string()),
                            ..pane(3, PaneAttentionStatus::Working)
                        },
                        PaneAttention {
                            muted: true,
                            last_line: "hidden".to_string(),
                            seq: 4,
                            process_name: Some("pi".to_string()),
                            process_is_agent: true,
                            agent_name: Some("pi".to_string()),
                            ..pane(4, PaneAttentionStatus::Working)
                        },
                        PaneAttention {
                            acknowledged: true,
                            last_line: "read".to_string(),
                            seq: 5,
                            process_name: Some("codex".to_string()),
                            process_is_agent: true,
                            agent_name: Some("Codex".to_string()),
                            ..pane(5, PaneAttentionStatus::Done)
                        },
                        PaneAttention {
                            last_line: "building".to_string(),
                            seq: 9,
                            process_name: Some("cargo test".to_string()),
                            ..pane(9, PaneAttentionStatus::Working)
                        },
                    ],
                )
            }],
        };
        let workspaces = std::slice::from_ref(&workspace);
        let agent_items = agents(workspaces, Some(&attention));
        let rows = attention_rows(&attention, workspaces, "");
        let command_items = commands(workspaces, Some(&attention));

        assert_eq!(
            agent_items.iter().map(|i| i.pane_id).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert_eq!(
            agent_items
                .iter()
                .map(|i| i.detail.clone())
                .collect::<Vec<_>>(),
            vec![
                "working · pi · Tab 3",
                "working · pi · Tab 4",
                "done · Codex · Tab 5"
            ]
        );
        assert_eq!(
            agent_items.iter().map(|i| i.indicator).collect::<Vec<_>>(),
            vec![Indicator::Working, Indicator::Working, Indicator::Idle]
        );
        let agent_row = rows
            .iter()
            .find(|row| row.pane.pane_id == 3)
            .expect("应有 pane 3 的行");
        assert_eq!(agent_row.indicator(), agent_items[0].indicator);
        assert_eq!(agent_row.detail(), agent_items[0].detail);
        assert_eq!(
            rows.iter()
                .map(|row| row.pane.pane_id)
                .collect::<HashSet<_>>(),
            HashSet::from([3, 9])
        );
        assert_eq!(
            command_items.iter().map(|i| i.pane_id).collect::<Vec<_>>(),
            vec![9]
        );
    }

    /// 对照 `testCursorProcessIsNotLabeledCodexAndIdleShellStaysOutOfAgents`。
    #[test]
    fn cursor_process_is_not_labeled_codex_and_idle_shell_stays_out_of_agents() {
        let workspace = tmux_workspace("muxterm@local", "muxterm", "local").with(|item| {
            item.shortcut = Some(1);
            item.tab_number_by_pane.insert(10, 1);
            item.tab_number_by_pane.insert(20, 2);
            item.tab_number_by_pane.insert(30, 3);
        });
        let attention = snapshot_for(
            &workspace,
            vec![
                PaneAttention {
                    status: PaneAttentionStatus::Idle,
                    acknowledged: true,
                    seq: 1,
                    process_name: Some("zsh".to_string()),
                    ..pane(10, PaneAttentionStatus::Idle)
                },
                PaneAttention {
                    status: PaneAttentionStatus::Working,
                    acknowledged: true,
                    last_line: "ctrl+c to stop".to_string(),
                    seq: 2,
                    process_name: Some(
                        "node /Users/x/.codex/vendor/cursor-agent/versions/1/index.js".to_string(),
                    ),
                    process_is_agent: true,
                    agent_name: Some("cursor".to_string()),
                    ..pane(20, PaneAttentionStatus::Working)
                },
                PaneAttention {
                    status: PaneAttentionStatus::Idle,
                    acknowledged: true,
                    seq: 3,
                    process_name: Some("zsh".to_string()),
                    ..pane(30, PaneAttentionStatus::Idle)
                },
            ],
        );
        let items = agents(std::slice::from_ref(&workspace), Some(&attention));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pane_id, 20);
        assert_eq!(items[0].agent_name, "cursor");
        assert_eq!(items[0].tab_number, Some(2));
        assert!(!items[0].detail.to_lowercase().contains("codex"));
    }

    /// 对照 `testPanelJumpRoutingRequiresMatchingWorkspaceForSharedPaneId`。
    #[test]
    fn panel_jump_routing_requires_matching_workspace_for_shared_pane_id() {
        let yaklang = "local@@yaklang@tmux@yaklang";
        let muxterm = "local@@muxterm@tmux@muxterm";
        let scenes = vec![
            ScenePaneIndex {
                workspace_ids: HashSet::from([yaklang.to_string()]),
                pane_ids: HashSet::from([151, 152]),
                is_open: true,
            },
            ScenePaneIndex {
                workspace_ids: HashSet::from([muxterm.to_string()]),
                pane_ids: HashSet::from([151, 153]),
                is_open: true,
            },
        ];
        assert!(target_scene_contains_pane(yaklang, 151, &scenes).is_some());
        assert!(
            target_scene_contains_pane(yaklang, 153, &scenes).is_none(),
            "同 paneId 在别的 workspace 时不得误命中"
        );
    }

    /// 对照 `testOrdinaryCommandDoesNotBecomePermanentAgent`。
    #[test]
    fn ordinary_command_does_not_become_permanent_agent() {
        let workspace = tmux_workspace("local@@dev@tmux@dev", "dev", "local");
        let attention = snapshot_for(
            &workspace,
            vec![PaneAttention {
                acknowledged: true,
                last_line: "building".to_string(),
                seq: 4,
                process_name: Some("cargo".to_string()),
                ..pane(1, PaneAttentionStatus::Working)
            }],
        );
        let workspaces = std::slice::from_ref(&workspace);
        assert!(agents(workspaces, Some(&attention)).is_empty());
        assert_eq!(attention_rows(&attention, workspaces, "").len(), 1);
    }

    /// 对照 `testTabActivitiesReuseAgentAndCommandProjectionPriority`。
    #[test]
    fn tab_activities_reuse_agent_and_command_projection_priority() {
        let workspace = SidebarItem::new("ssh@ryzen@dev", "dev").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "ryzen".to_string();
            item.is_active = true;
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(11, AgentStatus::Working)
            }];
            for (pane, tab) in [(11, 1), (12, 1), (21, 2), (31, 3)] {
                item.tab_number_by_pane.insert(pane, tab);
            }
            item.tab_id_by_pane.insert(11, 101);
            item.tab_id_by_pane.insert(12, 101);
            item.tab_id_by_pane.insert(21, 202);
            item.tab_id_by_pane.insert(31, 303);
        });
        let attention = AttentionSnapshot {
            blocked_count: 1,
            workspaces: vec![WorkspaceAttention {
                blocked: 1,
                done: 1,
                working: 2,
                ..WorkspaceAttention::new(
                    "ssh@ryzen@dev",
                    vec![
                        PaneAttention {
                            last_line: "thinking".to_string(),
                            seq: 1,
                            process_name: Some("codex".to_string()),
                            process_is_agent: true,
                            ..pane(11, PaneAttentionStatus::Working)
                        },
                        PaneAttention {
                            last_line: "input required".to_string(),
                            seq: 2,
                            process_name: Some("cargo".to_string()),
                            ..pane(12, PaneAttentionStatus::Blocked)
                        },
                        PaneAttention {
                            last_line: "finished".to_string(),
                            seq: 3,
                            process_name: Some("make".to_string()),
                            ..pane(21, PaneAttentionStatus::Done)
                        },
                        PaneAttention {
                            seq: 4,
                            process_name: Some("zsh".to_string()),
                            ..pane(31, PaneAttentionStatus::Idle)
                        },
                    ],
                )
            }],
        };
        let workspaces = std::slice::from_ref(&workspace);
        let activities = tab_activities(workspaces, Some(&attention), &HashSet::new());
        let key = |tab_id| TabActivityKey {
            workspace_id: "ssh@ryzen@dev".to_string(),
            tab_id,
        };
        assert_eq!(
            activities.get(&key(101)),
            Some(&Indicator::Blocked),
            "同一 Tab 的 blocked 应覆盖 working"
        );
        assert_eq!(activities.get(&key(202)), Some(&Indicator::Done));
        assert_eq!(activities.get(&key(303)), None, "idle pane 不应点亮 Tab");

        let hidden = HashSet::from([AttentionVisibilityKey {
            workspace_id: "ssh@ryzen@dev".to_string(),
            pane_id: 12,
        }]);
        let hidden_activities = tab_activities(workspaces, Some(&attention), &hidden);
        assert_eq!(
            hidden_activities.get(&key(101)),
            Some(&Indicator::Working),
            "隐藏的 pane 不再影响 Tab 标记"
        );
    }
}
