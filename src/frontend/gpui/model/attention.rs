//! 注意力（Attention）列表的纯逻辑：状态、警报规则、标题归一化、行投影。
//!
//! 移植自 `chrome/AttentionModel.swift`。
//!
//! 关键区分：
//! - **Core 状态**（acknowledge / mute）由 Core 持有，前端只读；
//! - **前端隐藏身份**（[`AttentionVisibilityKey`]）纯粹是本地视图状态，
//!   隐藏一个条目永远不等于 acknowledge 或 mute。

use std::collections::HashMap;

use super::sidebar::{AgentItem, SidebarItem};

/// 注意力 pane 状态，与 Core `PaneStatus` 对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneAttentionStatus {
    #[default]
    Unknown,
    Working,
    Done,
    Blocked,
    Idle,
}

impl PaneAttentionStatus {
    /// 是否属于 Attention 可展示状态。已读过滤由 [`PaneAttention`] 决定。
    pub fn is_listed(self) -> bool {
        matches!(
            self,
            PaneAttentionStatus::Working | PaneAttentionStatus::Blocked | PaneAttentionStatus::Done
        )
    }

    /// Core JSON 里的状态字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            PaneAttentionStatus::Unknown => "unknown",
            PaneAttentionStatus::Working => "working",
            PaneAttentionStatus::Done => "done",
            PaneAttentionStatus::Blocked => "blocked",
            PaneAttentionStatus::Idle => "idle",
        }
    }

    /// 解析 Core JSON 的状态字面量。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(PaneAttentionStatus::Unknown),
            "working" => Some(PaneAttentionStatus::Working),
            "done" => Some(PaneAttentionStatus::Done),
            "blocked" => Some(PaneAttentionStatus::Blocked),
            "idle" => Some(PaneAttentionStatus::Idle),
            _ => None,
        }
    }
}

/// 单个 pane 的注意力条目。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneAttention {
    pub pane_id: u32,
    pub status: PaneAttentionStatus,
    pub acknowledged: bool,
    pub muted: bool,
    pub last_line: String,
    pub seq: u64,
    pub process_name: Option<String>,
    /// Core 的权威分类：进程可能叫 `node`，但前台 argv 解析出来是 Codex。
    pub process_is_agent: bool,
    pub agent_name: Option<String>,
}

/// 工作区聚合注意力视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttention {
    pub workspace_id: String,
    pub name: String,
    pub transport: String,
    pub path: String,
    pub blocked: u32,
    pub done: u32,
    pub working: u32,
    pub panes: Vec<PaneAttention>,
}

impl WorkspaceAttention {
    /// 只填必需字段的构造器，测试用。
    pub fn new(workspace_id: impl Into<String>, panes: Vec<PaneAttention>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            name: String::new(),
            transport: String::new(),
            path: "~".to_string(),
            blocked: 0,
            done: 0,
            working: 0,
            panes,
        }
    }
}

/// 注意力快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttentionSnapshot {
    pub blocked_count: u32,
    pub workspaces: Vec<WorkspaceAttention>,
}

impl AttentionSnapshot {
    /// Core 的活动快照（`muxterm_attention_snapshot` 的 owned DTO）→ 前端视图。
    ///
    /// 对照 macOS `AttentionModel.Snapshot.decode(_:)`：字段缺省即默认值，
    /// 状态字面量不认识就按 `unknown`（不丢整条 pane）。
    pub fn snapshot_from_activity(
        snapshot: &crate::frontend::utils::corebridge::ClientActivitySnapshot,
    ) -> AttentionSnapshot {
        let workspaces = snapshot
            .workspaces
            .iter()
            .map(|workspace| WorkspaceAttention {
                workspace_id: workspace.workspace_id.clone(),
                name: String::new(),
                transport: String::new(),
                path: workspace.path.clone(),
                blocked: workspace.blocked as u32,
                done: workspace.done as u32,
                working: workspace.working as u32,
                panes: workspace
                    .panes
                    .iter()
                    .map(|pane| PaneAttention {
                        pane_id: pane.pane_id,
                        status: PaneAttentionStatus::parse(&pane.status)
                            .unwrap_or(PaneAttentionStatus::Unknown),
                        acknowledged: pane.acknowledged,
                        muted: pane.muted,
                        last_line: pane.last_line.clone(),
                        seq: pane.seq,
                        process_name: pane.process_name.clone(),
                        process_is_agent: pane.process_is_agent,
                        agent_name: pane.agent_name.clone(),
                    })
                    .collect(),
            })
            .collect();
        AttentionSnapshot {
            blocked_count: snapshot.blocked_count as u32,
            workspaces,
        }
    }

    /// 解析 Core `muxterm_attention_snapshot` JSON；失败返回 `None`，不 panic。
    ///
    /// 逐字段容错，与 Swift 的 `??` 默认值一致：缺省即取默认值，只有
    /// `ok != true`、整体不是 JSON、或缺少 `pane_id`/`status` 才丢条目。
    pub fn decode(data: &[u8]) -> Option<Self> {
        let json: serde_json::Value = serde_json::from_slice(data).ok()?;
        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return None;
        }
        let blocked_count = json
            .get("blocked_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let workspaces = json
            .get("workspaces")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(decode_workspace).collect())
            .unwrap_or_default();
        Some(Self {
            blocked_count,
            workspaces,
        })
    }
}

fn decode_workspace(value: &serde_json::Value) -> Option<WorkspaceAttention> {
    let workspace_id = value.get("workspace_id")?.as_str()?.to_string();
    let panes = value
        .get("panes")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(decode_pane).collect())
        .unwrap_or_default();
    Some(WorkspaceAttention {
        workspace_id,
        name: string_or(value, "name", ""),
        transport: string_or(value, "transport", "local"),
        path: string_or(value, "path", "~"),
        blocked: u32_or(value, "blocked", 0),
        done: u32_or(value, "done", 0),
        working: u32_or(value, "working", 0),
        panes,
    })
}

