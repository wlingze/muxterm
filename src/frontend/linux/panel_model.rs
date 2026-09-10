//! 三 tab 面板的纯逻辑模型（LINUX-PLAN §10 C3.1）。
//!
//! 无 GTK 依赖：tab 切换、query 保留、Tab1 工作区过滤/状态标记、
//! Tab2 注意力排序、Tab3 搜索占位。GTK 层只负责渲染。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use crate::frontend::ffi_client::{
    ClientAttentionPane, ClientAttentionStatus, ClientOpenRequest, ClientSearchHit,
};
use crate::frontend::i18n::{self, Key as TextKey};
use crate::frontend::linux::quickconnect::existing::ExistingEntry;
use crate::frontend::linux::quickconnect::model::{
    QuickBadge, QuickConnect, QuickConnectEntry, TargetConfig, WorkspaceQuery,
};
use crate::frontend::linux::quickconnect::store::QuickConnectStore;
use crate::frontend::linux::workspace_sidebar::{ActivityIndicator, AgentSidebarItem};
use crate::frontend::ssh_probe::SshReach;

/// QuickConnect 面板的候选项。
///
/// 这是页面模型，不携带 GTK widget；View 只负责把它们渲染成 ListBox 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelItem {
    Target(QuickConnectEntry, bool),
    NewProject,
    /// 目录（已有的连接 / 本地 / SSH）。
    Folder {
        id: &'static str,
        title: String,
    },
    /// 子目录返回。
    Back,
    /// 一条活着的 tmux session 或 Herdr workspace。
    Existing(ExistingEntry),
    /// SSH host 行（探测到至少一条 tmux 或 Herdr）。
    Host {
        alias: String,
    },
    /// SSH 探测中占位。
    Loading,
    /// 空目录占位。
    Empty {
        title: String,
    },
}

/// 已有连接面板的纯逻辑导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExistingNav {
    #[default]
    Root,
    Home,
    Local,
    SshHosts,
    SshHost {
        alias: String,
    },
}

/// 已有连接面板共享状态。
///
/// Window 侧负责更新探测结果，面板 View 只读取这份 owned snapshot 并重建行。
#[derive(Debug, Clone, Default)]
pub struct ExistingPanelState {
    pub nav: ExistingNav,
    pub locals: Vec<ExistingEntry>,
    pub hosts: Vec<String>,
    pub remote: HashMap<String, Vec<ExistingEntry>>,
    /// SSH config 中的全部 alias（即使该 host 当前没有可连接 workspace，
    /// 也要能用于 `@alias` 补全）。
    pub ssh_aliases: Vec<String>,
    /// SSH 探测是否在跑：空 host + inflight → Loading；空 + 完成 → Empty。
    pub probe_inflight: bool,
}

type SearchCallback = Box<dyn Fn(&str, SearchScope) -> Vec<SearchRow>>;
type MuteCallback = Box<dyn Fn(String, u32, Duration)>;

/// QuickConnect View 的输入快照与业务回调契约。
///
/// 该结构只包含 owned DTO、模型值和闭包，不依赖 GTK；View 负责消费它并
/// 将用户手势转发给这些回调。
pub struct PanelShowArgs {
    pub initial_tab: PanelTab,
    pub workspaces: Vec<PanelItem>,
    /// 非空搜索时追加的完整 Recent/Project 候选。
    pub workspace_search_items: Vec<PanelItem>,
    pub agents: Vec<AgentSidebarItem>,
    pub attention: Vec<ClientAttentionPane>,
    pub on_connect: Box<dyn Fn(ClientOpenRequest)>,
    pub on_existing_connect: Box<dyn Fn(ClientOpenRequest)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
    pub on_jump_pane: Box<dyn Fn(String, u32, u64)>,
    pub on_mute: MuteCallback,
    pub search: SearchCallback,
    pub on_close: Box<dyn Fn()>,
    pub ssh_reach: HashMap<String, SshReach>,
    pub existing: Rc<RefCell<ExistingPanelState>>,
    pub on_existing_nav: Box<dyn Fn(ExistingNav)>,
}

pub fn build_items(store: &QuickConnectStore, current: Option<&TargetConfig>) -> Vec<PanelItem> {
    build_items_with_recent_limit(store, current, 5)
}

