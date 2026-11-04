//! 侧栏四段的投影：工作区 / Agents / Commands / Hidden Commands。
//!
//! 对照 macOS `App/WorkspaceSidebar.swift`（四段区块、36pt 行、可折叠、命令
//! 可见性切换）与 `Chrome/WorkspaceSidebarModel.swift`（行内容）。
//!
//! 这里只算「要画什么」，不碰 gpui：渲染层拿到 `SidebarModel` 直接铺行。

use std::collections::HashSet;

use crate::frontend::gpui::model::aggregate::{AGENTS_IDENTITY, SHELLS_IDENTITY};
use crate::frontend::gpui::model::attention::AttentionSnapshot;
use crate::frontend::gpui::model::sidebar::{
    self, backend_status, AgentItem, CommandItem, Indicator, SidebarItem,
};
use crate::frontend::utils::i18n::{tr, TextKey};

/// 段的顺序与 Swift 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Workspaces,
    Agents,
    Commands,
    HiddenCommands,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Workspaces,
        Section::Agents,
        Section::Commands,
        Section::HiddenCommands,
    ];

    pub fn title(self) -> String {
        tr(match self {
            Section::Workspaces => TextKey::SidebarWorkspaces,
            Section::Agents => TextKey::SidebarAgents,
            Section::Commands => TextKey::SidebarCommands,
            Section::HiddenCommands => TextKey::SidebarHiddenCommands,
        })
    }
}

/// 四段的展开状态。`HiddenCommands` 默认收起（对应 Swift `state = .off`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionExpansion {
    pub workspaces: bool,
    pub agents: bool,
    pub commands: bool,
    pub hidden_commands: bool,
}

impl Default for SectionExpansion {
    fn default() -> Self {
        Self {
            workspaces: true,
            agents: true,
            commands: true,
            hidden_commands: false,
        }
    }
}

impl SectionExpansion {
    pub fn is_expanded(&self, section: Section) -> bool {
        match section {
            Section::Workspaces => self.workspaces,
            Section::Agents => self.agents,
            Section::Commands => self.commands,
            Section::HiddenCommands => self.hidden_commands,
        }
    }

    pub fn toggle(&mut self, section: Section) {
        let slot = match section {
            Section::Workspaces => &mut self.workspaces,
            Section::Agents => &mut self.agents,
            Section::Commands => &mut self.commands,
            Section::HiddenCommands => &mut self.hidden_commands,
        };
        *slot = !*slot;
    }
}

/// 命令可见性的稳定身份：pane id 跨工作区会重复，必须带 workspace。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandKey {
    pub workspace_id: String,
    pub pane_id: u32,
}

/// 点一行之后要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowTarget {
    Workspace(String),
    /// 跳到另一个 workspace 的某个 pane。
    Pane {
        workspace_id: String,
        pane_id: u32,
        tab_id: Option<u32>,
    },
}

/// 侧栏一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarRow {
    /// 状态点/地标：`○` `●` `◆` `!` `◌``。
    pub marker: &'static str,
    pub indicator: Indicator,
    pub title: String,
    pub detail: String,
    /// 快捷键徽标：工作区是 `1`..`9`，聚合槽是 `S`/`A`。
    pub shortcut: Option<String>,
    pub active: bool,
    /// 聚合槽加粗显示。
    pub emphasized: bool,
    /// 连接丢失时详情标红。
    pub detail_alert: bool,
    /// 尾部按钮：命令行为 `eye.slash`/`eye`，工作区行为 `xmark`。
    pub trailing: Option<&'static str>,
    pub target: RowTarget,
}

/// 一段的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarSectionModel {
    pub section: Section,
    pub title: String,
    pub expanded: bool,
    pub rows: Vec<SidebarRow>,
}

/// 侧栏模型的全部输入。
pub struct SidebarInputs<'a> {
    pub workspaces: &'a [SidebarItem],
    pub attention: Option<&'a AttentionSnapshot>,
    pub expansion: SectionExpansion,
    pub hidden_commands: &'a HashSet<CommandKey>,
}

