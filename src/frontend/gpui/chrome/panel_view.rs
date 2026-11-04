//! Cmd-P 统一面板：Workspaces / Attention / Search 三个 tab。
//!
//! 对照 macOS `App/UnifiedPanelController.swift` 的交互规则：Tab / Shift+Tab
//! 循环三个 tab，query 跨 tab 保留，Escape 关闭，Enter 激活选中行。
//! 这里只有投影与状态，渲染在 `chrome/panel.rs`。

use crate::frontend::gpui::model::attention::{attention_rows, AttentionRow};
use crate::frontend::gpui::model::panel::{PanelModel, PanelTab, SearchScope};
use crate::frontend::gpui::model::search::{search_rows, SearchHit, SearchSnapshot};
use crate::frontend::gpui::model::sidebar::SidebarItem;

/// 面板一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelRow {
    Workspace {
        workspace_id: String,
        title: String,
        detail: String,
        active: bool,
    },
    Attention {
        workspace_id: String,
        pane_id: u32,
        title: String,
        detail: String,
    },
    Search(SearchHit),
}

impl PanelRow {
    /// 主行文本。
    pub fn title(&self) -> String {
        match self {
            PanelRow::Workspace { title, .. } => title.clone(),
            PanelRow::Attention { title, .. } => title.clone(),
            PanelRow::Search(hit) => {
                if hit.line.trim().is_empty() {
                    format!("pane {}", hit.pane_id)
                } else {
                    hit.line.clone()
                }
            }
        }
    }

    /// 次行文本；搜索命中没有次行。
    pub fn detail(&self) -> Option<String> {
        match self {
            PanelRow::Workspace { detail, .. } => Some(detail.clone()),
            PanelRow::Attention { detail, .. } => Some(detail.clone()),
            PanelRow::Search(_) => None,
        }
    }
}

/// 面板模型：打开状态 + 三个 tab 的 model。
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    pub open: bool,
    pub model: PanelModel,
}

impl PanelState {
    /// 打开面板并切到指定 tab。已打开时只切 tab（不是 toggle）。
    pub fn show(&mut self, tab: PanelTab) {
        self.open = true;
        self.model.tab = tab;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Tab / Shift+Tab 循环；query 跨 tab 保留。
    pub fn cycle(&mut self, back: bool) {
        self.model.cycle_tab(back);
    }

    /// 按当前 tab 生成行。
    pub fn rows(
        &self,
        workspaces: &[SidebarItem],
        attention: Option<&crate::frontend::gpui::model::attention::AttentionSnapshot>,
        search: Option<&SearchSnapshot>,
    ) -> Vec<PanelRow> {
        match self.model.tab {
            PanelTab::Workspaces => workspace_rows(workspaces, &self.model.query),
            PanelTab::Attention => match attention {
                Some(snapshot) => attention_panel_rows(snapshot, workspaces, &self.model.query),
                None => Vec::new(),
            },
            PanelTab::Search => match search {
                Some(snapshot) => {
                    let (hits, _) = search_rows(snapshot, &self.model.query);
                    hits.into_iter().map(PanelRow::Search).collect()
                }
                None => Vec::new(),
            },
        }
    }
}

/// Workspaces tab：标题匹配 `name` 或 `runtime @ transport`。
pub fn workspace_rows(workspaces: &[SidebarItem], query: &str) -> Vec<PanelRow> {
    let q = query.trim().to_lowercase();
    workspaces
        .iter()
        .filter(|workspace| {
            q.is_empty()
                || workspace.display_title().to_lowercase().contains(&q)
                || workspace.runtime.to_lowercase().contains(&q)
                || workspace.transport.to_lowercase().contains(&q)
        })
        .map(|workspace| PanelRow::Workspace {
            workspace_id: workspace.workspace_id.clone(),
            title: workspace.display_title(),
            detail: format!("{} @ {}", workspace.runtime, workspace.transport),
            active: workspace.is_active,
        })
        .collect()
}

/// Attention tab：行文本与侧栏对齐。
pub fn attention_panel_rows(
    snapshot: &crate::frontend::gpui::model::attention::AttentionSnapshot,
    workspaces: &[SidebarItem],
    query: &str,
) -> Vec<PanelRow> {
    attention_rows(snapshot, workspaces, query)
        .into_iter()
        .map(attention_row)
        .collect()
}

fn attention_row(row: AttentionRow) -> PanelRow {
    let detail = crate::frontend::gpui::model::attention::AttentionRowLabel::detail(
        row.pane.status,
        &row.agent_name,
        row.tab_number,
    );
    PanelRow::Attention {
        workspace_id: row.workspace_id,
        pane_id: row.pane.pane_id,
        title: crate::frontend::gpui::model::attention::AttentionRowLabel::sidebar_aligned(
            &row.workspace_name,
            row.pane.status,
            &row.agent_name,
            row.tab_number,
        ),
        detail,
    }
}

/// 搜索范围切到指定 tab 时用哪个 scope（macOS 里 Search tab 默认整库）。
pub fn scope_for(tab: PanelTab) -> SearchScope {
    match tab {
        PanelTab::Workspaces => SearchScope::Workspace,
        PanelTab::Attention => SearchScope::Pane,
        PanelTab::Search => SearchScope::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::attention::{
        AttentionSnapshot, PaneAttention, PaneAttentionStatus, WorkspaceAttention,
    };

    fn workspace(id: &str, name: &str, active: bool) -> SidebarItem {
        SidebarItem::new(id, name).with(|item| {
            item.runtime = "tmux".into();
            item.transport = "local".into();
            item.is_active = active;
        })
    }

    /// 对照 `UnifiedPanelController`: 打开时指定 tab，Tab 循环，query 保留。
    #[test]
    fn opening_and_cycling_keeps_the_query() {
        let mut state = PanelState::default();
        assert!(!state.open);
        state.show(PanelTab::Workspaces);
        assert!(state.open);
        assert_eq!(state.model.tab, PanelTab::Workspaces);

        state.model.query = "dev".into();
        state.cycle(false);
        assert_eq!(state.model.tab, PanelTab::Attention);
        assert_eq!(state.model.query, "dev", "query 跨 tab 保留");

        state.cycle(true);
        assert_eq!(state.model.tab, PanelTab::Workspaces);
        state.close();
        assert!(!state.open);
    }

    /// Workspaces tab 按标题 / runtime / transport 过滤。
    #[test]
    fn workspace_rows_filter_on_title_runtime_and_transport() {
        let mut remote = workspace("remote", "work", false);
        remote.transport = "ryzen".into();
        let workspaces = [workspace("local", "dev", true), remote];
        let state = PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Workspaces,
                query: "ryzen".into(),
                ..PanelModel::default()
            },
        };
        let rows = state.rows(&workspaces, None, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title(), "work · ryzen");
        assert_eq!(rows[0].detail().as_deref(), Some("tmux @ ryzen"));

        // 空 query 全部返回，且保留顺序。
        let all = workspace_rows(&workspaces, "");
        assert_eq!(all.len(), 2);
        assert!(matches!(&all[0], PanelRow::Workspace { active: true, .. }));
    }