fn build_items_with_recent_limit(
    store: &QuickConnectStore,
    current: Option<&TargetConfig>,
    recent_limit: usize,
) -> Vec<PanelItem> {
    let current_id = current.map(QuickConnect::unique_id);
    let mut items: Vec<PanelItem> =
        QuickConnect::entries(&store.recents, &store.projects, recent_limit)
            .into_iter()
            .map(|mut entry| {
                let is_current = current_id
                    .as_ref()
                    .is_some_and(|id| QuickConnect::unique_id(&entry.config) == *id);
                if !entry.badges.contains(&QuickBadge::Recent) {
                    entry.project_id = store.project_id_for(&entry.config);
                }
                PanelItem::Target(entry, is_current)
            })
            .collect();
    items.push(PanelItem::NewProject);
    items
}

/// 搜索用的完整 Recent/Project 集合；空 query 的展示仍由 `build_items` 保持
/// 紧凑，只在用户开始输入时把这里的隐藏 Recent 合并进来。
pub fn build_search_items(
    store: &QuickConnectStore,
    current: Option<&TargetConfig>,
) -> Vec<PanelItem> {
    build_items_with_recent_limit(store, current, store.recents.len())
}

/// W20b：根列表 = 第一项「已有的连接」Folder + 原 Recent/Project + New Project。
pub fn build_root_items(
    store: &QuickConnectStore,
    current: Option<&TargetConfig>,
) -> Vec<PanelItem> {
    let mut items = vec![PanelItem::Folder {
        id: "existing-connections",
        title: i18n::tr(TextKey::ExistingConnections),
    }];
    items.extend(build_items(store, current));
    items
}

/// 把已有连接压平成根查询候选。空 query 时仍只显示 Folder，避免改变
/// 原有的工作区入口；用户开始搜索后才把 local/SSH 的 Existing 行并入结果。
pub fn existing_root_items(existing: &ExistingPanelState) -> Vec<PanelItem> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut entries = existing.locals.clone();
    let mut aliases: Vec<&String> = existing.remote.keys().collect();
    aliases.sort();
    for alias in aliases {
        if let Some(rows) = existing.remote.get(alias) {
            entries.extend(rows.iter().cloned());
        }
    }
    entries.sort_by_key(|entry| QuickConnect::unique_id(&entry.target_config()));
    for entry in entries {
        let id = QuickConnect::unique_id(&entry.target_config());
        if seen.insert(id) {
            result.push(PanelItem::Existing(entry));
        }
    }
    result
}

/// 根目录搜索候选：Recent/Project 仍保持原顺序，已存在连接仅在用户
/// 输入查询后并入，避免空面板被大量 runtime 行挤满。
pub fn root_items_with_existing(
    base: &[PanelItem],
    existing: &ExistingPanelState,
    query: &str,
) -> Vec<PanelItem> {
    root_items_with_existing_and_search(base, &[], existing, query)
}

/// 根目录搜索候选的完整版本：非空 query 时合并完整 Recent/Project 集合，
/// 再追加 Existing workspace；空 query 仍只返回紧凑的 `base`。
pub fn root_items_with_existing_and_search(
    base: &[PanelItem],
    search_base: &[PanelItem],
    existing: &ExistingPanelState,
    query: &str,
) -> Vec<PanelItem> {
    let mut items = base.to_vec();
    if query.trim().is_empty() {
        return items;
    }
    let mut seen: HashSet<String> = items
        .iter()
        .filter_map(|item| match item {
            PanelItem::Target(entry, _) => Some(QuickConnect::unique_id(&entry.config)),
            _ => None,
        })
        .collect();
    for item in search_base {
        let PanelItem::Target(entry, _) = item else {
            continue;
        };
        if seen.insert(QuickConnect::unique_id(&entry.config)) {
            items.push(item.clone());
        }
    }
    let root_existing = existing_root_items(existing);
    if root_existing.is_empty() && existing.probe_inflight {
        items.push(PanelItem::Loading);
    }
    for item in root_existing {
        let PanelItem::Existing(entry) = &item else {
            continue;
        };
        if seen.insert(QuickConnect::unique_id(&entry.target_config())) {
            items.push(item);
        }
    }
    items
}

