//! Cmd-P 统一面板：Workspaces / Attention / Search 三个 tab。
//!
//! 对照 macOS `App/UnifiedPanelController.swift` 的交互规则：Tab / Shift+Tab
//! 循环三个 tab，query 跨 tab 保留，Escape 关闭，Enter 激活选中行。
//! 这里只有投影与状态，渲染在 `chrome/panel.rs`。

use crate::frontend::gpui::model::attention::{attention_rows, AttentionRow};
use crate::frontend::gpui::model::existing::ExistingRuntime;
use crate::frontend::gpui::model::machine_inventory::{MachineInventory, MachinePhase};
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
            PanelRow::Action(_) | PanelRow::Heading { .. } | PanelRow::Status(_) => vec![],
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
        /// 行的状态色：working/done/blocked 由它决定，不靠详情文字猜。
        status: crate::frontend::gpui::model::attention::PaneAttentionStatus,
    },
    Search(SearchHit),
    /// 搜索时按机器分组的标题（本机 / 一个 SSH 别名）。只做分区，不能选中。
    Heading {
        label: String,
        phase: MachinePhase,
    },
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
            PanelRow::Heading { label, .. } => label.clone(),
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
            PanelRow::Action(_) | PanelRow::Heading { .. } | PanelRow::Status(_) => None,
            PanelRow::Workspace { detail, .. } => Some(detail.clone()),
            PanelRow::Attention { detail, .. } => Some(detail.clone()),
            PanelRow::Search(_) => None,
            PanelRow::Existing { detail, .. } => Some(detail.clone()),
        }
    }

    /// 键盘能不能停在这一行。分组标题与状态说明只读。
    pub fn is_selectable(&self) -> bool {
        !matches!(self, PanelRow::Heading { .. } | PanelRow::Status(_))
    }

    /// 两行是不是「同一条」。新结果到达后按它保住选中项。
    ///
    /// `Existing` 的 `index` 会随其他机器回包移动，`saved` 也会被 Project
    /// 列表重算，两者都不能参与身份比较。
    pub fn same_row(&self, other: &Self) -> bool {
        match (self, other) {
            (PanelRow::Existing { config: left, .. }, PanelRow::Existing { config: right, .. }) => {
                left == right
            }
            (PanelRow::Heading { label: left, .. }, PanelRow::Heading { label: right, .. }) => {
                left == right
            }
            (left, right) => left == right,
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
    /// Core 记账的 attach 频率（面板排序唯一数据源；空 = 保持目录顺序）。
    pub usage: Vec<crate::frontend::gpui::model::usage::UsageEntry>,
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
    ///
    /// 分组标题 / 状态说明不是动作行，必须跳过；否则方向键会停在一个按
    /// Enter 什么都没发生的行上。
    pub fn move_selection(&mut self, delta: i32, rows: &[PanelRow]) {
        let selectable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_selectable())
            .map(|(index, _)| index)
            .collect();
        let Some(last) = selectable.len().checked_sub(1) else {
            self.selected = 0;
            return;
        };
        let current = selectable.iter().position(|index| *index == self.selected);
        let next = match current {
            Some(position) => (position as i32 + delta).clamp(0, last as i32) as usize,
            None if delta < 0 => last,
            None => 0,
        };
        self.selected = selectable[next];
    }

    /// 查询串编辑：可打印字符进 query，任何编辑都把选中打回第一行。
    pub fn push_query(&mut self, text: &str) {
        self.model.input.insert(text);
        self.selected = 0;
    }

    pub fn backspace_query(&mut self) {
        self.model.input.backspace();
        self.selected = 0;
    }

    /// 查询串的光标与选区（渲染光标 / 高亮用）。
    pub fn query_segments(&self) -> (String, String, String) {
        self.model.input.segments()
    }

    /// Tab / Shift+Tab 循环；query 跨 tab 保留。
    pub fn cycle(&mut self, back: bool) {
        self.model.cycle_tab(back);
    }

    fn workspace_rows(
        &self,
        workspaces: &[SidebarItem],
        inventory: &MachineInventory,
    ) -> Vec<PanelRow> {
        use crate::frontend::gpui::model::workspace_picker::same_target;
        use PickerAction as Action;
        let query = self.model.query();
        let query = &query;
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
                // 已打开的 workspace **不**按使用频率或活跃度重排：它们带固定编号，
                // 面板顺序必须就是侧栏顺序，也就是 `Cmd-Ctrl-1..9` 的顺序。
                // 一重排，行首的数字就会变成“第几个打开”而位置变成“多常用”，
                // 用户按 1 却跳到别处。频次只用于未打开的 Project / discovery。
                let mut live = workspace_rows(&regular, "");
                live.sort_by_key(|row| match row {
                    PanelRow::Workspace { shortcut, .. } => shortcut.unwrap_or(usize::MAX),
                    _ => usize::MAX,
                });
                let mut project_rows: Vec<_> = projects
                    .iter()
                    .cloned()
                    .map(|config| PanelRow::Target { config })
                    .collect();
                self.sort_by_usage(&mut project_rows);

                // 空查询：保持截图里的分区顺序，不插机器标题。
                if query.trim().is_empty() {
                    rows.extend(live);
                    rows.extend(project_rows);
                    let mut discovered: Vec<_> = annotate(existing_rows(&inventory.rows(), ""))
                        .into_iter()
                        .filter(|row| {
                            let PanelRow::Existing { config, .. } = row else {
                                return true;
                            };
                            !projects.iter().any(|project| same_target(project, config))
                        })
                        .collect();
                    self.sort_by_usage(&mut discovered);
                    rows.extend(discovered);
                    return rows;
                }

                // 搜索：非机器行照旧共用 WorkspaceQuery 打分；机器自己的行留在
                // 机器分组里，否则一条结果会跑到别的机器标题下面。
                let parsed = WorkspaceQuery::new(query);
                let needle = query.trim().to_lowercase();
                let mut scored: Vec<(i64, usize, PanelRow)> = Vec::new();
                for (order, row) in rows.into_iter().chain(live).chain(project_rows).enumerate() {
                    let score = match &row {
                        PanelRow::Target { config } => parsed.score(config),
                        PanelRow::Workspace { workspace_id, .. } => workspaces
                            .iter()
                            .find(|item| &item.workspace_id == workspace_id)
                            .and_then(|item| parsed.score(&config_for(item))),
                        _ => row
                            .title()
                            .to_lowercase()
                            .contains(&needle)
                            .then_some(10_000),
                    };
                    if let Some(score) = score {
                        scored.push((score, order, row));
                    }
                }
                scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                let mut rows: Vec<PanelRow> = scored.into_iter().map(|(_, _, row)| row).collect();
                rows.extend(machine_groups(inventory, &parsed, &projects));
                rows
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

    /// 一行的排序键：活跃优先 + Core 的 attach 频率。
    fn rank_of(&self, row: &PanelRow) -> (bool, Option<(u32, i64)>) {
        use crate::frontend::gpui::model::usage::best_rank;
        match row {
            PanelRow::Workspace {
                workspace_id,
                active,
                ..
            } => (
                *active,
                best_rank(&self.usage, Some(workspace_id), None, None),
            ),
            PanelRow::Target { config } | PanelRow::Existing { config, .. } => (
                false,
                best_rank(
                    &self.usage,
                    config.workspace_id.as_deref(),
                    config.workspace_id.as_deref(),
                    config.persisted_id.as_deref(),
                ),
            ),
            _ => (false, None),
        }
    }

    /// 稳定排序：同分（或都没记录）保持原顺序。
    fn sort_by_usage(&self, rows: &mut [PanelRow]) {
        rows.sort_by(|left, right| {
            let (left_active, left_rank) = self.rank_of(left);
            let (right_active, right_rank) = self.rank_of(right);
            crate::frontend::gpui::model::usage::compare_rows(
                left_active,
                left_rank,
                right_active,
                right_rank,
            )
        });
    }

    /// 按当前 tab 生成行。
    pub fn rows(
        &self,
        workspaces: &[SidebarItem],
        attention: Option<&crate::frontend::gpui::model::attention::AttentionSnapshot>,
        search: Option<&SearchSnapshot>,
        inventory: &MachineInventory,
    ) -> Vec<PanelRow> {
        match self.model.tab {
            PanelTab::Workspaces => self.workspace_rows(workspaces, inventory),
            PanelTab::Attention => match attention {
                Some(snapshot) => attention_panel_rows(snapshot, workspaces, &self.model.query()),
                None => Vec::new(),
            },
            PanelTab::Search => match search {
                Some(snapshot) => {
                    let (hits, _) = search_rows(snapshot, &self.model.query());
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

/// 去掉下面没有可见行的机器标题。
///
/// 「已打开的工作区」会在投影后从列表里删掉；如果一台机器的命中全是已打开
/// 的 session，标题就会孤零零挂在列表里。仍在加载的机器保留标题：它是
/// 「还没检查完、不能报无匹配」的唯一证据。
pub fn drop_empty_headings(rows: Vec<PanelRow>) -> Vec<PanelRow> {
    let keep: Vec<bool> = (0..rows.len())
        .map(|index| match &rows[index] {
            PanelRow::Heading { phase, .. } => {
                phase.is_loading()
                    || rows[index + 1..]
                        .iter()
                        .take_while(|row| !matches!(row, PanelRow::Heading { .. }))
                        .any(|row| matches!(row, PanelRow::Existing { .. }))
            }
            _ => true,
        })
        .collect();
    rows.into_iter()
        .zip(keep)
        .filter_map(|(row, keep)| keep.then_some(row))
        .collect()
}

/// 根页搜索：现有 runtime 按机器分组。
///
/// 标题行只做分区；仍在加载的机器即使还没有结果也必须有标题，这样
/// 「其他机器还没回来」不会被当成「没有匹配结果」。已解析完且没有匹配的
/// 机器整组省略；全部结束仍有机器没检查成功时补一条状态说明，把
/// 「没搜到」和「没检查完」区分开。
fn machine_groups(
    inventory: &MachineInventory,
    parsed: &WorkspaceQuery,
    projects: &[TargetConfig],
) -> Vec<PanelRow> {
    use crate::frontend::gpui::model::workspace_picker::same_target;
    let mut rows = Vec::new();
    let mut flat = 0usize;
    for entry in inventory.entries() {
        let mut matched: Vec<(i64, usize, PanelRow)> = Vec::new();
        for runtime in entry.rows() {
            let index = flat;
            flat += 1;
            let config = runtime.config();
            let Some(score) = parsed.score(&config) else {
                continue;
            };
            // 已保存成 Project 的 runtime 由 Project 行代表，不能出现两次。
            if projects.iter().any(|project| same_target(project, &config)) {
                continue;
            }
            matched.push((
                score,
                index,
                PanelRow::Existing {
                    index,
                    title: runtime.name.clone(),
                    detail: runtime.detail(),
                    saved: false,
                    config,
                },
            ));
        }
        matched.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        if matched.is_empty() && !entry.is_loading() {
            continue;
        }
        rows.push(PanelRow::Heading {
            label: entry.transport.label().to_string(),
            phase: entry.phase.clone(),
        });
        rows.extend(matched.into_iter().map(|(_, _, row)| row));
    }
    if !inventory.is_pending() && inventory.failed_machines() > 0 {
        rows.push(PanelRow::Status(crate::frontend::utils::i18n::tr(
            crate::frontend::utils::i18n::TextKey::PanelMachineUnreachable,
        )));
    }
    rows
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
        status: row.pane.status,
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

    fn usage(
        workspace_id: Option<&str>,
        project_id: Option<&str>,
        count: u32,
        last: i64,
    ) -> crate::frontend::gpui::model::usage::UsageEntry {
        crate::frontend::gpui::model::usage::UsageEntry {
            identity: format!("{workspace_id:?}{project_id:?}"),
            workspace_id: workspace_id.map(str::to_string),
            target_workspace_id: None,
            project_id: project_id.map(str::to_string),
            count,
            last_used_at: last,
        }
    }

    /// 已打开的 workspace 按打开顺序 = 侧栏顺序 = 数字快捷键：1 永远是第一个打开的。
    ///
    /// Core 的 attach 频次**不能**重排这一组：行首数字与位置一旦不一致，
    /// 按 `Cmd-Ctrl-1` 就会跳到用户没预期的行。
    #[test]
    fn opened_workspaces_keep_the_sidebar_order_and_numbers() {
        let state = PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Workspaces,
                ..PanelModel::default()
            },
            usage: vec![
                usage(Some("ws-c"), None, 9, 100),
                usage(Some("ws-b"), None, 2, 10),
            ],
            ..PanelState::default()
        };
        let mut workspaces = vec![
            workspace("ws-a", "a", false),
            workspace("ws-b", "b", false),
            workspace("ws-c", "c", false),
            workspace("ws-d", "d", true),
        ];
        for (offset, item) in workspaces.iter_mut().enumerate() {
            item.shortcut = Some(offset as u32 + 1);
        }
        let rows = state.rows(&workspaces, None, None, &MachineInventory::default());
        let live: Vec<(String, Option<usize>)> = rows
            .iter()
            .skip(3) // Shells / Agents / Open
            .filter_map(|row| match row {
                PanelRow::Workspace {
                    title, shortcut, ..
                } => Some((title.clone(), *shortcut)),
                _ => None,
            })
            .collect();
        assert_eq!(
            live,
            vec![
                ("a · local".to_string(), Some(1)),
                ("b · local".to_string(), Some(2)),
                ("c · local".to_string(), Some(3)),
                ("d · local".to_string(), Some(4)),
            ],
            "打开顺序不能被频次/活跃度打乱：{live:?}"
        );
    }

    /// 保存的 Project 也按 Core 的 attach 频率排；没记录的保持配置顺序。
    #[test]
    fn saved_projects_follow_the_core_attach_frequency() {
        let projects =
            crate::frontend::gpui::model::workspace_picker::ProjectList::new(&serde_json::json!([
                {"id": "p-alpha", "name": "alpha", "path": "/srv/alpha",
                 "runtime": {"id": "tmux"}, "transport": {"id": "local"}},
                {"id": "p-beta", "name": "beta", "path": "/srv/beta",
                 "runtime": {"id": "tmux"}, "transport": {"id": "local"}},
                {"id": "p-gamma", "name": "gamma", "path": "/srv/gamma",
                 "runtime": {"id": "tmux"}, "transport": {"id": "local"}},
            ]));
        let mut picker = crate::frontend::gpui::model::workspace_picker::WorkspacePicker::default();
        picker.projects = projects;
        let state = PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Workspaces,
                ..PanelModel::default()
            },
            picker,
            usage: vec![
                usage(None, Some("p-gamma"), 7, 50),
                usage(None, Some("p-beta"), 3, 90),
            ],
            ..PanelState::default()
        };
        let rows = state.rows(&[], None, None, &MachineInventory::default());
        let titles: Vec<String> = rows
            .iter()
            .filter(|row| matches!(row, PanelRow::Target { .. }))
            .map(|row| row.title())
            .collect();
        assert_eq!(titles, vec!["gamma", "beta", "alpha"], "{titles:?}");
    }

    /// 对照 `UnifiedPanelController`: 打开时指定 tab，Tab 循环，query 保留。
    #[test]
    fn opening_and_cycling_keeps_the_query() {
        let mut state = PanelState::default();
        assert!(!state.open);
        state.show(PanelTab::Workspaces);
        assert!(state.open);
        assert_eq!(state.model.tab, PanelTab::Workspaces);

        state.model.input.set_text("dev");
        state.cycle(false);
        assert_eq!(state.model.tab, PanelTab::Attention);
        assert_eq!(state.model.query(), "dev", "query 跨 tab 保留");

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
                input: crate::frontend::gpui::model::text_input::TextInput::from_text("ryzen"),
                ..PanelModel::default()
            },
            selected: 0,
            ..PanelState::default()
        };
        let rows = state.rows(&workspaces, None, None, &MachineInventory::default());
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
        let rows = state.rows(
            &workspaces,
            Some(&snapshot),
            None,
            &MachineInventory::default(),
        );
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
        assert!(state
            .rows(&[], None, None, &MachineInventory::default())
            .is_empty());
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
                input: crate::frontend::gpui::model::text_input::TextInput::from_text("failed"),
                ..PanelModel::default()
            },
            selected: 0,
            ..PanelState::default()
        };
        let rows = state.rows(&[], None, Some(&snapshot), &MachineInventory::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title(), "cargo test failed");
        assert_eq!(rows[0].detail(), None);

        state.model.input.clear();
        assert_eq!(
            state
                .rows(&[], None, Some(&snapshot), &MachineInventory::default())
                .len(),
            2
        );
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

    /// 已打开的工作区被过滤掉以后，不能留下一个空标题；加载中的标题必须保留。
    #[test]
    fn empty_machine_headings_are_dropped_only_when_loaded() {
        let rows = vec![
            PanelRow::Heading {
                label: "local".into(),
                phase: MachinePhase::Loaded,
            },
            PanelRow::Heading {
                label: "ryzen".into(),
                phase: MachinePhase::Loading,
            },
            PanelRow::Heading {
                label: "slow".into(),
                phase: MachinePhase::Loaded,
            },
            PanelRow::Action(PickerAction::Open),
        ];
        let titles: Vec<String> = drop_empty_headings(rows)
            .iter()
            .map(PanelRow::title)
            .collect();
        assert_eq!(titles, vec!["ryzen", "Open"], "{titles:?}");
    }

    /// 标题下面还有结果时照常保留。
    #[test]
    fn a_machine_heading_with_rows_survives() {
        let rows = vec![
            PanelRow::Heading {
                label: "local".into(),
                phase: MachinePhase::Loaded,
            },
            PanelRow::Existing {
                index: 0,
                title: "alpha".into(),
                detail: String::new(),
                config: TargetConfig::default(),
                saved: false,
            },
            PanelRow::Heading {
                label: "ryzen".into(),
                phase: MachinePhase::Loaded,
            },
        ];
        let titles: Vec<String> = drop_empty_headings(rows)
            .iter()
            .map(PanelRow::title)
            .collect();
        assert_eq!(titles, vec!["local", "alpha"], "{titles:?}");
    }

    fn ssh(name: &str) -> TargetTransport {
        TargetTransport::Ssh { name: name.into() }
    }

    fn discovered(machine: &str, session: &str) -> ExistingRuntime {
        let ssh = machine != "local";
        ExistingRuntime {
            name: session.into(),
            runtime: "tmux".into(),
            transport: if ssh { "ssh".into() } else { "local".into() },
            target: if ssh { machine.into() } else { String::new() },
            session: session.into(),
            socket: None,
            windows: 1,
            attached: false,
            workspace_id: None,
            path: String::new(),
        }
    }

    fn searching(query: &str) -> PanelState {
        PanelState {
            open: true,
            model: PanelModel {
                tab: PanelTab::Workspaces,
                input: crate::frontend::gpui::model::text_input::TextInput::from_text(query),
                ..PanelModel::default()
            },
            ..PanelState::default()
        }
    }

    fn headings(rows: &[PanelRow]) -> Vec<(String, MachinePhase)> {
        rows.iter()
            .filter_map(|row| match row {
                PanelRow::Heading { label, phase } => Some((label.clone(), phase.clone())),
                _ => None,
            })
            .collect()
    }

    /// 报障回归：慢机器还在路上时，搜索不能提前变成「没有匹配」。
    ///
    /// 已完成的机器整组省略；在路上的机器一定要留下标题（渲染带「正在检查」），
    /// 这样列表非空、不会落到空态。
    #[test]
    fn a_loading_machine_keeps_a_heading_when_nothing_matches() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);
        inventory.finish(
            &TargetTransport::Local,
            vec![discovered("local", "muxterm")],
        );

        let rows = searching("zzz").rows(&[], None, None, &inventory);
        assert_eq!(
            headings(&rows),
            vec![("ryzen".to_string(), MachinePhase::Loading)],
            "只有还在加载的机器需要标题：{rows:?}"
        );
        assert!(!rows.is_empty(), "还有机器在跑时不能是空列表");
    }

    /// 搜索按机器分组：每台机器一个标题，结果留在自己的机器下面。
    #[test]
    fn search_groups_each_machine_under_its_own_heading() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);
        inventory.finish(&TargetTransport::Local, vec![discovered("local", "alpha")]);
        inventory.finish(&ssh("ryzen"), vec![discovered("ryzen", "api")]);

        let rows = searching("a").rows(&[], None, None, &inventory);
        let titles: Vec<String> = rows
            .iter()
            .filter(|row| matches!(row, PanelRow::Heading { .. } | PanelRow::Existing { .. }))
            .map(PanelRow::title)
            .collect();
        assert_eq!(titles, vec!["local", "alpha", "ryzen", "api"], "{titles:?}");
        assert!(
            rows.iter()
                .filter(|row| matches!(row, PanelRow::Heading { .. }))
                .all(|row| !row.is_selectable()),
            "机器标题不能被方向键选中"
        );
    }

    /// 全部完成但有一台没检查成功：不能用「没有匹配」冒充结果。
    #[test]
    fn an_unchecked_machine_is_not_reported_as_no_matches() {
        let mut inventory = MachineInventory::default();
        inventory.begin(&[TargetTransport::Local, ssh("ryzen")]);
        inventory.finish(
            &TargetTransport::Local,
            vec![discovered("local", "muxterm")],
        );
        inventory.fail(&ssh("ryzen"), "ssh: connect timed out".into());

        let rows = searching("zzz").rows(&[], None, None, &inventory);
        assert!(headings(&rows).is_empty(), "失败的机器没有命中就不占标题");
        assert!(
            matches!(rows.last(), Some(PanelRow::Status(text))
            if *text == crate::frontend::utils::i18n::tr(
                crate::frontend::utils::i18n::TextKey::PanelMachineUnreachable
            )),
            "没检查完要有说明，不能显示「没有匹配结果」：{rows:?}"
        );
    }

    /// 空查询不插机器标题：保持截图里的分区顺序与编号。
    #[test]
    fn a_plain_root_list_has_no_machine_headings() {
        let inventory = MachineInventory::loaded(vec![
            discovered("ryzen", "api"),
            discovered("local", "muxterm"),
        ]);
        let rows = PanelState::default().rows(&[], None, None, &inventory);
        assert!(headings(&rows).is_empty(), "空查询不分组：{rows:?}");
        assert!(
            rows.iter().any(|row| row.title() == "api")
                && rows.iter().any(|row| row.title() == "muxterm"),
            "所有机器的结果都要在列表里"
        );
    }

    /// 方向键 / 默认落点都要跳过标题与状态行。
    #[test]
    fn selection_skips_headings_and_status_rows() {
        let rows = vec![
            PanelRow::Heading {
                label: "local".into(),
                phase: MachinePhase::Loaded,
            },
            PanelRow::Action(PickerAction::Open),
            PanelRow::Status("done".into()),
        ];
        let mut state = PanelState::default();
        state.move_selection(1, &rows);
        assert_eq!(state.selected, 1);
        state.move_selection(1, &rows);
        assert_eq!(state.selected, 1, "状态说明不是动作行");
        state.move_selection(-1, &rows);
        assert_eq!(state.selected, 1);
    }

    /// 每个 tab 对应一个搜索范围。
    #[test]
    fn each_tab_maps_to_its_scope() {
        assert_eq!(scope_for(PanelTab::Workspaces), SearchScope::Workspace);
        assert_eq!(scope_for(PanelTab::Attention), SearchScope::Pane);
        assert_eq!(scope_for(PanelTab::Search), SearchScope::All);
    }
}