/// 把工作区列表投影成侧栏四段。
pub fn build(inputs: &SidebarInputs<'_>) -> Vec<SidebarSectionModel> {
    let agent_items = sidebar::agents(inputs.workspaces, inputs.attention);
    let command_items = sidebar::commands(inputs.workspaces, inputs.attention);

    let mut visible_commands = Vec::new();
    let mut hidden_commands = Vec::new();
    for item in command_items {
        let key = CommandKey {
            workspace_id: item.workspace_id.clone(),
            pane_id: item.pane_id,
        };
        let row = command_row(&item, inputs.hidden_commands.contains(&key));
        if inputs.hidden_commands.contains(&key) {
            hidden_commands.push(row);
        } else {
            visible_commands.push(row);
        }
    }

    Section::ALL
        .into_iter()
        .map(|section| SidebarSectionModel {
            section,
            title: section.title(),
            expanded: inputs.expansion.is_expanded(section),
            rows: match section {
                Section::Workspaces => inputs.workspaces.iter().map(workspace_row).collect(),
                Section::Agents => agent_items.iter().map(agent_row).collect(),
                Section::Commands => visible_commands.clone(),
                Section::HiddenCommands => hidden_commands.clone(),
            },
        })
        .collect()
}

/// 工作区行：标记、详情与快捷键照抄 `configureWorkspaceCell`。
fn workspace_row(item: &SidebarItem) -> SidebarRow {
    let aggregate = matches!(
        item.workspace_id.as_str(),
        SHELLS_IDENTITY | AGENTS_IDENTITY
    );
    let opening = item.opening_stage.is_some();
    // 断开 / 错误 / 已退出 三种都算「连接丢了」，标记与详情都要变红。
    let connection_lost = matches!(
        item.backend_status,
        backend_status::DISCONNECTED | backend_status::ERROR | backend_status::EXITED
    );
    let marker = if opening {
        "◌"
    } else if aggregate {
        "◆"
    } else if connection_lost {
        "!"
    } else if item.is_active {
        "●"
    } else {
        "○"
    };
    let indicator = if opening {
        Indicator::Working
    } else if connection_lost {
        Indicator::Blocked
    } else if item.is_active {
        Indicator::Done
    } else {
        Indicator::Idle
    };

    let connection_label = match item.backend_status {
        backend_status::DISCONNECTED => Some(tr(TextKey::StatusDisconnected)),
        backend_status::CONNECTING => Some(tr(TextKey::StatusConnecting)),
        backend_status::CONNECTED => None,
        backend_status::ERROR => Some(tr(TextKey::StatusError)),
        backend_status::EXITED => Some(tr(TextKey::StatusExited)),
        _ => Some(tr(TextKey::StatusUnknown)),
    };
    let base_detail = format!("{} @ {}", item.runtime, item.transport);
    let connection_detail = match &connection_label {
        Some(label) => format!("{base_detail} · {label}"),
        None => base_detail,
    };

    let detail = match &item.opening_stage {
        Some(stage) => format!("Opening · {stage}"),
        None => match (&item.runtime_update_phase, &item.runtime_update_message) {
            // idle / succeeded 不占详情行：没有话要对用户说。
            (Some(phase), Some(message)) if phase != "idle" && phase != "succeeded" => {
                message.clone()
            }
            _ => connection_detail,
        },
    };

    SidebarRow {
        marker,
        indicator,
        title: item.name.clone(),
        detail,
        shortcut: item.shortcut_text(),
        active: item.is_active,
        emphasized: aggregate,
        detail_alert: connection_lost,
        trailing: item.is_closable.then_some("xmark"),
        target: RowTarget::Workspace(item.workspace_id.clone()),
    }
}

/// Agent 行：标记取 indicator，标题与详情来自投影。
fn agent_row(item: &AgentItem) -> SidebarRow {
    SidebarRow {
        marker: item.indicator.marker(),
        indicator: item.indicator,
        title: item.title.clone(),
        detail: item.detail.clone(),
        shortcut: None,
        active: false,
        emphasized: false,
        detail_alert: false,
        trailing: None,
        target: RowTarget::Pane {
            workspace_id: item.workspace_id.clone(),
            pane_id: item.pane_id,
            tab_id: item.tab_id,
        },
    }
}