/// W20c：已有的连接子目录内容（纯函数，可单测）。
///
/// - Home：Back + Local + SSH 两个 Folder
/// - Local：Back + 本地 tmux/Herdr 行（空 → Empty）
/// - SshHosts：Back + 探测到的 Host 行（探测中 → Loading）
/// - SshHost{alias}：Back + 该 host 的 tmux/Herdr 行
pub fn existing_items(
    nav: ExistingNav,
    locals: &[ExistingEntry],
    hosts: &[String],
    probe_inflight: bool,
    remote_of_alias: impl Fn(&str) -> Vec<ExistingEntry>,
) -> Vec<PanelItem> {
    let mut items = vec![PanelItem::Back];
    match nav {
        ExistingNav::Root | ExistingNav::Home => {
            // C9：扁平 runtime list。locals + 每个 connect 的远端行，双份不去重。
            let mut rows: Vec<ExistingEntry> = locals.to_vec();
            for host in hosts {
                rows.extend(remote_of_alias(host));
            }
            if rows.is_empty() {
                if probe_inflight {
                    items.push(PanelItem::Loading);
                } else {
                    items.push(PanelItem::Empty {
                        title: i18n::tr(TextKey::ExistingEmpty),
                    });
                }
            } else {
                items.extend(rows.into_iter().map(PanelItem::Existing));
            }
        }
        ExistingNav::Local => {
            if locals.is_empty() {
                items.push(PanelItem::Empty {
                    title: i18n::tr(TextKey::ExistingEmpty),
                });
            } else {
                items.extend(locals.iter().cloned().map(PanelItem::Existing));
            }
        }
        ExistingNav::SshHosts => {
            if hosts.is_empty() {
                if probe_inflight {
                    items.push(PanelItem::Loading);
                } else {
                    items.push(PanelItem::Empty {
                        title: i18n::tr(TextKey::ExistingEmpty),
                    });
                }
            } else {
                items.extend(hosts.iter().map(|alias| PanelItem::Host {
                    alias: alias.clone(),
                }));
            }
        }
        ExistingNav::SshHost { alias } => {
            let rows = remote_of_alias(&alias);
            if rows.is_empty() {
                items.push(PanelItem::Empty {
                    title: i18n::tr(TextKey::ExistingEmpty),
                });
            } else {
                items.extend(rows.into_iter().map(PanelItem::Existing));
            }
        }
    }
    items
}

/// 按查询过滤 QuickConnect 候选，并保持原始顺序作为同分排序依据。
pub(crate) fn filter_panel_items(items: &[PanelItem], query: &str) -> Vec<PanelItem> {
    let q = query.trim();
    if q.is_empty() {
        return items.to_vec();
    }
    let parsed = WorkspaceQuery::parse(q);
    let needle = q.to_lowercase();
    let mut matched: Vec<(usize, u32, PanelItem)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let score = match item {
                PanelItem::Target(entry, _) => parsed.score(&entry.config),
                PanelItem::NewProject => {
                    let label = format!(
                        "new project {}",
                        i18n::tr(TextKey::NewProject).to_lowercase()
                    );
                    label.contains(&needle).then_some(0)
                }
                PanelItem::Folder { title, .. } => {
                    title.to_lowercase().contains(&needle).then_some(0)
                }
                PanelItem::Back => Some(0),
                PanelItem::Existing(entry) => parsed.score(&entry.target_config()),
                PanelItem::Host { alias } => parsed.host_score(alias),
                PanelItem::Loading => Some(0),
                PanelItem::Empty { title } => title.to_lowercase().contains(&needle).then_some(0),
            }?;
            Some((index, score, item.clone()))
        })
        .collect();
    matched.sort_by(
        |(left_index, left_score, _), (right_index, right_score, _)| {
            right_score
                .cmp(left_score)
                .then(left_index.cmp(right_index))
        },
    );
    matched.into_iter().map(|(_, _, item)| item).collect()
}

/// 面板 tab。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Workspaces = 0,
    Attention = 1,
    Search = 2,
}

/// 搜索范围（W18f）：当前 pane / 本工作区 / 全部已连接工作区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    #[default]
    All,
    Workspace,
    Pane,
}

pub struct PanelModel {
    pub tab: PanelTab,
    pub query: String,
    pub scope: SearchScope,
}

