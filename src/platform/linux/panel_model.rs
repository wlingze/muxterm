//! 三 tab 面板的纯逻辑模型（LINUX-PLAN §10 C3.1）。
//!
//! 无 GTK 依赖：tab 切换、query 保留、Tab1 工作区过滤/状态标记、
//! Tab2 注意力排序、Tab3 搜索占位。GTK 层只负责渲染。

use std::collections::HashMap;

use crate::core::attention::engine::PaneAttention;
use crate::core::attention::state::PaneStatus;
use crate::platform::linux::quickconnect_panel::{filter_panel_items, PanelItem};
use crate::platform::linux::workspace_sidebar::{ActivityIndicator, AgentSidebarItem};

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
    pub status: Option<PaneStatus>,
}

/// Tab2 行：注意力 pane。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionRow {
    pub attention: PaneAttention,
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

impl From<crate::core::workspace::workspace::SearchHit> for SearchRow {
    fn from(hit: crate::core::workspace::workspace::SearchHit) -> Self {
        Self {
            workspace_id: hit.workspace_id,
            tab_id: hit.tab_id.0,
            pane_id: hit.pane_id.0,
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
    status_of: impl Fn(&PanelItem) -> Option<PaneStatus>,
) -> Vec<WorkspaceRow> {
    filter_panel_items(items, query)
        .into_iter()
        .map(|item| WorkspaceRow {
            status: status_of(&item),
            item,
        })
        .collect()
}

/// Tab2 过滤：query 匹配工作区名/进程/last_line；保留所有 Working，
/// 以及尚未确认的 Blocked/Done。顺序沿用调用方的稳定 pane 顺序。
pub fn filter_attention_rows(panes: &[PaneAttention], query: &str) -> Vec<AttentionRow> {
    let q = query.trim().to_lowercase();
    panes
        .iter()
        .filter(|pane| {
            pane.status == PaneStatus::Working
                || (matches!(pane.status, PaneStatus::Blocked | PaneStatus::Done)
                    && !pane.acknowledged)
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
        .collect()
}

/// 给 Attention 中仍可见的命令补上 agent 的 title/detail。
/// 已读 agent 只留在侧栏，不再被无条件并入 Attention。
pub fn filter_attention_panel_rows(
    agents: &[AgentSidebarItem],
    panes: &[PaneAttention],
    query: &str,
) -> Vec<AttentionPanelRow> {
    let agent_by_key: HashMap<(String, u32), &AgentSidebarItem> = agents
        .iter()
        .map(|agent| ((agent.workspace_id.replica_id(), agent.pane_id), agent))
        .collect();
    let q = query.trim().to_lowercase();
    filter_attention_rows(panes, "")
        .into_iter()
        .filter_map(|row| {
            let attention = row.attention;
            let key = (attention.workspace_id.clone(), attention.pane_id);
            let (title, detail) = if let Some(agent) = agent_by_key.get(&key) {
                (agent.title.clone(), agent.detail.clone())
            } else {
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
                (title, detail)
            };
            let matches = q.is_empty()
                || attention.workspace_id.to_lowercase().contains(&q)
                || title.to_lowercase().contains(&q)
                || detail.to_lowercase().contains(&q)
                || attention.last_line.to_lowercase().contains(&q);
            matches.then_some(AttentionPanelRow {
                workspace_id: attention.workspace_id,
                pane_id: attention.pane_id,
                title,
                detail,
                indicator: match attention.status {
                    PaneStatus::Working => ActivityIndicator::Running,
                    PaneStatus::Blocked | PaneStatus::Done => ActivityIndicator::Done,
                    _ => ActivityIndicator::None,
                },
            })
        })
        .collect()
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
    use std::time::Instant;

    use crate::core::workspace::id::WorkspaceId;
    use crate::platform::linux::workspace_sidebar::{ActivityIndicator, AgentSidebarItem};

    fn attention(ws: &str, pane: u32, status: PaneStatus, seq: u64) -> PaneAttention {
        PaneAttention {
            workspace_id: ws.into(),
            pane_id: pane,
            status,
            acknowledged: false,
            last_line: format!("line-{pane}"),
            seq,
            process_name: Some("cat".into()),
            process_is_agent: false,
            agent_name: None,
            shell_name: Some("zsh".into()),
            mute_until: None,
            last_regex_eval: Instant::now(),
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
    fn attention_shows_running_and_unread_done_but_hides_read_items() {
        let mut read_done = attention("ws-read", 5, PaneStatus::Done, 5);
        read_done.acknowledged = true;
        let rows = filter_attention_rows(
            &[
                attention("ws-a", 1, PaneStatus::Done, 1),
                attention("ws-b", 2, PaneStatus::Blocked, 2),
                attention("ws-c", 3, PaneStatus::Working, 3),
                attention("ws-d", 4, PaneStatus::Idle, 4),
                read_done,
            ],
            "",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].attention.workspace_id, "ws-a");
        assert_eq!(rows[1].attention.workspace_id, "ws-b");
        assert_eq!(rows[2].attention.workspace_id, "ws-c");
    }

    #[test]
    fn attention_query_filters_by_workspace_process_line() {
        let rows = filter_attention_rows(
            &[
                attention("legion", 1, PaneStatus::Blocked, 1),
                attention("other", 2, PaneStatus::Blocked, 2),
            ],
            "legion",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attention.workspace_id, "legion");
    }

    #[test]
    fn attention_panel_only_keeps_running_and_unread_commands() {
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
        let mut seen_agent = attention(&second_id.replica_id(), 9, PaneStatus::Done, 2);
        seen_agent.acknowledged = true;
        let panes = vec![
            attention(&first_id.replica_id(), 7, PaneStatus::Working, 1),
            seen_agent,
            attention("plain@local", 11, PaneStatus::Blocked, 3),
        ];

        let rows = filter_attention_panel_rows(&agents, &panes, "");
        assert_eq!(rows.len(), 2, "read agents must leave Attention");
        assert_eq!(rows[0].title, "pi");
        assert_eq!(rows[0].indicator, ActivityIndicator::Running);
        assert_eq!(rows[1].workspace_id, "plain@local");

        let filtered = filter_attention_panel_rows(&agents, &panes, "feature/panel");
        assert!(
            filtered.is_empty(),
            "read agents must stay hidden when queried"
        );
    }

    #[test]
    fn workspace_order_stable_when_status_changes() {
        let items = vec![
            PanelItem::Target(
                crate::platform::linux::quickconnect::model::QuickConnectEntry::new(
                    crate::platform::linux::quickconnect::model::TargetConfig::new(
                        "a",
                        crate::platform::linux::quickconnect::model::TargetRuntime::Tmux,
                        crate::platform::linux::quickconnect::model::TargetTransport::Local,
                        "/tmp/a",
                    ),
                    vec![],
                ),
                false,
            ),
            PanelItem::NewProject,
        ];
        let rows = filter_workspace_rows(&items, "", |_| Some(PaneStatus::Blocked));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, Some(PaneStatus::Blocked));
        assert_eq!(rows[1].status, Some(PaneStatus::Blocked));
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