    /// Attention tab：行文本与侧栏一致，按状态排序。
    #[test]
    fn attention_rows_use_the_sidebar_alignment() {
        let workspaces = [workspace("ws", "dev", true).with(|item| {
            item.tab_number_by_pane.insert(7, 2);
        })];
        let snapshot = AttentionSnapshot {
            blocked_count: 1,
            workspaces: vec![WorkspaceAttention {
                workspace_id: "ws".to_string(),
                name: "dev".to_string(),
                transport: "ryzen".to_string(),
                ..WorkspaceAttention::new(
                    "ws",
                    vec![
                        PaneAttention {
                            pane_id: 7,
                            status: PaneAttentionStatus::Blocked,
                            process_name: Some("cargo".into()),
                            last_line: "waiting".into(),
                            seq: 5,
                            ..PaneAttention::default()
                        },
                        PaneAttention {
                            pane_id: 8,
                            status: PaneAttentionStatus::Working,
                            process_name: Some("vim".into()),
                            seq: 6,
                            ..PaneAttention::default()
                        },
                    ],
                )
            }],
        };
        let state = PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Attention,
                ..PanelModel::default()
            },
        };
        let rows = state.rows(&workspaces, Some(&snapshot), None);
        assert_eq!(rows.len(), 2);
        // blocked 排在 working 前。
        assert!(rows[0].title().contains("dev"));
        assert!(rows[0].title().contains("blocked"));
        assert!(rows[0].title().contains("vim") || rows[0].title().contains("cargo"));
        assert!(rows[1].title().contains("working"));
    }

    /// Attention tab 没有快照时是空列表，不是 panic。
    #[test]
    fn attention_tab_without_a_snapshot_is_empty() {
        let state = PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Attention,
                ..PanelModel::default()
            },
        };
        assert!(state.rows(&[], None, None).is_empty());
    }

    /// Search tab：命中行来自 `search_rows`，query 过滤生效。
    #[test]
    fn search_rows_come_from_the_search_model() {
        let snapshot = SearchSnapshot::decode(
            br#"{"ok":true,"hits":[
                {"workspace_id":"ws","tab_id":1,"pane_id":7,"seq":12,"line":"cargo test failed"},
                {"workspace_id":"ws","tab_id":1,"pane_id":8,"seq":13,"line":"all good"}
            ]}"#,
        )
        .expect("snapshot");
        let mut state = PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Search,
                query: "failed".into(),
                ..PanelModel::default()
            },
        };
        let rows = state.rows(&[], None, Some(&snapshot));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title(), "cargo test failed");
        assert_eq!(rows[0].detail(), None);

        state.model.query.clear();
        assert_eq!(state.rows(&[], None, Some(&snapshot)).len(), 2);
    }

    /// 每个 tab 对应一个搜索范围。
    #[test]
    fn each_tab_maps_to_its_scope() {
        assert_eq!(scope_for(PanelTab::Workspaces), SearchScope::Workspace);
        assert_eq!(scope_for(PanelTab::Attention), SearchScope::Pane);
        assert_eq!(scope_for(PanelTab::Search), SearchScope::All);
    }
}