impl PanelModel {
    /// 打开面板，指定初始 tab。
    pub fn open(initial: PanelTab) -> Self {
        Self {
            tab: initial,
            query: String::new(),
            scope: SearchScope::All,
        }
    }

    /// Tab / Shift+Tab 循环切换。
    pub fn cycle_tab(&mut self, back: bool) {
        let n = 3;
        let delta = if back { n - 1 } else { 1 };
        self.tab = match (self.tab as u8 + delta) % n {
            0 => PanelTab::Workspaces,
            1 => PanelTab::Attention,
            _ => PanelTab::Search,
        };
    }
}

/// Tab1 行：工作区 + 可选状态标记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub item: PanelItem,
    /// 工作区级状态：blocked 显示 `●`，done 显示 `✓`，否则无。
    pub status: Option<ClientAttentionStatus>,
}

/// Tab2 行：注意力 pane。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionRow {
    pub attention: ClientAttentionPane,
}

/// Attention tab 的统一展示行：agent 常驻，其余行保留 Blocked/Done 语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionPanelRow {
    pub workspace_id: String,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: ActivityIndicator,
}

/// Tab3 结果：工作区 PaneBuf 搜索命中行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    pub workspace_id: String,
    pub tab_id: u32,
    pub pane_id: u32,
    pub seq: u64,
    pub line: String,
}

impl From<ClientSearchHit> for SearchRow {
    fn from(hit: ClientSearchHit) -> Self {
        Self {
            workspace_id: hit.workspace_id,
            tab_id: hit.tab_id,
            pane_id: hit.pane_id,
            seq: hit.seq,
            line: hit.line,
        }
    }
}

/// Tab1 过滤：现有 filter_panel_items + 每行状态标记。
/// 顺序固定，不按最近使用重排。
pub fn filter_workspace_rows(
    items: &[PanelItem],
    query: &str,
    status_of: impl Fn(&PanelItem) -> Option<ClientAttentionStatus>,
) -> Vec<WorkspaceRow> {
    filter_panel_items(items, query)
        .into_iter()
        .map(|item| WorkspaceRow {
            status: status_of(&item),
            item,
        })
        .collect()
}

/// Tab2 过滤：保留运行中的命令和未读 Blocked/Done；已读完成项不再出现。
/// 未读 blocked/done 先于 running；同状态按 seq 新者优先。
pub fn filter_attention_rows(panes: &[ClientAttentionPane], query: &str) -> Vec<AttentionRow> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<AttentionRow> = panes
        .iter()
        .filter(|p| match p.status_kind() {
            ClientAttentionStatus::Working => true,
            ClientAttentionStatus::Blocked | ClientAttentionStatus::Done => !p.acknowledged,
            ClientAttentionStatus::Unknown | ClientAttentionStatus::Idle => false,
        })
        .filter(|p| {
            q.is_empty()
                || p.workspace_id.to_lowercase().contains(&q)
                || p.process_name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&q)
                || p.last_line.to_lowercase().contains(&q)
        })
        .cloned()
        .map(|attention| AttentionRow { attention })
        .collect();
    rows.sort_by(|a, b| {
        let rank = |status| match status {
            ClientAttentionStatus::Blocked => 0,
            ClientAttentionStatus::Done => 1,
            ClientAttentionStatus::Working => 2,
            ClientAttentionStatus::Unknown | ClientAttentionStatus::Idle => 3,
        };
        rank(a.attention.status_kind())
            .cmp(&rank(b.attention.status_kind()))
            .then(b.attention.seq.cmp(&a.attention.seq))
    });
    rows
}

