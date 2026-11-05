//! Cmd-P 统一面板：Workspaces / Attention / Search 三个 tab。
//!
//! 对照 macOS `App/UnifiedPanelController.swift` 的交互规则：Tab / Shift+Tab
//! 循环三个 tab，query 跨 tab 保留，Escape 关闭，Enter 激活选中行。
//! 这里只有投影与状态，渲染在 `chrome/panel.rs`。

use crate::frontend::gpui::model::attention::{attention_rows, AttentionRow};
use crate::frontend::gpui::model::existing::ExistingRuntime;
use crate::frontend::gpui::model::panel::{PanelModel, PanelTab, SearchScope};
use crate::frontend::gpui::model::quick_connect::{TargetConfig, WorkspaceQuery};
use crate::frontend::gpui::model::search::{search_rows, SearchHit, SearchSnapshot};
use crate::frontend::gpui::model::sidebar::SidebarItem;
use crate::frontend::gpui::model::target::{TargetRuntime, TargetTransport};
use crate::frontend::gpui::model::workspace_picker::{PickerStage, WorkspacePicker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateKind {
    Shells,
    Agents,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction {
    Open,
    Back,
    Host(TargetTransport),
    OpenNewPath,
    OpenThisDirectory,
    Directory(String),
    Create,
}

/// 面板选中行的行内动作。
///
/// 对照 Swift `QuickPanelActions.contextual(for:)`：每个行只给一小串动作，
/// 不是全局命令表。标题用 Swift `panel_action_*` / `attention_*` 的英文文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Open,
    Close,
    Connect,
    Attach,
    Save,
    Edit,
    Delete,
    Jump,
    Hide,
}
impl RowAction {
    pub fn title(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Close => "Close workspace",
            Self::Connect => "Connect",
            Self::Attach => "Attach",
            Self::Save => "Save",
            Self::Edit => "Edit",
            Self::Delete => "Delete",
            Self::Jump => "Jump",
            Self::Hide => "Hide",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActionMenu {
    pub target: PanelRow,
    pub query: String,
    pub selected: usize,
}
impl ActionMenu {
    pub fn rows(&self) -> Vec<RowAction> {
        let actions = match &self.target {
            // 聚合行是前端合成的 workspace：不可关闭，也没有运行时切换。
            PanelRow::Aggregate(_) => vec![RowAction::Open],
            PanelRow::Workspace { .. } => vec![RowAction::Open, RowAction::Close],
            PanelRow::Target { .. } => {
                vec![RowAction::Connect, RowAction::Edit, RowAction::Delete]
            }
            PanelRow::Existing { saved, .. } => {
                if *saved {
                    vec![RowAction::Connect, RowAction::Edit, RowAction::Delete]
                } else {
                    vec![RowAction::Attach, RowAction::Save]
                }
            }
            PanelRow::Attention { .. } => vec![RowAction::Jump, RowAction::Hide],
            // Swift `.search(line)` 只有 Jump。
            PanelRow::Search(_) => vec![RowAction::Jump],
            // Swift 里 .open / .back / .status 这类行没有行内动作。
            PanelRow::Action(_) | PanelRow::Status(_) => vec![],
        };
        let terms: Vec<_> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        actions
            .into_iter()
            .filter(|action| {
                terms
                    .iter()
                    .all(|term| action.title().to_lowercase().contains(term))
            })
            .collect()
    }
}

/// 面板一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelRow {
    Workspace {
        workspace_id: String,
        title: String,
        detail: String,
        active: bool,
        shortcut: Option<usize>,
    },
    Aggregate(AggregateKind),
    Action(PickerAction),
    Target {
        config: TargetConfig,
    },
    Status(String),
    Attention {
        workspace_id: String,
        pane_id: u32,
        title: String,
        detail: String,
    },
    Search(SearchHit),
    /// 现有 runtime：选中后走 Catalog 的 attach。
    Existing {
        index: usize,
        title: String,
        detail: String,
        config: TargetConfig,
        saved: bool,
    },
}

impl PanelRow {
    /// 主行文本。
    pub fn title(&self) -> String {
        match self {
            PanelRow::Aggregate(kind) => match kind {
                AggregateKind::Shells => "Shells",
                AggregateKind::Agents => "Agents",
            }
            .into(),
            PanelRow::Action(action) => match action {
                PickerAction::Open => "Open".into(),
                PickerAction::Back => "‹ Back".into(),
                PickerAction::Host(transport) => transport.label().into(),
                PickerAction::OpenNewPath => "+ Open New Path".into(),
                PickerAction::OpenThisDirectory => "Open This Directory".into(),
                PickerAction::Directory(name) => name.clone(),
                PickerAction::Create => "Create Workspace".into(),
            },
            PanelRow::Target { config } => config.name.clone(),
            PanelRow::Status(text) => text.clone(),
            PanelRow::Workspace { title, .. } => title.clone(),
            PanelRow::Attention { title, .. } => title.clone(),
            PanelRow::Search(hit) => {
                if hit.line.trim().is_empty() {
                    format!("pane {}", hit.pane_id)
                } else {
                    hit.line.clone()
                }
            }
            PanelRow::Existing { title, .. } => title.clone(),
        }
    }

    /// 次行文本；搜索命中没有次行。
    pub fn detail(&self) -> Option<String> {
        match self {
            PanelRow::Aggregate(AggregateKind::Shells) => Some("aggregate @ local + ssh".into()),
            PanelRow::Aggregate(AggregateKind::Agents) => Some("aggregate @ all workspaces".into()),
            PanelRow::Target { config } => Some(format!(
                "{} · {}",
                crate::frontend::gpui::model::quick_connect::QuickConnect::subtitle(config),
                config.path
            )),
            PanelRow::Action(_) | PanelRow::Status(_) => None,
            PanelRow::Workspace { detail, .. } => Some(detail.clone()),
            PanelRow::Attention { detail, .. } => Some(detail.clone()),
            PanelRow::Search(_) => None,
            PanelRow::Existing { detail, .. } => Some(detail.clone()),
        }
    }
}

/// 面板模型：打开状态 + 三个 tab 的 model。
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    pub open: bool,
    pub model: PanelModel,
    /// 键盘选中的行（`rows` 的下标）。
    pub selected: usize,
    pub picker: WorkspacePicker,
    pub actions: Option<ActionMenu>,
}

impl PanelState {
    /// 打开面板并切到指定 tab。已打开时只切 tab（不是 toggle）。
    pub fn show(&mut self, tab: PanelTab) {
        self.open = true;
        self.model.tab = tab;
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.selected = 0;
    }

    /// 上下移动选中行，越界夹住（不环绕：macOS uniﬁed panel 同样不环绕）。
    pub fn move_selection(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as i32 + delta;
        self.selected = next.clamp(0, len as i32 - 1) as usize;
    }

    /// 查询串编辑：可打印字符进 query，任何编辑都把选中打回第一行。
    pub fn push_query(&mut self, text: &str) {
        self.model.query.push_str(text);
        self.selected = 0;
    }

    pub fn backspace_query(&mut self) {
        self.model.query.pop();
        self.selected = 0;
    }

    /// Tab / Shift+Tab 循环；query 跨 tab 保留。
    pub fn cycle(&mut self, back: bool) {
        self.model.cycle_tab(back);
    }

    fn workspace_rows(
        &self,
        workspaces: &[SidebarItem],
        existing: &[ExistingRuntime],
    ) -> Vec<PanelRow> {
        use crate::frontend::gpui::model::workspace_picker::same_target;
        use PickerAction as Action;
        let query = &self.model.query;
        let projects = self.picker.projects.configs();
        let annotate = |rows: Vec<PanelRow>| {
            rows.into_iter()
                .map(|mut row| {
                    if let PanelRow::Existing { config, saved, .. } = &mut row {
                        *saved = self.picker.projects.contains(config);
                    }
                    row
                })
                .collect::<Vec<_>>()
        };
        let mut result = match &self.picker.stage {
            PickerStage::Root => {
                let mut rows = vec![
                    PanelRow::Aggregate(AggregateKind::Shells),
                    PanelRow::Aggregate(AggregateKind::Agents),
                    PanelRow::Action(Action::Open),
                ];
                let regular: Vec<_> = workspaces
                    .iter()
                    .filter(|workspace| workspace.runtime != "shell" && !workspace.is_aggregate())
                    .cloned()
                    .collect();
                rows.extend(workspace_rows(&regular, ""));
                rows.extend(
                    projects
                        .iter()
                        .cloned()
                        .map(|config| PanelRow::Target { config }),
                );
                rows.extend(
                    annotate(existing_rows(existing, ""))
                        .into_iter()
                        .filter(|row| {
                            let PanelRow::Existing { config, .. } = row else {
                                return true;
                            };
                            !projects.iter().any(|project| same_target(project, config))
                        }),
                );
                // 开启搜索后，各来源共用 WorkspaceQuery；空串保持截图中的分区顺序。
                if query.trim().is_empty() {
                    rows
                } else {
                    let parsed = WorkspaceQuery::new(query);
                    let mut scored: Vec<_> =
                        rows.into_iter()
                            .enumerate()
                            .filter_map(|(order, row)| {
                                let score = match &row {
                                    PanelRow::Target { config }
                                    | PanelRow::Existing { config, .. } => parsed.score(config),
                                    PanelRow::Workspace { workspace_id, .. } => workspaces
                                        .iter()
                                        .find(|item| &item.workspace_id == workspace_id)
                                        .and_then(|item| parsed.score(&config_for(item))),
                                    _ => row
                                        .title()
                                        .to_lowercase()
                                        .contains(&query.trim().to_lowercase())
                                        .then_some(10_000),
                                }?;
                                Some((score, order, row))
                            })
                            .collect();
                    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                    scored.into_iter().map(|(_, _, row)| row).collect()
                }
            }
            PickerStage::Hosts => {
                let mut rows = vec![PanelRow::Action(Action::Back)];
                let mut hosts = vec![TargetTransport::Local];
                hosts.extend(
                    self.picker
                        .aliases
                        .iter()
                        .cloned()
                        .map(|name| TargetTransport::Ssh { name }),
                );
                if !query.trim().is_empty()
                    && !hosts
                        .iter()
                        .any(|host| host.label().eq_ignore_ascii_case(query.trim()))
                {
                    hosts.push(TargetTransport::Ssh {
                        name: query.trim().into(),
                    });
                }
                rows.extend(
                    hosts
                        .into_iter()
                        .filter(|host| {
                            host.label()
                                .to_lowercase()
                                .contains(&query.trim().to_lowercase())
                        })
                        .map(|host| PanelRow::Action(Action::Host(host))),
                );
                rows
            }
            PickerStage::Sessions(_) => {
                let mut rows = vec![
                    PanelRow::Action(Action::Back),
                    PanelRow::Action(Action::OpenNewPath),
                ];
                rows.extend(annotate(existing_rows(&self.picker.sessions, query)));
                if self.picker.loading {
                    rows.push(PanelRow::Status("Discovering tmux and Herdr…".into()));
                }
                rows
            }
            PickerStage::Path(_) => {
                let mut rows = vec![
                    PanelRow::Action(Action::Back),
                    PanelRow::Action(Action::OpenThisDirectory),
                ];
                rows.extend(
                    self.picker
                        .path
                        .candidates()
                        .iter()
                        .cloned()
                        .map(|name| PanelRow::Action(Action::Directory(name))),
                );
                rows
            }
            PickerStage::Confirm(_) => vec![
                PanelRow::Action(Action::Back),
                PanelRow::Action(Action::Create),
            ],
        };
        if let Some(error) = &self.picker.error {
            result.push(PanelRow::Status(error.clone()));
        }
        result
    }

    /// 按当前 tab 生成行。
    pub fn rows(
        &self,
        workspaces: &[SidebarItem],
        attention: Option<&crate::frontend::gpui::model::attention::AttentionSnapshot>,
        search: Option<&SearchSnapshot>,
        existing: &[ExistingRuntime],
    ) -> Vec<PanelRow> {
        match self.model.tab {
            PanelTab::Workspaces => self.workspace_rows(workspaces, existing),
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

/// Workspaces tab：查询语言与 `@runtime` / `@local` / `@alias` 过滤都走
/// `WorkspaceQuery`，和 macOS UnifiedPanel 共用一套规则。
pub fn workspace_rows(workspaces: &[SidebarItem], query: &str) -> Vec<PanelRow> {
    let parsed = WorkspaceQuery::new(query);
    let mut scored: Vec<(usize, i64, &SidebarItem)> = workspaces
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            parsed
                .score(&config_for(item))
                .map(|score| (index, score, item))
        })
        .collect();
    scored.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    scored
        .into_iter()
        .map(|(_, _, workspace)| PanelRow::Workspace {
            workspace_id: workspace.workspace_id.clone(),
            title: workspace.display_title(),
            detail: format!("{} @ {}", workspace.runtime, workspace.transport),
            active: workspace.is_active,
            shortcut: workspace.shortcut.map(|number| number as usize),
        })
        .collect()
}

/// Existing 分区：查询串按名字/session/transport 做无关大小写包含。
pub fn existing_rows(existing: &[ExistingRuntime], query: &str) -> Vec<PanelRow> {
    let parsed = WorkspaceQuery::new(query);
    existing
        .iter()
        .enumerate()
        .filter(|(_, runtime)| parsed.matches(&runtime.config()))
        .map(|(index, runtime)| PanelRow::Existing {
            index,
            title: runtime.name.clone(),
            detail: runtime.detail(),
            config: runtime.config(),
            saved: false,
        })
        .collect()
}

/// 侧栏投影里没有 path/session，只能填出查询用得上的那几个字段。
fn config_for(item: &SidebarItem) -> TargetConfig {
    let runtime = TargetRuntime::parse(&item.runtime).unwrap_or(TargetRuntime::Shell);
    let transport = if item.transport == "local" {
        TargetTransport::Local
    } else {
        TargetTransport::Ssh {
            name: item.transport.clone(),
        }
    };
    TargetConfig {
        name: item.name.clone(),
        runtime,
        transport,
        ..TargetConfig::default()
    }
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
            selected: 0,
            ..PanelState::default()
        };
        let rows = state.rows(&workspaces, None, None, &[]);
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
            selected: 0,
            ..PanelState::default()
        };
        let rows = state.rows(&workspaces, Some(&snapshot), None, &[]);
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
            selected: 0,
            ..PanelState::default()
        };
        assert!(state.rows(&[], None, None, &[]).is_empty());
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
            selected: 0,
            ..PanelState::default()
        };
        let rows = state.rows(&[], None, Some(&snapshot), &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title(), "cargo test failed");
        assert_eq!(rows[0].detail(), None);

        state.model.query.clear();
        assert_eq!(state.rows(&[], None, Some(&snapshot), &[]).len(), 2);
    }

    /// `@runtime` 过滤与子序列匹配都来自 WorkspaceQuery。
    #[test]
    fn workspace_query_filters_by_runtime_and_subsequence() {
        let mut tmux = workspace("tmux", "muxterm", true);
        tmux.runtime = "tmux".into();
        let mut shell = workspace("shell", "notes", false);
        shell.runtime = "shell".into();
        shell.transport = "ryzen".into();
        let workspaces = [tmux, shell];

        assert_eq!(workspace_rows(&workspaces, "@tmux").len(), 1);
        assert_eq!(workspace_rows(&workspaces, "@shell").len(), 1);
        assert_eq!(
            workspace_rows(&workspaces, "@local").len(),
            1,
            "只有本地目标命中 @local"
        );
        assert_eq!(workspace_rows(&workspaces, "@ryzen").len(), 1);
        assert_eq!(
            workspace_rows(&workspaces, "mxrm").len(),
            1,
            "子序列也能命中 muxterm"
        );
        assert!(
            workspace_rows(&workspaces, "@tmux notes").is_empty(),
            "条件必须同时满足"
        );
    }

    /// 每个 tab 对应一个搜索范围。
    #[test]
    fn each_tab_maps_to_its_scope() {
        assert_eq!(scope_for(PanelTab::Workspaces), SearchScope::Workspace);
        assert_eq!(scope_for(PanelTab::Attention), SearchScope::Pane);
        assert_eq!(scope_for(PanelTab::Search), SearchScope::All);
    }
}