fn decode_pane(value: &serde_json::Value) -> Option<PaneAttention> {
    let pane_id = value.get("pane_id")?.as_u64()? as u32;
    let status = PaneAttentionStatus::parse(value.get("status")?.as_str()?)?;
    Some(PaneAttention {
        pane_id,
        status,
        acknowledged: value
            .get("acknowledged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        muted: value
            .get("muted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        last_line: string_or(value, "last_line", ""),
        seq: value.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
        process_name: value
            .get("process_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        process_is_agent: value
            .get("process_is_agent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        agent_name: value
            .get("agent_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn string_or(value: &serde_json::Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

fn u32_or(value: &serde_json::Value, key: &str, fallback: u32) -> u32 {
    value
        .get(key)
        .and_then(|v| v.as_u64())
        .unwrap_or(fallback as u64) as u32
}

/// 前端本地隐藏身份。隐藏不等于 acknowledge / mute。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttentionVisibilityKey {
    pub workspace_id: String,
    pub pane_id: u32,
}

/// 注意力列表行（过滤 + 排序后的展示条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionRow {
    pub workspace_id: String,
    pub transport: String,
    pub path: String,
    pub pane: PaneAttention,
    pub workspace_name: String,
    pub agent_name: String,
    pub tab_number: Option<u32>,
    pub projected_agent: Option<AgentItem>,
}

impl AttentionRow {
    /// 第一行：工作区名 + 进程 + 机器/路径身份。
    pub fn title(&self) -> String {
        let identity = AttentionRowLabel::display(
            self.pane.process_name.as_deref(),
            &self.transport,
            &self.path,
        );
        if self.workspace_name.is_empty() {
            identity
        } else {
            format!("{}  {identity}", self.workspace_name)
        }
    }

    /// 第二行：优先用 Agents 投影的 detail，保证两处文字一致。
    pub fn detail(&self) -> String {
        if let Some(projected) = &self.projected_agent {
            return projected.detail.clone();
        }
        let agent_name = if self.agent_name.is_empty() {
            AttentionRowLabel::normalized_process(self.pane.process_name.as_deref())
                .or_else(|| self.pane.process_name.clone())
                .unwrap_or_default()
        } else {
            self.agent_name.clone()
        };
        AttentionRowLabel::detail(self.pane.status, &agent_name, self.tab_number)
    }

    /// 指示标记：已读的 done 降为 idle。
    pub fn indicator(&self) -> super::sidebar::Indicator {
        if let Some(projected) = &self.projected_agent {
            return projected.indicator;
        }
        match self.pane.status {
            PaneAttentionStatus::Working => super::sidebar::Indicator::Working,
            PaneAttentionStatus::Blocked => super::sidebar::Indicator::Blocked,
            PaneAttentionStatus::Done => {
                if self.pane.acknowledged {
                    super::sidebar::Indicator::Idle
                } else {
                    super::sidebar::Indicator::Done
                }
            }
            PaneAttentionStatus::Idle | PaneAttentionStatus::Unknown => {
                super::sidebar::Indicator::Idle
            }
        }
    }

    /// 中文注释：本行的本地隐藏身份。
    pub fn visibility_key(&self) -> AttentionVisibilityKey {
        AttentionVisibilityKey {
            workspace_id: self.workspace_id.clone(),
            pane_id: self.pane.pane_id,
        }
    }
}

/// 已知 agent 可执行文件名（小写）。
///
/// 顺序也有意义：先匹配到的胜出。
const KNOWN_AGENTS: &[&str] = &[
    "codex", "cursor", "claude", "gemini", "aider", "opencode", "copilot", "cline", "goose", "amp",
    "grok", "windsurf", "kiro", "pi", "hermes", "droid",
];

/// command 分词的分隔符：空白以及 shell 元字符。
fn is_command_separator(c: char) -> bool {
    c.is_whitespace() || ";|&<>\"'`()".contains(c)
}

/// 注意力行标题：进程名 + transport + path，不用 last_line 片段。
pub struct AttentionRowLabel;

impl AttentionRowLabel {
    /// 只认第一个可执行文件的 basename。路径中间的 agent 目录不是进程身份。
    pub fn known_agent_executable(process: Option<&str>) -> Option<String> {
        let process = process?;
        let value = strip_terminal_sequences(process).trim().to_string();
        if value.is_empty() {
            return None;
        }
        let token = value.split(is_command_separator).next()?;
        let raw = token.trim_matches(['\'', '"']);
        if raw.is_empty() || raw.starts_with('-') {
            return None;
        }
        known_agent_basename(basename(raw))
    }

    /// 把 wrapper 名称收敛成用户真正关心的 agent 名。
    ///
    /// `npx @openai/codex`、`/opt/cursor-agent` 都应显示为 codex / cursor，
    /// 而不是路径或 node wrapper。未知命令只保留第一个可执行词的 basename，
    /// 避免把 `sleep 1 && echo output` 这类命令行混进标题。
    pub fn normalized_process(process: Option<&str>) -> Option<String> {
        let process = process?;
        let value = strip_terminal_sequences(process).trim().to_string();
        if value.is_empty() {
            return None;
        }
        let tokens: Vec<&str> = value.split(is_command_separator).collect();

        // 先看 argv basename，避免路径中间的 cursor/codex 目录互相抢身份。
        for token in &tokens {
            let raw = token.trim_matches(['\'', '"']);
            if raw.is_empty() || raw.starts_with('-') {
                continue;
            }
            let package = basename(raw);
            let lower = package.to_lowercase();
            if let Some(alias) = agent_alias(&lower) {
                return Some(alias.to_string());
            }
            if let Some(known) = known_agent_prefix(&lower) {
                return Some(known.to_string());
            }
        }
        // 路径段从右往左：`.../codex/.../cursor-agent/...` 必须认 cursor。
        for token in &tokens {
            for segment in token.split(['/', '\\']).filter(|s| !s.is_empty()).rev() {
                let lower = segment.to_lowercase();
                if let Some(alias) = agent_alias(&lower) {
                    return Some(alias.to_string());
                }
                if let Some(known) = KNOWN_AGENTS.iter().find(|k| lower == **k) {
                    return Some((*known).to_string());
                }
            }
        }
        let first = tokens.first()?;
        Some(basename(first).to_string())
    }

    /// `进程  transport  path`；进程缺失时显示 `?`。
    pub fn display(process: Option<&str>, transport: &str, path: &str) -> String {
        let trimmed = Self::normalized_process(process).unwrap_or_default();
        let name = if trimmed.is_empty() { "?" } else { &trimmed };
        format!("{name}  {transport}  {path}")
    }

    /// `status · agentName · Tab N`。
    pub fn detail(
        status: PaneAttentionStatus,
        agent_name: &str,
        tab_number: Option<u32>,
    ) -> String {
        let status_text = match status {
            PaneAttentionStatus::Working => "working",
            PaneAttentionStatus::Blocked => "blocked",
            PaneAttentionStatus::Done => "done",
            PaneAttentionStatus::Idle | PaneAttentionStatus::Unknown => "idle",
        };
        let mut values = vec![status_text.to_string()];
        if !agent_name.is_empty() {
            values.push(agent_name.to_string());
        }
        if let Some(tab) = tab_number.filter(|n| *n > 0) {
            values.push(format!("Tab {tab}"));
        }
        values.join(" · ")
    }

    /// 侧栏对齐的整行文本：`工作区 · status · agent · Tab N`。
    pub fn sidebar_aligned(
        workspace_name: &str,
        status: PaneAttentionStatus,
        agent_name: &str,
        tab_number: Option<u32>,
    ) -> String {
        let detail = Self::detail(status, agent_name, tab_number);
        [workspace_name.to_string(), detail]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn agent_alias(lower: &str) -> Option<&'static str> {
    match lower {
        "agent" | "cursor-agent" => Some("cursor"),
        _ => None,
    }
}

fn known_agent_prefix(lower: &str) -> Option<&'static str> {
    KNOWN_AGENTS.iter().copied().find(|known| {
        lower == *known
            || lower.starts_with(&format!("{known}-"))
            || lower.starts_with(&format!("{known}_"))
    })
}

/// 已知 agent basename → 规范名。
fn known_agent_basename(basename: &str) -> Option<String> {
    let lower = basename.to_lowercase();
    if let Some(alias) = agent_alias(&lower) {
        return Some(alias.to_string());
    }
    known_agent_prefix(&lower).map(str::to_string)
}

/// 取最后一个路径段。
fn basename(value: &str) -> &str {
    value
        .split(['/', '\\'])
        .next_back()
        .filter(|s| !s.is_empty())
        .unwrap_or(value)
}

/// `process_name` 可能来自 pane-cmd 或带 ANSI 的 shell 状态行。
///
/// 标题只需要可执行名，所以先丢弃终端控制序列和不可见控制字符：
/// - CSI（`ESC [` 到 final byte）整段丢弃；
/// - OSC（`ESC ]` 到 BEL 或 `ESC \`）整段丢弃；
/// - 其它 ESC 序列丢掉一个字节；
/// - 退格 / DEL 回删一个字符（shell 重绘 `s\b sleep` 不能把 `s` 当第一个词）；
/// - 其余 C0 控制字符变成空格。
fn strip_terminal_sequences(value: &str) -> String {
    let scalars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len());
    let mut index = 0;

    while index < scalars.len() {
        let scalar = scalars[index];
        if scalar != '\u{1b}' {
            match scalar {
                '\u{08}' | '\u{7f}' => {
                    // 注意：Swift 的 removeLast 按 Character 删，这里按 char 删；
                    // 对 ASCII 控制序列等价，非 ASCII 情形两边都已在前面丢弃。
                    result.pop();
                }
                c if (c as u32) < 0x20 => result.push(' '),
                c => result.push(c),
            }
            index += 1;
            continue;
        }

        // CSI: ESC [ ... final byte (0x40..=0x7e)
        index += 1;
        if index >= scalars.len() {
            break;
        }
        if scalars[index] == '[' {
            index += 1;
            while index < scalars.len() {
                let final_byte = scalars[index] as u32;
                index += 1;
                if (0x40..=0x7e).contains(&final_byte) {
                    break;
                }
            }
            continue;
        }

        // OSC: ESC ] ... BEL or ESC backslash
        if scalars[index] == ']' {
            index += 1;
            while index < scalars.len() {
                let current = scalars[index];
                if current == '\u{07}' {
                    index += 1;
                    break;
                }
                if current == '\u{1b}' && index + 1 < scalars.len() && scalars[index + 1] == '\\' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }

        // 其它 ESC 序列通常只有一个 final byte；跳过它，避免把控制码参数
        // 误识别成进程名。
        index += 1;
    }
    result
}

/// Attention 与 Commands 共用的警报规则。
///
/// Working 一直列出；未读的 blocked/done 列出；静音、已读完成和空闲都不算
/// 警报。Agents 不用这套显隐：Core 已识别的 agent 无论状态都留在 Agents。
pub struct AttentionAlert;

impl AttentionAlert {
    /// 中文注释：该 pane 是否应出现在 Attention / Commands 列表。
    pub fn includes(pane: &PaneAttention) -> bool {
        !pane.muted
            && pane.status.is_listed()
            && (pane.status == PaneAttentionStatus::Working || !pane.acknowledged)
    }

    /// Core 标记、权威 agent 名，或可执行文件本身就是已知 agent。
    ///
    /// 不扫描路径中间的目录，避免把普通命令吸进 Agents。
    pub fn is_agent(pane: &PaneAttention) -> bool {
        if pane.process_is_agent {
            return true;
        }
        if let Some(name) = pane.agent_name.as_deref().map(str::trim) {
            if !name.is_empty() {
                return true;
            }
        }
        AttentionRowLabel::known_agent_executable(pane.process_name.as_deref()).is_some()
    }

    /// 展示用的 agent 名。
    pub fn agent_name(pane: &PaneAttention) -> Option<String> {
        if let Some(explicit) = pane.agent_name.as_deref().map(str::trim) {
            if !explicit.is_empty() {
                return Some(explicit.to_string());
            }
        }
        if let Some(known) = AttentionRowLabel::known_agent_executable(pane.process_name.as_deref())
        {
            return Some(known);
        }
        let process = pane.process_name.as_deref().map(str::trim);
        match (pane.process_is_agent, process) {
            (true, Some(process)) if !process.is_empty() => Some(process.to_string()),
            _ => None,
        }
    }
}

/// 注意力列表纯逻辑：保留 running 与未读 done/blocked；已读完成项不再出现。
pub fn attention_rows(
    snapshot: &AttentionSnapshot,
    workspaces: &[SidebarItem],
    query: &str,
) -> Vec<AttentionRow> {
    let q = query.trim().to_lowercase();
    let projected = super::sidebar::agents(workspaces, Some(snapshot));
    let mut projected_by_pane: HashMap<(String, u32), AgentItem> = HashMap::new();
    for item in projected {
        projected_by_pane
            .entry((item.workspace_id.clone(), item.pane_id))
            .or_insert(item);
    }

    let mut rows: Vec<AttentionRow> = Vec::new();
    for workspace in &snapshot.workspaces {
        let chrome = workspaces
            .iter()
            .find(|item| item.workspace_id == workspace.workspace_id);
        if let Some(chrome) = chrome {
            if chrome.is_aggregate() || chrome.is_opening() {
                continue;
            }
            if !super::sidebar::backend_status::includes_activity(chrome.backend_status) {
                continue;
            }
        }

        // 工作区 chrome 可以补名称与 transport，但不能用 `runtime @ transport`
        // 副标题覆盖 Core 提供的真实路径。
        let transport = if !workspace.transport.is_empty() {
            workspace.transport.clone()
        } else if let Some(chrome) = chrome {
            chrome.transport.clone()
        } else if workspace.workspace_id.contains("@ssh") {
            "ssh".to_string()
        } else {
            "local".to_string()
        };
        let workspace_name = first_nonempty([
            chrome.map(|c| c.name.as_str()),
            Some(workspace.name.as_str()),
            workspace.workspace_id.split('@').next(),
        ])
        .unwrap_or_else(|| workspace.workspace_id.clone());

        for pane in workspace
            .panes
            .iter()
            .filter(|pane| AttentionAlert::includes(pane))
        {
            let projected_agent = projected_by_pane
                .get(&(workspace.workspace_id.clone(), pane.pane_id))
                .cloned();
            let agent_name = first_nonempty([
                chrome
                    .and_then(|c| {
                        c.structured_agents
                            .iter()
                            .find(|a| a.pane_id == pane.pane_id)
                    })
                    .and_then(|agent| {
                        first_nonempty([
                            agent.display_name.as_deref(),
                            agent.name.as_deref(),
                            agent.kind.as_deref(),
                            agent.title.as_deref(),
                        ])
                    })
                    .as_deref(),
                pane.agent_name.as_deref(),
                AttentionRowLabel::normalized_process(pane.process_name.as_deref()).as_deref(),
                pane.process_name.as_deref(),
            ])
            .unwrap_or_default();

            let matches = q.is_empty()
                || workspace.workspace_id.to_lowercase().contains(&q)
                || workspace_name.to_lowercase().contains(&q)
                || agent_name.to_lowercase().contains(&q)
                || pane
                    .process_name
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&q)
                || pane.last_line.to_lowercase().contains(&q)
                || transport.to_lowercase().contains(&q);
            if !matches {
                continue;
            }

            rows.push(AttentionRow {
                workspace_id: workspace.workspace_id.clone(),
                transport: transport.clone(),
                // 路径必须来自 Core 快照，不能用 chrome 的副标题替换。
                path: workspace.path.clone(),
                pane: pane.clone(),
                workspace_name: workspace_name.clone(),
                agent_name,
                tab_number: chrome.and_then(|c| c.tab_number_by_pane.get(&pane.pane_id).copied()),
                projected_agent,
            });
        }
    }

    // done → blocked → working → idle；同块内新的在前。
    rows.sort_by(|a, b| {
        a.indicator()
            .activity_rank()
            .cmp(&b.indicator().activity_rank())
            .then_with(|| b.pane.seq.cmp(&a.pane.seq))
    });
    rows
}

/// 取第一个非空（trim 后）字符串。
fn first_nonempty<const N: usize>(values: [Option<&str>; N]) -> Option<String> {
    values.iter().find_map(|value| {
        let trimmed = (*value)?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// 单条结构化通知；新版 FFI 提供 pane、进程名和最后一行，旧版可缺失 pane。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionNotification {
    pub workspace_id: String,
    pub pane_id: Option<u32>,
    pub kind: PaneAttentionStatus,
    pub process_name: Option<String>,
    pub last_line: String,
    pub seq: u64,
}

impl AttentionNotification {
    /// 中文注释：通知里展示用的进程名。
    pub fn display_process_name(&self) -> Option<String> {
        AttentionRowLabel::normalized_process(self.process_name.as_deref())
    }
}

/// 通知记录。新版核心提供结构化记录；旧核心只给工作区数组。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttentionNotifications {
    pub blocked: Vec<String>,
    pub done: Vec<String>,
    pub notifications: Vec<AttentionNotification>,
}

impl AttentionNotifications {
    /// 解析 Core `muxterm_attention_take_notifications` JSON。
    pub fn decode(data: &[u8]) -> Option<Self> {
        let json: serde_json::Value = serde_json::from_slice(data).ok()?;
        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return None;
        }
        let strings = |key: &str| -> Vec<String> {
            json.get(key)
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let blocked = strings("blocked");
        let done = strings("done");

        // 只接受 blocked / done 两种 kind；其它状态不是通知。
        let structured: Vec<AttentionNotification> = json
            .get("notifications")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let workspace_id = item.get("workspace_id")?.as_str()?.to_string();
                        let kind = PaneAttentionStatus::parse(item.get("kind")?.as_str()?)?;
                        if !matches!(
                            kind,
                            PaneAttentionStatus::Blocked | PaneAttentionStatus::Done
                        ) {
                            return None;
                        }
                        Some(AttentionNotification {
                            workspace_id,
                            pane_id: item
                                .get("pane_id")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32),
                            kind,
                            process_name: item
                                .get("process_name")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            last_line: item
                                .get("last_line")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            seq: item.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // 与旧 FFI 的 workspace-only 响应兼容：pane 缺失时前端只做系统通知，
        // 不会尝试 acknowledge 一个不确定的 pane。
        let notifications = if structured.is_empty() {
            let mut legacy: Vec<AttentionNotification> = blocked
                .iter()
                .map(|id| AttentionNotification {
                    workspace_id: id.clone(),
                    pane_id: None,
                    kind: PaneAttentionStatus::Blocked,
                    process_name: None,
                    last_line: String::new(),
                    seq: 0,
                })
                .collect();
            legacy.extend(done.iter().map(|id| AttentionNotification {
                workspace_id: id.clone(),
                pane_id: None,
                kind: PaneAttentionStatus::Done,
                process_name: None,
                last_line: String::new(),
                seq: 0,
            }));
            legacy
        } else {
            structured.clone()
        };

        let blocked = if blocked.is_empty() {
            structured
                .iter()
                .filter(|n| n.kind == PaneAttentionStatus::Blocked)
                .map(|n| n.workspace_id.clone())
                .collect()
        } else {
            blocked
        };
        let done = if done.is_empty() {
            structured
                .iter()
                .filter(|n| n.kind == PaneAttentionStatus::Done)
                .map(|n| n.workspace_id.clone())
                .collect()
        } else {
            done
        };
        Some(Self {
            blocked,
            done,
            notifications,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::sidebar::{AgentStatus, Indicator, PaneAgent, SidebarItem};

    /// 构造一个只填必需字段的 pane 条目。
    fn pane(pane_id: u32, status: PaneAttentionStatus) -> PaneAttention {
        PaneAttention {
            pane_id,
            status,
            ..PaneAttention::default()
        }
    }

    fn list_snapshot(panes: Vec<PaneAttention>) -> AttentionSnapshot {
        let blocked = panes
            .iter()
            .filter(|p| p.status == PaneAttentionStatus::Blocked)
            .count() as u32;
        let done = panes
            .iter()
            .filter(|p| p.status == PaneAttentionStatus::Done)
            .count() as u32;
        AttentionSnapshot {
            blocked_count: blocked,
            workspaces: vec![WorkspaceAttention {
                blocked,
                done,
                ..WorkspaceAttention::new("ws@local", panes)
            }],
        }
    }

    // === AttentionSnapshotDecodeTests ===

    /// 对照 `testDecodesBlockedCountAndWorkspaces`。
    #[test]
    fn decodes_blocked_count_and_workspaces() {
        let json = r#"
        {
          "ok": true,
          "blocked_count": 2,
          "workspaces": [
            {
              "workspace_id": "legion@local",
              "blocked": 1,
              "done": 0,
              "working": 0,
              "panes": [
                {"pane_id": 1, "status": "blocked", "acknowledged": false, "last_line": "ask?", "seq": 3, "process_name": "cat"}
              ]
            },
            {
              "workspace_id": "other@local",
              "blocked": 0,
              "done": 1,
              "working": 0,
              "panes": [
                {"pane_id": 2, "status": "done", "acknowledged": true, "last_line": "complete", "seq": 7, "process_name": null}
              ]
            }
          ]
        }
        "#;
        let snapshot = AttentionSnapshot::decode(json.as_bytes()).expect("应当能解析");
        assert_eq!(snapshot.blocked_count, 2);
        assert_eq!(snapshot.workspaces.len(), 2);
        assert_eq!(snapshot.workspaces[0].workspace_id, "legion@local");
        assert_eq!(
            snapshot.workspaces[0].panes[0].status,
            PaneAttentionStatus::Blocked
        );
        assert!(!snapshot.workspaces[0].panes[0].acknowledged);
        assert_eq!(snapshot.workspaces[0].panes[0].last_line, "ask?");
        assert_eq!(
            snapshot.workspaces[1].panes[0].status,
            PaneAttentionStatus::Done
        );
        assert!(snapshot.workspaces[1].panes[0].acknowledged);
    }

    /// 对照 `testDecodeFailureReturnsNil`。
    #[test]
    fn decode_failure_returns_none() {
        assert!(AttentionSnapshot::decode(b"not json").is_none());
        assert!(AttentionSnapshot::decode(br#"{"ok": false}"#).is_none());
    }

    /// 对照 `testDecodeIncludesWorkspaceNameTransportAndPath`。
    #[test]
    fn decode_includes_workspace_name_transport_and_path() {
        let json = r#"
        {
          "ok": true,
          "blocked_count": 0,
          "workspaces": [
            {
              "workspace_id": "muxterm@ryzen",
              "name": "muxterm",
              "transport": "ryzen",
              "path": "~/Developer/self/muxterm",
              "blocked": 0, "done": 0, "working": 1,
              "panes": [
                {"pane_id": 4, "status": "working", "acknowledged": false, "last_line": "running",
                 "seq": 1, "process_name": "codex", "agent_name": "Codex"}
              ]
            }
          ]
        }
        "#;
        let snapshot = AttentionSnapshot::decode(json.as_bytes()).expect("应当能解析");
        assert_eq!(snapshot.workspaces[0].name, "muxterm");
        assert_eq!(snapshot.workspaces[0].transport, "ryzen");
        assert_eq!(snapshot.workspaces[0].path, "~/Developer/self/muxterm");
        assert_eq!(
            snapshot.workspaces[0].panes[0].agent_name.as_deref(),
            Some("Codex")
        );
    }

    // === AttentionListTests ===

    /// 对照 `testRunningAndUnreadDoneAreListed`：idle 不列出；done 排最前。
    #[test]
    fn running_and_unread_done_are_listed() {
        let snapshot = list_snapshot(vec![
            pane(1, PaneAttentionStatus::Blocked),
            pane(2, PaneAttentionStatus::Done),
            pane(3, PaneAttentionStatus::Working),
            pane(4, PaneAttentionStatus::Idle),
        ]);
        let rows = attention_rows(&snapshot, &[], "");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pane.pane_id, 2);
        assert_eq!(rows[1].pane.pane_id, 1);
        assert_eq!(rows[2].pane.pane_id, 3);
    }

    /// 对照 `testReadDoneDoesNotRemainInAttention`。
    #[test]
    fn read_done_does_not_remain_in_attention() {
        let snapshot = AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![WorkspaceAttention::new(
                "ws@local",
                vec![PaneAttention {
                    pane_id: 7,
                    status: PaneAttentionStatus::Done,
                    acknowledged: true,
                    last_line: "complete".to_string(),
                    seq: 9,
                    process_name: Some("codex".to_string()),
                    ..PaneAttention::default()
                }],
            )],
        };
        assert!(attention_rows(&snapshot, &[], "").is_empty());
    }

    /// 对照 `testDoneFirstThenNewerBlockedSeq`。
    #[test]
    fn done_first_then_newer_blocked_seq() {
        let snapshot = list_snapshot(vec![
            PaneAttention {
                seq: 1,
                ..pane(1, PaneAttentionStatus::Done)
            },
            PaneAttention {
                seq: 2,
                ..pane(2, PaneAttentionStatus::Blocked)
            },
            PaneAttention {
                seq: 3,
                ..pane(3, PaneAttentionStatus::Blocked)
            },
        ]);
        let ids: Vec<u32> = attention_rows(&snapshot, &[], "")
            .iter()
            .map(|row| row.pane.pane_id)
            .collect();
        assert_eq!(ids, vec![1, 3, 2]);
    }

    /// 对照 `testQueryFiltersByWorkspaceProcessAndLine`。
    #[test]
    fn query_filters_by_workspace_process_and_line() {
        let snapshot = AttentionSnapshot {
            blocked_count: 2,
            workspaces: vec![
                WorkspaceAttention::new(
                    "legion@local",
                    vec![PaneAttention {
                        last_line: "ask?".to_string(),
                        seq: 1,
                        process_name: Some("codex".to_string()),
                        ..pane(1, PaneAttentionStatus::Blocked)
                    }],
                ),
                WorkspaceAttention::new(
                    "other@local",
                    vec![PaneAttention {
                        last_line: "confirm?".to_string(),
                        seq: 2,
                        process_name: Some("bash".to_string()),
                        ..pane(2, PaneAttentionStatus::Blocked)
                    }],
                ),
            ],
        };
        assert_eq!(attention_rows(&snapshot, &[], "legion").len(), 1);
        assert_eq!(attention_rows(&snapshot, &[], "codex").len(), 1);
        assert_eq!(attention_rows(&snapshot, &[], "confirm").len(), 1);
        assert_eq!(attention_rows(&snapshot, &[], "missing").len(), 0);
    }

    /// 对照 `testAttentionTitleMatchesSidebarWorkspaceAgentAndTab`。
    #[test]
    fn attention_title_matches_sidebar_workspace_agent_and_tab() {
        let snapshot = AttentionSnapshot {
            blocked_count: 0,
            workspaces: vec![WorkspaceAttention {
                name: "muxterm".to_string(),
                transport: "ryzen".to_string(),
                path: "~/Developer/self/muxterm".to_string(),
                working: 1,
                ..WorkspaceAttention::new(
                    "muxterm@ryzen",
                    vec![PaneAttention {
                        seq: 1,
                        process_name: Some("codex".to_string()),
                        agent_name: Some("Codex".to_string()),
                        ..pane(4, PaneAttentionStatus::Working)
                    }],
                )
            }],
        };
        let chrome = SidebarItem::new("muxterm@ryzen", "muxterm").with(|item| {
            item.runtime = "tmux".to_string();
            item.transport = "ryzen".to_string();
            item.is_active = true;
            item.structured_agents = vec![PaneAgent {
                display_name: Some("Codex".to_string()),
                name: Some("codex".to_string()),
                kind: Some("codex".to_string()),
                status: AgentStatus::Working,
                ..PaneAgent::new(4, AgentStatus::Working)
            }];
            item.tab_number_by_pane.insert(4, 2);
        });
        let rows = attention_rows(&snapshot, std::slice::from_ref(&chrome), "");
        let projected = crate::frontend::gpui::model::sidebar::agents(
            std::slice::from_ref(&chrome),
            Some(&snapshot),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(projected.len(), 1);
        assert_eq!(
            rows[0].title(),
            "muxterm  codex  ryzen  ~/Developer/self/muxterm"
        );
        assert_eq!(rows[0].detail(), "working · Codex · Tab 2");
        assert_eq!(rows[0].indicator(), Indicator::Working);
        assert_eq!(rows[0].detail(), projected[0].detail);
        assert_eq!(rows[0].indicator(), projected[0].indicator);
        assert!(rows[0].title().contains("ryzen"));
        assert!(rows[0].title().contains("~/Developer/self/muxterm"));
        assert_eq!(
            AttentionRowLabel::sidebar_aligned(
                "muxterm",
                PaneAttentionStatus::Working,
                "Codex",
                Some(2)
            ),
            "muxterm · working · Codex · Tab 2"
        );
    }

    /// 对照 `testUnreadBlockedAndDoneUseDoneIndicator`。
    #[test]
    fn unread_blocked_and_done_use_done_indicator() {
        let snapshot = list_snapshot(vec![
            PaneAttention {
                seq: 1,
                process_name: Some("cat".to_string()),
                last_line: "ask?".to_string(),
                ..pane(1, PaneAttentionStatus::Blocked)
            },
            PaneAttention {
                seq: 2,
                process_name: Some("cat".to_string()),
                last_line: "complete".to_string(),
                ..pane(2, PaneAttentionStatus::Done)
            },
        ]);
        let rows = attention_rows(&snapshot, &[], "");
        assert_eq!(
            rows.iter().map(AttentionRow::indicator).collect::<Vec<_>>(),
            vec![Indicator::Done, Indicator::Blocked]
        );
        assert_eq!(rows[0].detail(), "done · cat");
        assert_eq!(rows[1].detail(), "blocked · cat");
    }

    // === AttentionNotificationsDecodeTests ===

    /// 对照 `testDecodesBlockedAndDone`。
    #[test]
    fn notifications_decode_blocked_and_done() {
        let notifications = AttentionNotifications::decode(
            br#"{"ok": true, "blocked": ["a@local"], "done": ["b@local"]}"#,
        )
        .expect("应当能解析");
        assert_eq!(notifications.blocked, vec!["a@local"]);
        assert_eq!(notifications.done, vec!["b@local"]);
    }

    /// 对照 `testDecodeFailureReturnsNil`（通知版）。
    #[test]
    fn notifications_decode_failure_returns_none() {
        assert!(AttentionNotifications::decode(br#"{"ok": false}"#).is_none());
    }

    /// 对照 `testDecodesStructuredPaneNotificationAndKeepsLegacyArrays`。
    #[test]
    fn structured_pane_notification_keeps_legacy_arrays() {
        let json = r#"
        {
          "ok": true,
          "notifications": [
            {
              "workspace_id": "legion@local",
              "pane_id": 9,
              "kind": "blocked",
              "process_name": "node /opt/codex",
              "last_line": "Approve?",
              "seq": 42
            }
          ],
          "blocked": ["legion@local"],
          "done": []
        }
        "#;
        let notifications = AttentionNotifications::decode(json.as_bytes()).expect("应当能解析");
        assert_eq!(notifications.blocked, vec!["legion@local"]);
        assert_eq!(notifications.notifications.len(), 1);
        let record = &notifications.notifications[0];
        assert_eq!(record.pane_id, Some(9));
        assert_eq!(record.kind, PaneAttentionStatus::Blocked);
        assert_eq!(record.display_process_name().as_deref(), Some("codex"));
        assert_eq!(record.last_line, "Approve?");
        assert_eq!(record.seq, 42);
    }

    /// 对照 `testLegacyNotificationArraysBecomeCompatibilityRecords`。
    #[test]
    fn legacy_notification_arrays_become_compatibility_records() {
        let notifications = AttentionNotifications::decode(
            br#"{"ok": true, "blocked": ["a@local"], "done": ["b@local"]}"#,
        )
        .expect("应当能解析");
        assert_eq!(
            notifications
                .notifications
                .iter()
                .map(|n| n.workspace_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a@local", "b@local"]
        );
        assert_eq!(notifications.notifications[0].pane_id, None);
    }

    // === AttentionRowLabelTests ===

    /// 对照 `testJoinsProcessTransportAndPath`。
    #[test]
    fn joins_process_transport_and_path() {
        let title = AttentionRowLabel::display(Some("sleep"), "ssh", "/home/wlz/yaklang-workspace");
        assert_eq!(title, "sleep  ssh  /home/wlz/yaklang-workspace");
        assert!(!title.contains("aa"));
    }

    /// 对照 `testEmptyProcessBecomesQuestionMark`。
    #[test]
    fn empty_process_becomes_question_mark() {
        assert_eq!(
            AttentionRowLabel::display(Some("  "), "local", "~"),
            "?  local  ~"
        );
    }

    /// 对照 `testKnownAgentExecutableUsesOnlyTheProgramBasename`。
    #[test]
    fn known_agent_executable_uses_only_the_program_basename() {
        assert_eq!(
            AttentionRowLabel::known_agent_executable(Some("pi")).as_deref(),
            Some("pi")
        );
        assert_eq!(
            AttentionRowLabel::known_agent_executable(Some("/usr/local/bin/pi --model x"))
                .as_deref(),
            Some("pi")
        );
        assert_eq!(
            AttentionRowLabel::known_agent_executable(Some("cargo test ~/Developer/pi")),
            None,
            "参数或路径里的 pi 不是 agent"
        );
        assert_eq!(
            AttentionRowLabel::known_agent_executable(Some("node /opt/codex-cli")),
            None
        );
    }

    /// 对照 `testNormalizesKnownAgentWrapper`。
    #[test]
    fn normalizes_known_agent_wrapper() {
        assert_eq!(
            AttentionRowLabel::normalized_process(Some("node /opt/codex-cli")).as_deref(),
            Some("codex")
        );
        assert_eq!(
            AttentionRowLabel::display(Some("npx @openai/codex"), "local", "~"),
            "codex  local  ~"
        );
        assert_eq!(
            AttentionRowLabel::normalized_process(Some("pi")).as_deref(),
            Some("pi")
        );
        assert_eq!(
            AttentionRowLabel::normalized_process(Some("hermes-agent")).as_deref(),
            Some("hermes")
        );
        assert_eq!(
            AttentionRowLabel::normalized_process(Some(
                "node /Users/x/.codex/vendor/cursor-agent/versions/1/index.js"
            ))
            .as_deref(),
            Some("cursor"),
            "路径左侧的 .codex 不得把 cursor-agent 标成 codex"
        );
        assert_ne!(
            AttentionRowLabel::normalized_process(Some(
                "node /Users/x/.codex/vendor/helper/index.js"
            ))
            .as_deref(),
            Some("codex"),
            "仅 .codex 配置目录不得识别成 agent"
        );
        assert_eq!(
            AttentionRowLabel::normalized_process(Some("agent --resume")).as_deref(),
            Some("cursor")
        );
    }

    /// 对照 `testExtractsExecutableFromAnsiShellCommand`。
    #[test]
    fn extracts_executable_from_ansi_shell_command() {
        let process = "\u{1b}[K\u{1b}[?2004hs\u{08}sleep 1 && echo AA_FG\u{1b}[?2004l";
        assert_eq!(
            AttentionRowLabel::normalized_process(Some(process)).as_deref(),
            Some("sleep")
        );
        let title = AttentionRowLabel::display(Some(process), "ssh", "~");
        assert_eq!(title, "sleep  ssh  ~");
        assert!(!title.contains("AA_FG"));
    }

    /// 对照 `testStripsOscTitleBeforeExtractingExecutable`。
    #[test]
    fn strips_osc_title_before_extracting_executable() {
        let process = "\u{1b}]0;sleep 1 && echo AA_BG\u{1b}\\/bin/zsh -i";
        assert_eq!(
            AttentionRowLabel::normalized_process(Some(process)).as_deref(),
            Some("zsh")
        );
    }
}

#[cfg(test)]
mod client_mapping_tests {
    use super::*;
    use crate::frontend::utils::corebridge::{
        ClientActivitySnapshot, ClientAttentionPane, ClientWorkspaceAttention,
    };

    fn pane(
        pane_id: u32,
        status: &str,
        process: Option<&str>,
        is_agent: bool,
    ) -> ClientAttentionPane {
        ClientAttentionPane {
            muted: false,
            workspace_id: String::new(),
            pane_id,
            status: status.to_string(),
            acknowledged: false,
            last_line: "…".to_string(),
            seq: 7,
            process_name: process.map(str::to_string),
            process_is_agent: is_agent,
            agent_name: is_agent.then(|| process.unwrap_or_default().to_string()),
            shell_name: None,
            program_status: None,
        }
    }

    /// Core 的活动快照要原样落到前端视图：agent 标记、状态、末行。
    ///
    /// 对照 `AttentionModelTests` 里「agent pane 计入 Agents」的那组断言。
    #[test]
    fn activity_snapshot_maps_agent_panes() {
        let snapshot = ClientActivitySnapshot {
            blocked_count: 1,
            workspaces: vec![ClientWorkspaceAttention {
                workspace_id: "ws".to_string(),
                path: "/home/u/dev".to_string(),
                blocked: 1,
                done: 0,
                working: 2,
                panes: vec![
                    pane(7, "blocked", Some("pi"), true),
                    pane(8, "working", Some("cargo"), false),
                ],
            }],
        };
        let attention = AttentionSnapshot::snapshot_from_activity(&snapshot);
        assert_eq!(attention.blocked_count, 1);
        assert_eq!(attention.workspaces.len(), 1);
        let workspace = &attention.workspaces[0];
        assert_eq!(workspace.workspace_id, "ws");
        assert_eq!(workspace.path, "/home/u/dev");
        assert_eq!(workspace.blocked, 1);
        assert_eq!(workspace.working, 2);
        assert_eq!(workspace.panes[0].status, PaneAttentionStatus::Blocked);
        assert!(workspace.panes[0].process_is_agent);
        assert_eq!(workspace.panes[0].agent_name.as_deref(), Some("pi"));
        assert!(!workspace.panes[1].process_is_agent);
        assert_eq!(workspace.panes[1].seq, 7);
    }

    /// 不认识的状态字面量按 unknown 处理，但 pane 不能整条丢掉。
    #[test]
    fn unknown_status_keeps_the_pane() {
        let snapshot = ClientActivitySnapshot {
            blocked_count: 1,
            workspaces: vec![ClientWorkspaceAttention {
                workspace_id: "ws".to_string(),
                path: String::new(),
                blocked: 0,
                done: 0,
                working: 0,
                panes: vec![pane(3, "??", None, false)],
            }],
        };
        let attention = AttentionSnapshot::snapshot_from_activity(&snapshot);
        assert_eq!(attention.workspaces[0].panes.len(), 1);
        assert_eq!(
            attention.workspaces[0].panes[0].status,
            PaneAttentionStatus::Unknown
        );
    }
}