/// 合并正在运行/未读完成的 agent 与传统 attention 行。
///
/// 已读 agent 只留在常驻 Agent 侧边栏，不留在 Attention；
/// 同一 pane 即使同时处于 Blocked/Done，也只展示一次 agent 行。
pub fn filter_attention_panel_rows(
    agents: &[AgentSidebarItem],
    panes: &[ClientAttentionPane],
    query: &str,
) -> Vec<AttentionPanelRow> {
    let q = query.trim().to_lowercase();
    let pane_by_key: HashMap<(String, u32), &ClientAttentionPane> = panes
        .iter()
        .map(|pane| ((pane.workspace_id.clone(), pane.pane_id), pane))
        .collect();
    let mut agent_keys = HashSet::new();
    let mut rows = Vec::new();

    for agent in agents {
        if agent.indicator == ActivityIndicator::None {
            continue;
        }
        let workspace_id = agent.workspace_id.replica_id();
        let key = (workspace_id.clone(), agent.pane_id);
        agent_keys.insert(key.clone());
        let linked = pane_by_key.get(&key).copied();
        let matches = q.is_empty()
            || workspace_id.to_lowercase().contains(&q)
            || agent.title.to_lowercase().contains(&q)
            || agent.detail.to_lowercase().contains(&q)
            || linked.is_some_and(|pane| {
                pane.process_name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&q)
                    || pane.last_line.to_lowercase().contains(&q)
            });
        if matches {
            rows.push(AttentionPanelRow {
                workspace_id,
                pane_id: agent.pane_id,
                title: agent.title.clone(),
                detail: agent.detail.clone(),
                indicator: agent.indicator,
            });
        }
    }

    rows.extend(
        filter_attention_rows(panes, query)
            .into_iter()
            .filter(|row| {
                !agent_keys.contains(&(row.attention.workspace_id.clone(), row.attention.pane_id))
            })
            .map(|row| {
                let attention = row.attention;
                let title = attention
                    .process_name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or("Command")
                    .to_string();
                let detail = if attention.last_line.trim().is_empty() {
                    attention.workspace_id.clone()
                } else {
                    format!("{} · {}", attention.workspace_id, attention.last_line)
                };
                let indicator = match attention.status_kind() {
                    ClientAttentionStatus::Working => ActivityIndicator::Running,
                    ClientAttentionStatus::Blocked | ClientAttentionStatus::Done => {
                        ActivityIndicator::Done
                    }
                    ClientAttentionStatus::Unknown | ClientAttentionStatus::Idle => {
                        ActivityIndicator::None
                    }
                };
                AttentionPanelRow {
                    workspace_id: attention.workspace_id,
                    pane_id: attention.pane_id,
                    title,
                    detail,
                    indicator,
                }
            }),
    );
    rows
}

