//! Cmd-P 统一面板：三个 tab 的循环 + 共享 query + 搜索范围收窄。
//!
//! 移植自 `chrome/PanelModel.swift`，对标 Linux `panel_model::PanelTab`。

use super::search::SearchHit;

/// 统一面板的三个 tab。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelTab {
    Workspaces,
    Attention,
    Search,
}

impl PanelTab {
    /// 全部取值，顺序就是 Tab 键的前进方向。
    pub const ALL: [PanelTab; 3] = [PanelTab::Workspaces, PanelTab::Attention, PanelTab::Search];

    pub fn raw_value(self) -> usize {
        match self {
            PanelTab::Workspaces => 0,
            PanelTab::Attention => 1,
            PanelTab::Search => 2,
        }
    }

    fn from_raw(value: usize) -> Option<Self> {
        Self::ALL.get(value).copied()
    }
}

/// 搜索范围：当前 pane / 当前 Workspace / 所有已连接 Workspace。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchScope {
    Pane,
    Workspace,
    All,
}

impl SearchScope {
    /// 对 Core 的全局搜索结果做前端范围收窄。
    ///
    /// Workspace ID 是首选边界；`workspace_pane_ids` 仅作为旧 handle 无法
    /// 提供 Workspace ID 时的兼容回退，避免不同 Workspace 复用 pane 数字
    /// 时串结果。
    pub fn filter(
        self,
        hits: &[SearchHit],
        active_pane: Option<u32>,
        workspace_id: Option<&str>,
        workspace_pane_ids: &[u32],
    ) -> Vec<SearchHit> {
        match self {
            SearchScope::Pane => {
                let Some(active_pane) = active_pane else {
                    return Vec::new();
                };
                hits.iter()
                    .filter(|hit| {
                        hit.pane_id == active_pane
                            && workspace_id
                                .map(|id| hit.workspace_id == id)
                                .unwrap_or(true)
                    })
                    .cloned()
                    .collect()
            }
            SearchScope::Workspace => {
                if let Some(workspace_id) = workspace_id {
                    return hits
                        .iter()
                        .filter(|hit| hit.workspace_id == workspace_id)
                        .cloned()
                        .collect();
                }
                hits.iter()
                    .filter(|hit| workspace_pane_ids.contains(&hit.pane_id))
                    .cloned()
                    .collect()
            }
            SearchScope::All => hits.to_vec(),
        }
    }
}

/// 三 tab 面板纯状态：当前 tab + 共享 query（切 tab 必须保留）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelModel {
    pub tab: PanelTab,
    pub query: String,
    pub scope: SearchScope,
}

impl Default for PanelModel {
    fn default() -> Self {
        Self {
            tab: PanelTab::Workspaces,
            query: String::new(),
            scope: SearchScope::Workspace,
        }
    }
}

impl PanelModel {
    /// 打开面板：指定初始 tab，query 清空。
    pub fn open(initial: PanelTab) -> Self {
        Self {
            tab: initial,
            ..Self::default()
        }
    }

    /// Tab / Shift+Tab 循环：Workspaces → Attention → Search → Workspaces。
    pub fn cycle_tab(&mut self, back: bool) {
        let count = PanelTab::ALL.len();
        let delta = if back { count - 1 } else { 1 };
        let next = (self.tab.raw_value() + delta) % count;
        self.tab = PanelTab::from_raw(next).unwrap_or(PanelTab::Workspaces);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(workspace: &str, tab: u32, pane: u32, seq: u64, line: &str) -> SearchHit {
        SearchHit {
            workspace_id: workspace.to_string(),
            tab_id: tab,
            pane_id: pane,
            seq,
            line: line.to_string(),
        }
    }

    /// 对照 `PanelModelTests.testTabCycleWraps`。
    #[test]
    fn tab_cycle_wraps() {
        let mut model = PanelModel::open(PanelTab::Workspaces);
        model.cycle_tab(false);
        assert_eq!(model.tab, PanelTab::Attention);
        model.cycle_tab(false);
        assert_eq!(model.tab, PanelTab::Search);
        model.cycle_tab(false);
        assert_eq!(model.tab, PanelTab::Workspaces);
        model.cycle_tab(true);
        assert_eq!(model.tab, PanelTab::Search);
    }

    /// 对照 `testQuerySurvivesTabChange`。
    #[test]
    fn query_survives_tab_change() {
        let mut model = PanelModel::open(PanelTab::Workspaces);
        model.query = "legion".to_string();
        model.cycle_tab(false);
        assert_eq!(model.tab, PanelTab::Attention);
        assert_eq!(model.query, "legion");
    }

    /// 对照 `testSearchScopeDefaultsToWorkspaceAndSurvivesTabChange`。
    #[test]
    fn search_scope_defaults_to_workspace_and_survives_tab_change() {
        let mut model = PanelModel::open(PanelTab::Search);
        assert_eq!(model.scope, SearchScope::Workspace);
        model.scope = SearchScope::Pane;
        model.cycle_tab(false);
        model.cycle_tab(true);
        assert_eq!(model.scope, SearchScope::Pane);
    }

    /// 对照 `testSearchScopeFiltersCurrentPaneAndWorkspace`。
    #[test]
    fn search_scope_filters_current_pane_and_workspace() {
        let hits = vec![
            hit("one", 10, 1, 1, "one"),
            hit("one", 10, 2, 2, "two"),
            hit("two", 20, 2, 3, "collision"),
            hit("two", 20, 3, 4, "three"),
        ];
        let pane_ids = [1_u32, 2];
        assert_eq!(
            SearchScope::Pane
                .filter(&hits, Some(2), Some("one"), &pane_ids)
                .iter()
                .map(|h| h.pane_id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            SearchScope::Workspace
                .filter(&hits, Some(2), Some("one"), &pane_ids)
                .iter()
                .map(|h| h.pane_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            SearchScope::All
                .filter(&hits, Some(2), Some("one"), &pane_ids)
                .iter()
                .map(|h| h.pane_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 2, 3]
        );
    }

    /// 没有 workspace_id 时回退到 pane 编号集合，但 pane 范围必须有 active pane。
    #[test]
    fn filter_falls_back_to_pane_ids_without_workspace_id() {
        let hits = vec![hit("one", 10, 1, 1, "one"), hit("two", 20, 2, 2, "two")];
        assert_eq!(
            SearchScope::Workspace
                .filter(&hits, None, None, &[2])
                .iter()
                .map(|h| h.pane_id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(
            SearchScope::Pane
                .filter(&hits, None, Some("one"), &[1, 2])
                .is_empty(),
            "没有 active pane 时 pane 范围为空"
        );
    }
}
