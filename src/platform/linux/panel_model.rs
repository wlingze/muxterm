//! 三 tab 面板的纯逻辑模型（LINUX-PLAN §10 C3.1）。
//!
//! 无 GTK 依赖：tab 切换、query 保留、Tab1 工作区过滤/状态标记、
//! Tab2 注意力排序、Tab3 搜索占位。GTK 层只负责渲染。

use crate::core::attention::engine::PaneAttention;
use crate::core::attention::state::PaneStatus;
use crate::platform::linux::quickconnect_panel::{filter_panel_items, PanelItem};

/// 面板 tab。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Workspaces = 0,
    Attention = 1,
    Search = 2,
}

/// 面板状态：当前 tab + 共享 query（切 tab 保留）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelModel {
    pub tab: PanelTab,
    pub query: String,
}

impl PanelModel {
    /// 打开面板，指定初始 tab。
    pub fn open(initial: PanelTab) -> Self {
        Self {
            tab: initial,
            query: String::new(),
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

/// Tab3 结果：replica 搜索命中行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    pub workspace_id: String,
    pub pane_id: u32,
    pub seq: u64,
    pub line: String,
}

impl From<crate::core::replica::SearchHit> for SearchRow {
    fn from(hit: crate::core::replica::SearchHit) -> Self {
        Self {
            workspace_id: hit.workspace_id,
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

/// Tab2 过滤：query 匹配工作区名/进程/last_line；只保留 Blocked/Done；
/// blocked 先于 done；label `待处理 {n}` 由调用方按行数生成。
pub fn filter_attention_rows(panes: &[PaneAttention], query: &str) -> Vec<AttentionRow> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<AttentionRow> = panes
        .iter()
        .filter(|p| matches!(p.status, PaneStatus::Blocked | PaneStatus::Done))
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
    // blocked 先于 done；同状态按 seq 新者优先。
    rows.sort_by(|a, b| {
        let a_blocked = a.attention.status == PaneStatus::Blocked;
        let b_blocked = b.attention.status == PaneStatus::Blocked;
        b_blocked
            .cmp(&a_blocked)
            .then(b.attention.seq.cmp(&a.attention.seq))
    });
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
    use std::time::Instant;

    fn attention(ws: &str, pane: u32, status: PaneStatus, seq: u64) -> PaneAttention {
        PaneAttention {
            workspace_id: ws.into(),
            pane_id: pane,
            status,
            last_line: format!("line-{pane}"),
            seq,
            process_name: Some("cat".into()),
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
    fn attention_sort_blocked_first() {
        let rows = filter_attention_rows(
            &[
                attention("ws-a", 1, PaneStatus::Done, 1),
                attention("ws-b", 2, PaneStatus::Blocked, 2),
                attention("ws-c", 3, PaneStatus::Working, 3),
                attention("ws-d", 4, PaneStatus::Idle, 4),
            ],
            "",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].attention.workspace_id, "ws-b");
        assert_eq!(rows[1].attention.workspace_id, "ws-a");
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
                pane_id: 1,
                seq: 3,
                line: "TOKEN_BODY example".into(),
            },
            SearchRow {
                workspace_id: "muxterm".into(),
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