/// 命令行：尾部按钮在两段之间切换可见性。
fn command_row(item: &CommandItem, hidden: bool) -> SidebarRow {
    SidebarRow {
        marker: item.indicator.marker(),
        indicator: item.indicator,
        title: item.title.clone(),
        detail: item.detail.clone(),
        shortcut: None,
        active: false,
        emphasized: false,
        detail_alert: false,
        trailing: Some(if hidden { "eye" } else { "eye.slash" }),
        target: RowTarget::Pane {
            workspace_id: item.workspace_id.clone(),
            pane_id: item.pane_id,
            tab_id: item.tab_id,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::attention::{
        AttentionSnapshot, PaneAttention, PaneAttentionStatus, WorkspaceAttention,
    };
    use crate::frontend::gpui::model::sidebar::{PaneAgent, SidebarItem};

    fn item(id: &str, name: &str) -> SidebarItem {
        SidebarItem::new(id, name).with(|item| {
            item.runtime = "tmux".into();
            item.transport = "local".into();
        })
    }

    /// 单个工作区的注意力快照。
    fn attention_for(workspace: &SidebarItem, panes: Vec<PaneAttention>) -> AttentionSnapshot {
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

    fn empty_hidden() -> HashSet<CommandKey> {
        HashSet::new()
    }

    fn rows_of(model: &[SidebarSectionModel], section: Section) -> &[SidebarRow] {
        &model
            .iter()
            .find(|model| model.section == section)
            .expect("section")
            .rows
    }

    /// 对照 `WorkspaceSidebarView` 的四段顺序与默认展开状态。
    #[test]
    fn sections_keep_the_swift_order_and_default_expansion() {
        let workspaces = [item("local@@dev@tmux@dev", "dev")];
        let hidden = empty_hidden();
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: None,
            expansion: SectionExpansion::default(),
            hidden_commands: &hidden,
        });
        let order: Vec<Section> = model.iter().map(|model| model.section).collect();
        assert_eq!(
            order,
            vec![
                Section::Workspaces,
                Section::Agents,
                Section::Commands,
                Section::HiddenCommands
            ]
        );
        assert!(model[0].expanded && model[1].expanded && model[2].expanded);
        assert!(!model[3].expanded, "Hidden Commands 默认收起");
    }

    /// 对照 `configureWorkspaceCell`：标记、详情与快捷键。
    #[test]
    fn workspace_rows_follow_the_cell_rules() {
        let workspaces = [
            item("local@@dev@tmux@dev", "dev").with(|item| {
                item.is_active = true;
                item.shortcut = Some(1);
            }),
            item("local@@ops@herdr@w1", "ops").with(|item| {
                item.transport = "ryzen".into();
            }),
        ];
        let hidden = empty_hidden();
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: None,
            expansion: SectionExpansion::default(),
            hidden_commands: &hidden,
        });
        let rows = rows_of(&model, Section::Workspaces);
        assert_eq!(rows[0].marker, "●", "活动工作区实心");
        assert_eq!(rows[0].shortcut.as_deref(), Some("1"));
        assert_eq!(rows[0].detail, "tmux @ local");
        assert_eq!(rows[0].trailing.as_deref(), Some("xmark"));
        assert_eq!(rows[1].marker, "○");
        assert_eq!(rows[1].detail, "tmux @ ryzen");
    }

    /// 断连 / 错误 / 退出：标记变 `!`，详情追加状态词并标红。
    #[test]
    fn disconnected_workspaces_report_it_in_the_row() {
        let cases = [
            (backend_status::DISCONNECTED, TextKey::StatusDisconnected),
            (backend_status::ERROR, TextKey::StatusError),
            (backend_status::EXITED, TextKey::StatusExited),
        ];
        for (status, key) in cases {
            let workspaces = [item("w", "w").with(|item| item.backend_status = status)];
            let hidden = empty_hidden();
            let model = build(&SidebarInputs {
                workspaces: &workspaces,
                attention: None,
                expansion: SectionExpansion::default(),
                hidden_commands: &hidden,
            });
            let row = &rows_of(&model, Section::Workspaces)[0];
            assert_eq!(row.marker, "!");
            assert!(row.detail_alert);
            assert!(row.detail.ends_with(&tr(key)), "detail was {}", row.detail);
        }
    }

    /// 连接中保留状态词，已连接不追加。
    #[test]
    fn connecting_workspaces_keep_the_status_word() {
        let workspaces =
            [item("w", "w").with(|item| item.backend_status = backend_status::CONNECTING)];
        let hidden = empty_hidden();
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: None,
            expansion: SectionExpansion::default(),
            hidden_commands: &hidden,
        });
        let row = &rows_of(&model, Section::Workspaces)[0];
        assert_eq!(row.marker, "○");
        assert!(!row.detail_alert);
        assert!(row.detail.ends_with(&tr(TextKey::StatusConnecting)));
    }

    /// 打开中、聚合槽与运行时更新各自占详情行。
    #[test]
    fn opening_and_runtime_update_details_win_over_the_transport_line() {
        let workspaces = [
            item("opening", "new").with(|item| {
                item.opening_stage = Some("ssh".into());
                item.is_closable = false;
            }),
            item(SHELLS_IDENTITY, "Shells").with(|item| {
                item.is_closable = false;
                item.is_reorderable = false;
            }),
            item("upd", "upd").with(|item| {
                item.can_update_runtime = true;
                item.runtime_update_phase = Some("downloading".into());
                item.runtime_update_message = Some("Fetching 12%".into());
            }),
            item("done", "done").with(|item| {
                item.runtime_update_phase = Some("succeeded".into());
                item.runtime_update_message = Some("Updated".into());
            }),
        ];
        let hidden = empty_hidden();
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: None,
            expansion: SectionExpansion::default(),
            hidden_commands: &hidden,
        });
        let rows = rows_of(&model, Section::Workspaces);
        assert_eq!(rows[0].marker, "◌");
        assert_eq!(rows[0].detail, "Opening · ssh");
        assert_eq!(rows[0].shortcut, None, "不可关闭的工作区没有关闭按钮");
        assert_eq!(rows[0].trailing, None);

        assert_eq!(rows[1].marker, "◆");
        assert!(rows[1].emphasized);
        assert_eq!(rows[1].shortcut.as_deref(), Some("S"));

        assert_eq!(rows[2].detail, "Fetching 12%");
        assert_eq!(rows[3].detail, "tmux @ local", "succeeded 不占详情行");
    }

    /// Agents 段来自结构化 agent + attention，按投影顺序铺行。
    #[test]
    fn agent_rows_come_from_the_projection() {
        let workspaces = [item("ws", "dev").with(|item| {
            item.is_active = true;
            item.structured_agents = vec![PaneAgent {
                pane_id: 7,
                name: Some("codex".into()),
                status: crate::frontend::gpui::model::sidebar::AgentStatus::Working,
                state_change_seq: 3,
                revision: 1,
                ..PaneAgent::new(
                    7,
                    crate::frontend::gpui::model::sidebar::AgentStatus::Working,
                )
            }];
        })];
        let hidden = empty_hidden();
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: None,
            expansion: SectionExpansion::default(),
            hidden_commands: &hidden,
        });
        let rows = rows_of(&model, Section::Agents);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].marker, "●");
        assert_eq!(
            rows[0].target,
            RowTarget::Pane {
                workspace_id: "ws".into(),
                pane_id: 7,
                tab_id: None
            }
        );
    }

    /// 命令行的尾部按钮在两段之间切换，且两段不会重复。
    #[test]
    fn hidden_commands_move_to_their_own_section() {
        let workspaces = [item("ws", "dev").with(|item| item.is_active = true)];
        let snapshot = attention_for(
            &workspaces[0],
            vec![PaneAttention {
                pane_id: 3,
                status: PaneAttentionStatus::Working,
                last_line: "building".into(),
                seq: 9,
                process_name: Some("cargo".into()),
                ..PaneAttention::default()
            }],
        );
        let hidden: HashSet<CommandKey> = [CommandKey {
            workspace_id: "ws".into(),
            pane_id: 3,
        }]
        .into_iter()
        .collect();
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: Some(&snapshot),
            expansion: SectionExpansion::default(),
            hidden_commands: &hidden,
        });
        assert!(rows_of(&model, Section::Commands).is_empty());
        let hidden_rows = rows_of(&model, Section::HiddenCommands);
        assert_eq!(hidden_rows.len(), 1);
        assert_eq!(hidden_rows[0].trailing.as_deref(), Some("eye"));
    }

    /// 折叠只是状态：行还在模型里，渲染层据此决定画不画。
    #[test]
    fn collapsing_a_section_keeps_its_rows() {
        let workspaces = [item("ws", "dev")];
        let hidden = empty_hidden();
        let expansion = SectionExpansion {
            workspaces: false,
            ..SectionExpansion::default()
        };
        let model = build(&SidebarInputs {
            workspaces: &workspaces,
            attention: None,
            expansion,
            hidden_commands: &hidden,
        });
        let section = &model[0];
        assert!(!section.expanded);
        assert_eq!(section.rows.len(), 1);
    }
}