/// Tab3：按 query 过滤命中行；空结果返回占位 flag。
pub fn search_rows(query: &str, hits: Vec<SearchRow>) -> (Vec<SearchRow>, bool) {
    let q = query.trim().to_lowercase();
    let rows: Vec<SearchRow> = hits
        .into_iter()
        .filter(|h| {
            q.is_empty()
                || h.line.to_lowercase().contains(&q)
                || h.workspace_id.to_lowercase().contains(&q)
        })
        .collect();
    let placeholder = rows.is_empty();
    (rows, placeholder)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::frontend::linux::workspace_sidebar::{ActivityIndicator, AgentSidebarItem};
    use muxterm_protocol::WorkspaceId;

    fn attention(
        ws: &str,
        pane: u32,
        status: ClientAttentionStatus,
        seq: u64,
    ) -> ClientAttentionPane {
        ClientAttentionPane {
            workspace_id: ws.into(),
            pane_id: pane,
            status: format!("{status:?}").to_lowercase(),
            acknowledged: false,
            last_line: format!("line-{pane}"),
            seq,
            process_name: Some("cat".into()),
            process_is_agent: false,
            agent_name: None,
            shell_name: Some("zsh".into()),
        }
    }

    #[test]
    fn panel_tab_cycle_wraps() {
        let mut m = PanelModel::open(PanelTab::Workspaces);
        m.cycle_tab(false);
        assert_eq!(m.tab, PanelTab::Attention);
        m.cycle_tab(false);
        assert_eq!(m.tab, PanelTab::Search);
        m.cycle_tab(false);
        assert_eq!(m.tab, PanelTab::Workspaces);
        m.cycle_tab(true);
        assert_eq!(m.tab, PanelTab::Search);
    }

    #[test]
    fn query_survives_tab_change() {
        let mut m = PanelModel::open(PanelTab::Workspaces);
        m.query = "legion".into();
        m.cycle_tab(false);
        assert_eq!(m.query, "legion");
    }

    #[test]
    fn attention_keeps_running_and_unread_done_but_hides_read_items() {
        let mut read = attention("ws-e", 5, ClientAttentionStatus::Done, 5);
        read.acknowledged = true;
        let rows = filter_attention_rows(
            &[
                attention("ws-a", 1, ClientAttentionStatus::Done, 1),
                attention("ws-b", 2, ClientAttentionStatus::Blocked, 2),
                attention("ws-c", 3, ClientAttentionStatus::Working, 3),
                attention("ws-d", 4, ClientAttentionStatus::Idle, 4),
                read,
            ],
            "",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].attention.workspace_id, "ws-b");
        assert_eq!(rows[1].attention.workspace_id, "ws-a");
        assert_eq!(rows[2].attention.workspace_id, "ws-c");
    }

    #[test]
    fn attention_query_filters_by_workspace_process_line() {
        let rows = filter_attention_rows(
            &[
                attention("legion", 1, ClientAttentionStatus::Blocked, 1),
                attention("other", 2, ClientAttentionStatus::Blocked, 2),
            ],
            "legion",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attention.workspace_id, "legion");
    }

    #[test]
    fn attention_panel_keeps_running_and_unread_agents_without_duplicates() {
        let first_id = WorkspaceId::new("local", None, "alpha", "tmux", "/work/alpha");
        let second_id = WorkspaceId::new("local", None, "beta", "tmux", "/work/beta");
        let agents = vec![
            AgentSidebarItem {
                workspace_id: first_id.clone(),
                pane_id: 7,
                title: "pi".into(),
                detail: "/work/alpha · main".into(),
                indicator: ActivityIndicator::Running,
            },
            AgentSidebarItem {
                workspace_id: second_id.clone(),
                pane_id: 9,
                title: "codex".into(),
                detail: "/work/beta · feature/panel".into(),
                indicator: ActivityIndicator::None,
            },
        ];
        let mut seen_agent = attention(&second_id.replica_id(), 9, ClientAttentionStatus::Done, 2);
        seen_agent.acknowledged = true;
        let panes = vec![
            attention(&first_id.replica_id(), 7, ClientAttentionStatus::Working, 1),
            seen_agent,
            attention("plain@local", 11, ClientAttentionStatus::Blocked, 3),
        ];

        let rows = filter_attention_panel_rows(&agents, &panes, "");
        assert_eq!(
            rows.len(),
            2,
            "read agents leave Attention and panes must not duplicate"
        );
        assert_eq!(rows[0].title, "pi");
        assert_eq!(rows[0].indicator, ActivityIndicator::Running);
        assert_eq!(rows[1].workspace_id, "plain@local");

        let filtered = filter_attention_panel_rows(&agents, &panes, "feature/panel");
        assert!(
            filtered.is_empty(),
            "read agent must stay out of Attention even when queried"
        );
    }

    #[test]
    fn workspace_order_stable_when_status_changes() {
        let items = vec![
            PanelItem::Target(
                crate::frontend::linux::quickconnect::model::QuickConnectEntry::new(
                    crate::frontend::linux::quickconnect::model::TargetConfig::new(
                        "a",
                        crate::frontend::linux::quickconnect::model::TargetRuntime::Tmux,
                        crate::frontend::linux::quickconnect::model::TargetTransport::Local,
                        "/tmp/a",
                    ),
                    vec![],
                ),
                false,
            ),
            PanelItem::NewProject,
        ];
        let rows = filter_workspace_rows(&items, "", |_| Some(ClientAttentionStatus::Blocked));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, Some(ClientAttentionStatus::Blocked));
        assert_eq!(rows[1].status, Some(ClientAttentionStatus::Blocked));
        // 顺序固定：Target 仍在 NewProject 前。
        assert!(matches!(rows[0].item, PanelItem::Target(_, _)));
        assert!(matches!(rows[1].item, PanelItem::NewProject));
    }

    #[test]
    fn search_rows_filters_hits_and_flags_empty() {
        let hits = vec![
            SearchRow {
                workspace_id: "legion".into(),
                tab_id: 1,
                pane_id: 1,
                seq: 3,
                line: "TOKEN_BODY example".into(),
            },
            SearchRow {
                workspace_id: "muxterm".into(),
                tab_id: 2,
                pane_id: 2,
                seq: 7,
                line: "build ok".into(),
            },
        ];
        let (rows, placeholder) = search_rows("TOKEN_BODY", hits.clone());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pane_id, 1);
        assert!(!placeholder);

        let (rows, placeholder) = search_rows("missing", hits);
        assert!(rows.is_empty());
        assert!(placeholder);
    }
}
