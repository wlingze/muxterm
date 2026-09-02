//! Linux workspace sidebar.
//!
//! The sidebar is the resizable left column of the main window. It is opened
//! from the window title bar and lists every workspace currently held by the
//! Core pool, including the active workspace and background workspaces.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Label, ListBox, ListBoxRow, Orientation, Paned, Revealer,
    RevealerTransitionType, ScrolledWindow, SelectionMode, ToggleButton, Widget,
};

use crate::core::attention::engine::{known_agent_process_name, PaneAttention, WorkspaceAttention};
use crate::core::attention::state::PaneStatus;
use crate::core::model::state::{PaneAgentInfo, PaneAgentStatus};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::WorkspacePool;
use crate::core::workspace::workspace::Workspace;

/// A workspace row in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSidebarItem {
    pub id: WorkspaceId,
    pub name: String,
    pub runtime: String,
    pub transport: String,
    pub active: bool,
    /// Ctrl+Alt+N 快捷编号；目前只暴露固定顺序的前五个 workspace。
    pub shortcut: Option<u8>,
}

impl WorkspaceSidebarItem {
    /// Build the row model from a Core workspace.
    pub fn from_workspace(workspace: &Workspace, active_id: Option<&WorkspaceId>) -> Self {
        let (runtime, transport) = workspace
            .resolved_target()
            .map(|resolved| {
                (
                    resolved.canonical.runtime.as_str().to_string(),
                    resolved.canonical.transport.label(),
                )
            })
            .unwrap_or_else(|| {
                let id = workspace.id();
                let transport = if id.transport == "ssh" {
                    id.alias.clone().unwrap_or_else(|| "ssh".into())
                } else {
                    "local".into()
                };
                (id.runtime.clone(), transport)
            });
        Self {
            id: workspace.id().clone(),
            name: workspace.name().to_string(),
            runtime,
            transport,
            active: active_id == Some(workspace.id()),
            shortcut: None,
        }
    }

    /// Build every row currently owned by the pool.
    pub fn from_pool(pool: &WorkspacePool) -> Vec<Self> {
        let active_id = pool.active_id();
        pool.list()
            .into_iter()
            .enumerate()
            .map(|(index, workspace)| {
                let mut item = Self::from_workspace(workspace, active_id);
                item.shortcut = (index < 5).then_some((index + 1) as u8);
                item
            })
            .collect()
    }
}

/// Agent、Command 与 Attention 共用的状态点语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityIndicator {
    Running,
    Done,
    None,
}

/// 跨全部 Workspace 汇总的一条 agent。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSidebarItem {
    pub workspace_id: WorkspaceId,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: ActivityIndicator,
}

impl AgentSidebarItem {
    pub fn from_pool(
        pool: &WorkspacePool,
        attention: &[WorkspaceAttention],
    ) -> Vec<AgentSidebarItem> {
        let attention_by_pane: HashMap<(String, u32), &PaneAttention> = attention
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .map(|pane| ((pane.workspace_id.clone(), pane.pane_id), pane))
            .collect();
        let mut items = Vec::new();
        for workspace in pool.list() {
            let workspace_key = workspace.id().replica_id();
            let state = workspace.state();
            for tab in state.tabs() {
                for pane in state.panes(&tab.id) {
                    let attention = attention_by_pane
                        .get(&(workspace_key.clone(), pane.id.0))
                        .copied();
                    if let Some(agent) = workspace.pane_agent(pane.id) {
                        items.push(AgentSidebarItem {
                            workspace_id: workspace.id().clone(),
                            pane_id: pane.id.0,
                            title: agent_title(agent, &pane.title),
                            detail: activity_detail(workspace, Some(agent)),
                            indicator: structured_indicator(agent.status, attention),
                        });
                    } else if let Some(attention) = attention {
                        let agent_name = attention.agent_name.as_deref().or_else(|| {
                            attention
                                .process_is_agent
                                .then_some(attention.process_name.as_deref())
                                .flatten()
                                .and_then(known_agent_process_name)
                        });
                        let Some(agent_name) = agent_name else {
                            continue;
                        };
                        items.push(AgentSidebarItem {
                            workspace_id: workspace.id().clone(),
                            pane_id: pane.id.0,
                            title: agent_name.to_string(),
                            detail: activity_detail(workspace, None),
                            indicator: if attention.process_is_agent {
                                attention_indicator(attention)
                            } else {
                                ActivityIndicator::None
                            },
                        });
                    }
                }
            }
        }
        items
    }
}

fn agent_title(agent: &PaneAgentInfo, pane_title: &str) -> String {
    [
        agent.display_name.as_deref(),
        agent.title.as_deref(),
        agent.name.as_deref(),
        agent.kind.as_deref(),
        agent.terminal_title_stripped.as_deref(),
        agent.terminal_title.as_deref(),
        Some(pane_title),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())
    .unwrap_or("agent")
    .to_string()
}

fn activity_detail(workspace: &Workspace, agent: Option<&PaneAgentInfo>) -> String {
    let path = agent
        .and_then(|agent| agent.foreground_cwd.as_deref().or(agent.cwd.as_deref()))
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            workspace
                .resolved_target()
                .map(|resolved| resolved.canonical.path.as_str())
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            (!workspace.id().path.trim().is_empty()).then_some(workspace.id().path.as_str())
        })
        .unwrap_or_else(|| workspace.name());
    let branch = agent.and_then(|agent| {
        ["branch", "git_branch", "git.branch"]
            .into_iter()
            .find_map(|key| agent.state_labels.get(key))
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    });
    match branch {
        Some(branch) => format!("{path} · {branch}"),
        None => path.to_string(),
    }
}

fn command_detail(workspace: &Workspace) -> String {
    let metadata = WorkspaceSidebarItem::from_workspace(workspace, None);
    format!(
        "{}@{}@{}",
        workspace.name(),
        metadata.runtime,
        metadata.transport
    )
}

fn structured_indicator(
    status: PaneAgentStatus,
    attention: Option<&PaneAttention>,
) -> ActivityIndicator {
    match status {
        PaneAgentStatus::Working => ActivityIndicator::Running,
        PaneAgentStatus::Blocked | PaneAgentStatus::Done
            if attention.is_none_or(|pane| !pane.acknowledged) =>
        {
            ActivityIndicator::Done
        }
        _ => ActivityIndicator::None,
    }
}

fn attention_indicator(attention: &PaneAttention) -> ActivityIndicator {
    match attention.status {
        PaneStatus::Working => ActivityIndicator::Running,
        PaneStatus::Blocked | PaneStatus::Done if !attention.acknowledged => {
            ActivityIndicator::Done
        }
        _ => ActivityIndicator::None,
    }
}

/// 跨全部 Workspace 汇总的一条正在运行或尚未阅读的非 agent 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSidebarItem {
    pub workspace_id: WorkspaceId,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: ActivityIndicator,
}

impl CommandSidebarItem {
    pub fn from_pool(
        pool: &WorkspacePool,
        attention: &[WorkspaceAttention],
    ) -> Vec<CommandSidebarItem> {
        let attention_by_pane: HashMap<(String, u32), &PaneAttention> = attention
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .map(|pane| ((pane.workspace_id.clone(), pane.pane_id), pane))
            .collect();
        let mut items = Vec::new();
        for workspace in pool.list() {
            let workspace_key = workspace.id().replica_id();
            let state = workspace.state();
            for tab in state.tabs() {
                for pane in state.panes(&tab.id) {
                    let Some(attention) = attention_by_pane
                        .get(&(workspace_key.clone(), pane.id.0))
                        .copied()
                    else {
                        continue;
                    };
                    let active = (attention.status == PaneStatus::Working
                        || (matches!(attention.status, PaneStatus::Blocked | PaneStatus::Done)
                            && !attention.acknowledged))
                        .then_some(attention);
                    let Some(active) = active else {
                        continue;
                    };
                    if active.process_is_agent || workspace.pane_agent(pane.id).is_some() {
                        continue;
                    }
                    let Some(title) = active
                        .process_name
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    else {
                        continue;
                    };
                    items.push(CommandSidebarItem {
                        workspace_id: workspace.id().clone(),
                        pane_id: pane.id.0,
                        title: title.to_string(),
                        detail: command_detail(workspace),
                        indicator: attention_indicator(active),
                    });
                }
            }
        }
        items
    }
}

type WorkspaceActivateCb = Rc<RefCell<Option<Box<dyn Fn(&WorkspaceId)>>>>;
type WorkspaceCloseCb = Rc<RefCell<Option<Box<dyn Fn(&WorkspaceId)>>>>;
type ActivityActivateCb = Rc<RefCell<Option<Box<dyn Fn(&WorkspaceId, u32)>>>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HiddenCommandKey {
    workspace_id: WorkspaceId,
    pane_id: u32,
    title: String,
}

impl From<&CommandSidebarItem> for HiddenCommandKey {
    fn from(item: &CommandSidebarItem) -> Self {
        Self {
            workspace_id: item.workspace_id.clone(),
            pane_id: item.pane_id,
            title: item.title.clone(),
        }
    }
}

fn section_header(title: &str, widget_name: &str) -> (ToggleButton, Label) {
    let arrow = Label::new(Some("▾"));
    arrow.add_css_class("muxterm-sidebar-section-arrow");
    let title = Label::builder()
        .label(title)
        .halign(Align::Start)
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("muxterm-sidebar-title");

    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .margin_start(8)
        .margin_end(8)
        .build();
    content.append(&arrow);
    content.append(&title);

    let button = ToggleButton::new();
    button.set_widget_name(widget_name);
    button.add_css_class("muxterm-sidebar-section-header");
    button.set_has_frame(false);
    button.set_can_focus(false);
    button.set_active(true);
    button.set_child(Some(&content));
    (button, arrow)
}

/// The title-bar toggle plus the resizable sidebar.
pub struct WorkspaceSidebar {
    pub container: GtkBox,
    pub revealer: Revealer,
    pub list: ListBox,
    pub agent_list: ListBox,
    pub command_list: ListBox,
    pub hidden_command_list: ListBox,
    pub sections: Paned,
    pub lower_sections: Paned,
    pub workspace_section_toggle: ToggleButton,
    pub agent_section_toggle: ToggleButton,
    pub command_section_toggle: ToggleButton,
    pub hidden_command_section_toggle: ToggleButton,
    pub toggle: ToggleButton,
    ids: Rc<RefCell<Vec<WorkspaceId>>>,
    agent_targets: Rc<RefCell<Vec<(WorkspaceId, u32)>>>,
    command_targets: Rc<RefCell<Vec<(WorkspaceId, u32)>>>,
    workspace_items: RefCell<Vec<WorkspaceSidebarItem>>,
    agent_items: RefCell<Vec<AgentSidebarItem>>,
    command_items: RefCell<Vec<CommandSidebarItem>>,
    hidden_commands: Rc<RefCell<HashSet<HiddenCommandKey>>>,
    on_activate: WorkspaceActivateCb,
    on_close: WorkspaceCloseCb,
    on_agent_activate: ActivityActivateCb,
    on_command_activate: ActivityActivateCb,
}

impl WorkspaceSidebar {
    pub fn new() -> Self {
        let container = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(0)
            .hexpand(false)
            .vexpand(true)
            .build();
        container.set_widget_name("muxterm-sidebar-shell");
        container.set_visible(false);

        let toggle = ToggleButton::with_label("☰");
        toggle.set_widget_name("muxterm-sidebar-toggle");
        toggle.set_has_frame(false);
        toggle.set_can_focus(false);
        toggle.set_active(false);

        let list = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        list.set_widget_name("muxterm-sidebar-list");
        list.set_activate_on_single_click(true);
        list.set_can_focus(false);
        list.add_css_class("muxterm-sidebar-list");

        let scrolled = ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        scrolled.set_widget_name("muxterm-sidebar-scroll");

        let (workspace_section_toggle, workspace_arrow) =
            section_header("WORKSPACES", "muxterm-sidebar-workspaces-toggle");
        let workspace_section = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .vexpand(true)
            .build();
        workspace_section.set_widget_name("muxterm-sidebar-workspaces-section");
        workspace_section.append(&workspace_section_toggle);
        workspace_section.append(&scrolled);

        let agent_list = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        agent_list.set_widget_name("muxterm-sidebar-agent-list");
        agent_list.set_activate_on_single_click(true);
        agent_list.set_can_focus(false);
        agent_list.add_css_class("muxterm-sidebar-list");

        let agent_scrolled = ScrolledWindow::builder()
            .child(&agent_list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        agent_scrolled.set_widget_name("muxterm-sidebar-agent-scroll");

        let (agent_section_toggle, agent_arrow) =
            section_header("AGENTS", "muxterm-sidebar-agents-toggle");
        let agent_section = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .vexpand(true)
            .build();
        agent_section.set_widget_name("muxterm-sidebar-agents-section");
        agent_section.append(&agent_section_toggle);
        agent_section.append(&agent_scrolled);

        let command_list = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        command_list.set_widget_name("muxterm-sidebar-command-list");
        command_list.set_activate_on_single_click(true);
        command_list.set_can_focus(false);
        command_list.add_css_class("muxterm-sidebar-list");

        let command_scrolled = ScrolledWindow::builder()
            .child(&command_list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        command_scrolled.set_widget_name("muxterm-sidebar-command-scroll");

        let (command_section_toggle, command_arrow) =
            section_header("COMMANDS", "muxterm-sidebar-commands-toggle");
        command_section_toggle.set_active(false);

        let hidden_command_list = ListBox::builder()
            .selection_mode(SelectionMode::Single)
            .build();
        hidden_command_list.set_widget_name("muxterm-sidebar-hidden-command-list");
        hidden_command_list.set_activate_on_single_click(true);
        hidden_command_list.set_can_focus(false);
        hidden_command_list.add_css_class("muxterm-sidebar-list");

        let hidden_command_scrolled = ScrolledWindow::builder()
            .child(&hidden_command_list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        hidden_command_scrolled.set_widget_name("muxterm-sidebar-hidden-command-scroll");

        let (hidden_command_section_toggle, hidden_command_arrow) =
            section_header("HIDDEN COMMANDS", "muxterm-sidebar-hidden-commands-toggle");
        hidden_command_section_toggle.set_active(false);
        let command_section = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .vexpand(true)
            .build();
        command_section.set_widget_name("muxterm-sidebar-commands-section");
        command_section.append(&command_section_toggle);
        command_section.append(&command_scrolled);
        command_section.append(&hidden_command_section_toggle);
        command_section.append(&hidden_command_scrolled);

        let lower_sections = Paned::new(Orientation::Vertical);
        lower_sections.set_widget_name("muxterm-sidebar-lower-sections");
        lower_sections.add_css_class("muxterm-sidebar-sections");
        lower_sections.set_wide_handle(false);
        lower_sections.set_shrink_start_child(false);
        lower_sections.set_shrink_end_child(false);
        lower_sections.set_start_child(Some(&agent_section));
        lower_sections.set_end_child(Some(&command_section));
        lower_sections.set_position(220);
        lower_sections.set_vexpand(true);

        let sections = Paned::new(Orientation::Vertical);
        sections.set_widget_name("muxterm-sidebar-sections");
        sections.add_css_class("muxterm-sidebar-sections");
        sections.set_wide_handle(false);
        sections.set_shrink_start_child(false);
        sections.set_shrink_end_child(false);
        sections.set_start_child(Some(&workspace_section));
        sections.set_end_child(Some(&lower_sections));
        sections.set_position(260);
        sections.set_vexpand(true);

        let panel = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .vexpand(true)
            .build();
        panel.set_widget_name("muxterm-sidebar");
        panel.add_css_class("muxterm-sidebar");
        panel.append(&sections);

        let saved_divider = Rc::new(Cell::new(260));
        let saved_lower_divider = Rc::new(Cell::new(220));
        let update_sections: Rc<dyn Fn()> = Rc::new({
            let sections = sections.clone();
            let lower_sections = lower_sections.clone();
            let workspace_section_toggle = workspace_section_toggle.clone();
            let agent_section_toggle = agent_section_toggle.clone();
            let command_section_toggle = command_section_toggle.clone();
            let hidden_command_section_toggle = hidden_command_section_toggle.clone();
            let workspace_arrow = workspace_arrow.clone();
            let agent_arrow = agent_arrow.clone();
            let command_arrow = command_arrow.clone();
            let hidden_command_arrow = hidden_command_arrow.clone();
            let scrolled = scrolled.clone();
            let agent_scrolled = agent_scrolled.clone();
            let command_scrolled = command_scrolled.clone();
            let hidden_command_scrolled = hidden_command_scrolled.clone();
            let workspace_section = workspace_section.clone();
            let agent_section = agent_section.clone();
            let command_section = command_section.clone();
            let saved_divider = saved_divider.clone();
            let saved_lower_divider = saved_lower_divider.clone();
            move || {
                let workspaces_open = workspace_section_toggle.is_active();
                let agents_open = agent_section_toggle.is_active();
                let commands_open = command_section_toggle.is_active();
                let hidden_commands_open = hidden_command_section_toggle.is_active();
                let command_group_open = commands_open || hidden_commands_open;
                let lower_open = agents_open || command_group_open;
                workspace_arrow.set_label(if workspaces_open { "▾" } else { "▸" });
                agent_arrow.set_label(if agents_open { "▾" } else { "▸" });
                command_arrow.set_label(if commands_open { "▾" } else { "▸" });
                hidden_command_arrow.set_label(if hidden_commands_open { "▾" } else { "▸" });
                scrolled.set_visible(workspaces_open);
                agent_scrolled.set_visible(agents_open);
                command_scrolled.set_visible(commands_open);
                hidden_command_scrolled.set_visible(hidden_commands_open);
                workspace_section.set_vexpand(workspaces_open);
                agent_section.set_vexpand(agents_open);
                command_section.set_vexpand(command_group_open);
                sections.set_vexpand(workspaces_open || lower_open);
                lower_sections.set_vexpand(lower_open);
                sections.set_resize_start_child(workspaces_open);
                sections.set_resize_end_child(lower_open);
                lower_sections.set_resize_start_child(agents_open);
                lower_sections.set_resize_end_child(command_group_open);
                match (workspaces_open, lower_open) {
                    (true, true) => sections.set_position(saved_divider.get()),
                    (true, false) => sections.set_position(i32::MAX),
                    (false, true) | (false, false) => sections.set_position(0),
                }
                match (agents_open, command_group_open) {
                    (true, true) => lower_sections.set_position(saved_lower_divider.get()),
                    (true, false) => lower_sections.set_position(i32::MAX),
                    (false, true) | (false, false) => lower_sections.set_position(0),
                }
            }
        });
        {
            let update_sections = update_sections.clone();
            workspace_section_toggle.connect_toggled(move |_| update_sections());
        }
        {
            let update_sections = update_sections.clone();
            agent_section_toggle.connect_toggled(move |_| update_sections());
        }
        {
            let update_sections = update_sections.clone();
            command_section_toggle.connect_toggled(move |_| update_sections());
        }
        {
            let update_sections = update_sections.clone();
            hidden_command_section_toggle.connect_toggled(move |_| update_sections());
        }
        {
            let workspace_section_toggle = workspace_section_toggle.clone();
            let agent_section_toggle = agent_section_toggle.clone();
            let command_section_toggle = command_section_toggle.clone();
            let hidden_command_section_toggle = hidden_command_section_toggle.clone();
            let saved_divider = saved_divider.clone();
            sections.connect_notify_local(Some("position"), move |paned, _| {
                if workspace_section_toggle.is_active()
                    && (agent_section_toggle.is_active()
                        || command_section_toggle.is_active()
                        || hidden_command_section_toggle.is_active())
                {
                    saved_divider.set(paned.position().max(1));
                }
            });
        }
        {
            let agent_section_toggle = agent_section_toggle.clone();
            let command_section_toggle = command_section_toggle.clone();
            let hidden_command_section_toggle = hidden_command_section_toggle.clone();
            let saved_lower_divider = saved_lower_divider.clone();
            lower_sections.connect_notify_local(Some("position"), move |paned, _| {
                if agent_section_toggle.is_active()
                    && (command_section_toggle.is_active()
                        || hidden_command_section_toggle.is_active())
                {
                    saved_lower_divider.set(paned.position().max(1));
                }
            });
        }
        update_sections();

        let revealer = Revealer::builder()
            .child(&panel)
            .transition_type(RevealerTransitionType::SlideRight)
            .reveal_child(false)
            .build();
        revealer.set_widget_name("muxterm-sidebar-revealer");
        revealer.set_halign(Align::Start);
        revealer.set_valign(Align::Fill);
        revealer.set_hexpand(false);
        revealer.set_vexpand(true);
        // Paned 负责实际宽度；这里只保留可用下限，初始 280px 由 window 设置。
        revealer.set_size_request(180, -1);

        let ids = Rc::new(RefCell::new(Vec::new()));
        let agent_targets = Rc::new(RefCell::new(Vec::new()));
        let command_targets = Rc::new(RefCell::new(Vec::new()));
        let hidden_commands = Rc::new(RefCell::new(HashSet::new()));
        let on_activate: WorkspaceActivateCb = Rc::new(RefCell::new(None));
        let on_close: WorkspaceCloseCb = Rc::new(RefCell::new(None));
        let on_agent_activate: ActivityActivateCb = Rc::new(RefCell::new(None));
        let on_command_activate: ActivityActivateCb = Rc::new(RefCell::new(None));

        container.append(&revealer);

        {
            let revealer = revealer.clone();
            let container = container.clone();
            toggle.connect_toggled(move |button| {
                container.set_visible(button.is_active());
                revealer.set_reveal_child(button.is_active());
            });
        }

        {
            let ids = ids.clone();
            let on_activate = on_activate.clone();
            list.connect_row_activated(move |_, row| {
                let index = row.index().max(0) as usize;
                let Some(id) = ids.borrow().get(index).cloned() else {
                    return;
                };
                if let Some(callback) = on_activate.borrow().as_ref() {
                    callback(&id);
                }
            });
        }

        {
            let agent_targets = agent_targets.clone();
            let on_agent_activate = on_agent_activate.clone();
            agent_list.connect_row_activated(move |_, row| {
                let index = row.index().max(0) as usize;
                let Some((id, pane)) = agent_targets.borrow().get(index).cloned() else {
                    return;
                };
                if let Some(callback) = on_agent_activate.borrow().as_ref() {
                    callback(&id, pane);
                }
            });
        }

        {
            let command_targets = command_targets.clone();
            let on_command_activate = on_command_activate.clone();
            command_list.connect_row_activated(move |_, row| {
                let index = row.index().max(0) as usize;
                let Some((id, pane)) = command_targets.borrow().get(index).cloned() else {
                    return;
                };
                if let Some(callback) = on_command_activate.borrow().as_ref() {
                    callback(&id, pane);
                }
            });
        }

        {
            let command_targets = command_targets.clone();
            let on_command_activate = on_command_activate.clone();
            hidden_command_list.connect_row_activated(move |_, row| {
                let index = row.index().max(0) as usize;
                let Some((id, pane)) = command_targets.borrow().get(index).cloned() else {
                    return;
                };
                if let Some(callback) = on_command_activate.borrow().as_ref() {
                    callback(&id, pane);
                }
            });
        }

        Self {
            container,
            revealer,
            list,
            agent_list,
            command_list,
            hidden_command_list,
            sections,
            lower_sections,
            workspace_section_toggle,
            agent_section_toggle,
            command_section_toggle,
            hidden_command_section_toggle,
            toggle,
            ids,
            agent_targets,
            command_targets,
            workspace_items: RefCell::new(Vec::new()),
            agent_items: RefCell::new(Vec::new()),
            command_items: RefCell::new(Vec::new()),
            hidden_commands,
            on_activate,
            on_close,
            on_agent_activate,
            on_command_activate,
        }
    }

    /// Set every row from the Core pool.
    pub fn set_workspaces(&self, items: &[WorkspaceSidebarItem]) {
        if self.workspace_items.borrow().as_slice() == items {
            return;
        }
        *self.workspace_items.borrow_mut() = items.to_vec();
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        *self.ids.borrow_mut() = items.iter().map(|item| item.id.clone()).collect();

        for item in items {
            let row = ListBoxRow::new();
            row.set_widget_name("muxterm-sidebar-row");
            row.set_can_focus(false);
            row.add_css_class("muxterm-sidebar-row");
            row.add_css_class("muxterm-sidebar-workspace-row");
            if item.active {
                row.add_css_class("active");
            }

            let content = GtkBox::builder()
                .orientation(Orientation::Horizontal)
                .spacing(4)
                .build();
            let labels = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(1)
                .margin_top(5)
                .margin_bottom(5)
                .margin_start(if item.shortcut.is_some() { 4 } else { 12 })
                .hexpand(true)
                .build();

            let marker = if item.active { "●" } else { "○" };
            let name = Label::builder()
                .label(format!("{marker} {}", item.name))
                .halign(Align::Start)
                .xalign(0.0)
                .build();
            name.add_css_class("muxterm-sidebar-row-name");

            let detail = Label::builder()
                .label(format!("{} @ {}", item.runtime, item.transport))
                .halign(Align::Start)
                .xalign(0.0)
                .build();
            detail.add_css_class("muxterm-sidebar-row-detail");

            labels.append(&name);
            labels.append(&detail);
            let close = Button::from_icon_name("window-close-symbolic");
            close.set_widget_name(&format!(
                "muxterm-sidebar-workspace-close-{}",
                widget_id(&item.id.as_str())
            ));
            close.add_css_class("muxterm-sidebar-close");
            close.set_has_frame(false);
            close.set_can_focus(false);
            close.set_focus_on_click(false);
            close.set_valign(Align::Center);
            close.set_margin_end(6);
            close.set_tooltip_text(Some("Close workspace"));
            {
                let id = item.id.clone();
                let on_close = self.on_close.clone();
                close.connect_clicked(move |_| {
                    if let Some(callback) = on_close.borrow().as_ref() {
                        callback(&id);
                    }
                });
            }
            if let Some(shortcut) = item.shortcut {
                let badge = Label::new(Some(&shortcut.to_string()));
                badge.set_widget_name(&format!("muxterm-sidebar-workspace-shortcut-{shortcut}"));
                badge.add_css_class("muxterm-sidebar-workspace-shortcut");
                badge.set_width_chars(2);
                badge.set_margin_start(6);
                badge.set_tooltip_text(Some(&format!("Ctrl+Alt+{shortcut}")));
                content.append(&badge);
            }
            content.append(&labels);
            content.append(&close);
            row.set_child(Some(&content));
            self.list.append(&row);

            if item.active {
                self.list.select_row(Some(&row));
            }
        }
    }

    /// 更新全部工作区的 agent 行；模型不变时不重建 GTK widget。
    pub fn set_agents(&self, items: &[AgentSidebarItem]) {
        if self.agent_items.borrow().as_slice() == items {
            return;
        }
        *self.agent_items.borrow_mut() = items.to_vec();
        while let Some(child) = self.agent_list.first_child() {
            self.agent_list.remove(&child);
        }
        *self.agent_targets.borrow_mut() = items
            .iter()
            .map(|item| (item.workspace_id.clone(), item.pane_id))
            .collect();

        for item in items {
            self.agent_list
                .append(&activity_row("agent", "muxterm-sidebar-agent-dot", item));
        }
    }

    /// 更新全部工作区的非 agent 活跃命令；隐藏状态只绑定当前命令身份，
    /// 同一 pane 启动另一条命令后会自动重新显示。
    pub fn set_commands(&self, items: &[CommandSidebarItem]) {
        if self.command_items.borrow().as_slice() == items {
            return;
        }
        *self.command_items.borrow_mut() = items.to_vec();
        while let Some(child) = self.command_list.first_child() {
            self.command_list.remove(&child);
        }
        while let Some(child) = self.hidden_command_list.first_child() {
            self.hidden_command_list.remove(&child);
        }
        let current_keys = items
            .iter()
            .map(HiddenCommandKey::from)
            .collect::<HashSet<_>>();
        self.hidden_commands
            .borrow_mut()
            .retain(|key| current_keys.contains(key));
        *self.command_targets.borrow_mut() = items
            .iter()
            .map(|item| (item.workspace_id.clone(), item.pane_id))
            .collect();

        for item in items {
            let key = HiddenCommandKey::from(item);
            let is_hidden = self.hidden_commands.borrow().contains(&key);
            let (visible_row, hide) = command_activity_row(item, false);
            let (hidden_row, show) = command_activity_row(item, true);
            visible_row.set_visible(!is_hidden);
            hidden_row.set_visible(is_hidden);

            {
                let hidden_commands = self.hidden_commands.clone();
                let key = key.clone();
                let visible_row = visible_row.clone();
                let hidden_row = hidden_row.clone();
                hide.connect_clicked(move |_| {
                    hidden_commands.borrow_mut().insert(key.clone());
                    visible_row.set_visible(false);
                    hidden_row.set_visible(true);
                });
            }
            {
                let hidden_commands = self.hidden_commands.clone();
                let key = key.clone();
                let visible_row = visible_row.clone();
                let hidden_row = hidden_row.clone();
                show.connect_clicked(move |_| {
                    hidden_commands.borrow_mut().remove(&key);
                    hidden_row.set_visible(false);
                    visible_row.set_visible(true);
                });
            }

            self.command_list.append(&visible_row);
            self.hidden_command_list.append(&hidden_row);
        }
    }

    /// Open or close the overlay.
    pub fn set_open(&self, open: bool) {
        self.toggle.set_active(open);
        self.revealer.set_reveal_child(open);
    }

    pub fn is_open(&self) -> bool {
        self.revealer.reveals_child()
    }

    /// Handle a workspace row activation.
    pub fn connect_workspace_activated<F: Fn(&WorkspaceId) + 'static>(&self, callback: F) {
        *self.on_activate.borrow_mut() = Some(Box::new(callback));
    }

    pub fn connect_workspace_closed<F: Fn(&WorkspaceId) + 'static>(&self, callback: F) {
        *self.on_close.borrow_mut() = Some(Box::new(callback));
    }

    pub fn connect_agent_activated<F: Fn(&WorkspaceId, u32) + 'static>(&self, callback: F) {
        *self.on_agent_activate.borrow_mut() = Some(Box::new(callback));
    }

    pub fn connect_command_activated<F: Fn(&WorkspaceId, u32) + 'static>(&self, callback: F) {
        *self.on_command_activate.borrow_mut() = Some(Box::new(callback));
    }
}

trait SidebarActivityRow {
    fn workspace_id(&self) -> &WorkspaceId;
    fn pane_id(&self) -> u32;
    fn title(&self) -> &str;
    fn detail(&self) -> &str;
    fn indicator(&self) -> ActivityIndicator;
}

impl SidebarActivityRow for AgentSidebarItem {
    fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    fn pane_id(&self) -> u32 {
        self.pane_id
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn detail(&self) -> &str {
        &self.detail
    }

    fn indicator(&self) -> ActivityIndicator {
        self.indicator
    }
}

impl SidebarActivityRow for CommandSidebarItem {
    fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    fn pane_id(&self) -> u32 {
        self.pane_id
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn detail(&self) -> &str {
        &self.detail
    }

    fn indicator(&self) -> ActivityIndicator {
        self.indicator
    }
}

fn activity_row(kind: &str, dot_widget_name: &str, item: &impl SidebarActivityRow) -> ListBoxRow {
    activity_row_with_trailing(kind, dot_widget_name, item, None)
}

fn activity_row_with_trailing(
    kind: &str,
    dot_widget_name: &str,
    item: &impl SidebarActivityRow,
    trailing: Option<&Widget>,
) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_widget_name(&format!(
        "muxterm-sidebar-{kind}-row-{}-{}",
        widget_id(&item.workspace_id().as_str()),
        item.pane_id()
    ));
    row.set_can_focus(false);
    row.add_css_class("muxterm-sidebar-row");
    row.add_css_class(&format!("muxterm-sidebar-{kind}-row"));

    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(8)
        .margin_end(8)
        .build();
    if item.indicator() != ActivityIndicator::None {
        let dot = Label::new(Some("●"));
        dot.set_widget_name(dot_widget_name);
        dot.add_css_class("muxterm-sidebar-agent-dot");
        dot.add_css_class(match item.indicator() {
            ActivityIndicator::Running => "running",
            ActivityIndicator::Done => "done",
            ActivityIndicator::None => unreachable!("None does not create a status dot"),
        });
        content.append(&dot);
    }
    let labels = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(1)
        .hexpand(true)
        .build();
    let title = Label::builder()
        .label(item.title())
        .halign(Align::Start)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("muxterm-sidebar-row-name");
    let detail = Label::builder()
        .label(item.detail())
        .halign(Align::Start)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    detail.add_css_class("muxterm-sidebar-row-detail");
    labels.append(&title);
    labels.append(&detail);
    content.append(&labels);
    if let Some(trailing) = trailing {
        content.append(trailing);
    }
    row.set_child(Some(&content));
    row
}

fn command_activity_row(item: &CommandSidebarItem, hidden: bool) -> (ListBoxRow, Button) {
    let (icon, tooltip, widget_name) = if hidden {
        (
            "view-reveal-symbolic",
            "显示命令",
            "muxterm-sidebar-command-show",
        )
    } else {
        (
            "view-conceal-symbolic",
            "隐藏命令",
            "muxterm-sidebar-command-hide",
        )
    };
    let action = Button::from_icon_name(icon);
    action.set_widget_name(widget_name);
    action.set_has_frame(false);
    action.set_can_focus(false);
    action.set_focus_on_click(false);
    action.set_tooltip_text(Some(tooltip));
    action.add_css_class("muxterm-sidebar-command-visibility");
    let row = activity_row_with_trailing(
        if hidden { "hidden-command" } else { "command" },
        "muxterm-sidebar-command-dot",
        item,
        Some(action.upcast_ref()),
    );
    (row, action)
}

fn widget_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

impl Default for WorkspaceSidebar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockRuntime;
    use crate::core::model::state::StateChange;
    use crate::core::types::PaneId;
    use std::collections::BTreeMap;
    use std::time::Instant;

    fn workspace(name: &str) -> Workspace {
        let id = WorkspaceId::new("local", None, name, "tmux", name);
        Workspace::new(id, name.into(), Box::new(MockRuntime::with_single_pane()))
    }

    fn agent(status: PaneAgentStatus) -> PaneAgentInfo {
        PaneAgentInfo {
            terminal_id: None,
            name: Some("codex".into()),
            kind: Some("codex".into()),
            title: Some("Review muxterm".into()),
            terminal_title: None,
            terminal_title_stripped: None,
            display_name: Some("Codex".into()),
            status,
            screen_detection_skipped: false,
            state_labels: BTreeMap::from([("branch".into(), "feature/sidebar".into())]),
            tokens: BTreeMap::new(),
            session: None,
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: 1,
            cwd: Some("/work/muxterm".into()),
            foreground_cwd: None,
            revision: 1,
        }
    }

    fn attention(
        workspace_id: String,
        status: PaneStatus,
        acknowledged: bool,
    ) -> WorkspaceAttention {
        WorkspaceAttention {
            workspace_id: workspace_id.clone(),
            blocked: usize::from(status == PaneStatus::Blocked),
            done: usize::from(status == PaneStatus::Done),
            working: usize::from(status == PaneStatus::Working),
            panes: vec![PaneAttention {
                workspace_id,
                pane_id: 1,
                status,
                acknowledged,
                last_line: String::new(),
                seq: 1,
                process_name: Some("codex".into()),
                process_is_agent: true,
                agent_name: Some("codex".into()),
                shell_name: Some("zsh".into()),
                mute_until: None,
                last_regex_eval: Instant::now(),
            }],
        }
    }

    #[test]
    fn item_marks_active_workspace() {
        let ws = workspace("alpha");
        let item = WorkspaceSidebarItem::from_workspace(&ws, Some(ws.id()));
        assert!(item.active);
        assert_eq!(item.name, "alpha");
        assert_eq!(item.runtime, "tmux");
        assert_eq!(item.transport, "local");
    }

    #[test]
    fn item_marks_background_workspace() {
        let ws = workspace("beta");
        let item = WorkspaceSidebarItem::from_workspace(&ws, None);
        assert!(!item.active);
    }

    #[test]
    fn item_formats_runtime_at_ssh_transport_name() {
        let id = WorkspaceId::new("ssh", Some("archmini"), "default", "herdr", "w2");
        let ws = Workspace::new(
            id,
            "muxterm".into(),
            Box::new(MockRuntime::with_single_pane()),
        );
        let item = WorkspaceSidebarItem::from_workspace(&ws, None);
        assert_eq!(item.runtime, "herdr");
        assert_eq!(item.transport, "archmini");
    }

    #[tokio::test]
    async fn pool_items_keep_open_order_when_active_workspace_changes() {
        let mut pool =
            WorkspacePool::new(crate::core::workspace::pool::WorkspacePoolPolicy::new(8));
        for name in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"] {
            let workspace_id = WorkspaceId::new("local", None, name, "shell", name);
            pool.open(workspace_id, name.into(), |_| {
                Box::new(MockRuntime::with_single_pane())
            })
            .await
            .unwrap();
        }
        let beta = WorkspaceId::new("local", None, "beta", "shell", "beta");
        pool.activate(&beta);

        let items = WorkspaceSidebarItem::from_pool(&pool);
        let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
        assert_eq!(
            names,
            ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"]
        );
        assert_eq!(
            items.iter().map(|item| item.shortcut).collect::<Vec<_>>(),
            [Some(1), Some(2), Some(3), Some(4), Some(5), None],
            "workspace shortcut numbers must follow stable opened_order"
        );
    }

    #[test]
    fn agents_merge_structured_runtime_and_generic_attention_state() {
        let id = WorkspaceId::new("local", None, "muxterm", "herdr", "w2");
        let mut runtime = MockRuntime::with_single_pane();
        runtime.events.push(StateChange::PaneAgentChanged {
            pane: PaneId(1),
            agent: Some(Box::new(agent(PaneAgentStatus::Working))),
            initial: false,
        });
        let mut workspace = Workspace::new(id.clone(), "muxterm".into(), Box::new(runtime));
        workspace.refresh();
        let mut pool =
            WorkspacePool::new(crate::core::workspace::pool::WorkspacePoolPolicy::new(8));
        pool.insert_connected(workspace);

        let items = AgentSidebarItem::from_pool(
            &pool,
            &[attention(id.replica_id(), PaneStatus::Working, false)],
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Codex");
        assert_eq!(items[0].detail, "/work/muxterm · feature/sidebar");
        assert_eq!(items[0].indicator, ActivityIndicator::Running);
    }

    #[test]
    fn tmux_pi_attention_is_projected_into_agents() {
        let workspace = workspace("agent-workspace");
        let id = workspace.id().clone();
        let mut pool =
            WorkspacePool::new(crate::core::workspace::pool::WorkspacePoolPolicy::new(8));
        pool.insert_connected(workspace);

        let mut generic = attention(id.replica_id(), PaneStatus::Working, true);
        generic.panes[0].process_name = Some("pi".into());
        generic.panes[0].agent_name = Some("pi".into());
        let items = AgentSidebarItem::from_pool(&pool, &[generic]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].workspace_id, id);
        assert_eq!(items[0].pane_id, 1);
        assert_eq!(items[0].title, "pi");
        assert_eq!(items[0].indicator, ActivityIndicator::Running);
    }

    #[test]
    fn commands_project_non_agent_lifecycle_and_return_to_shell_after_read() {
        let workspace = workspace("command-workspace");
        let id = workspace.id().clone();
        let mut pool =
            WorkspacePool::new(crate::core::workspace::pool::WorkspacePoolPolicy::new(8));
        pool.insert_connected(workspace);

        let shell_rows = CommandSidebarItem::from_pool(&pool, &[]);
        assert!(
            shell_rows.is_empty(),
            "idle shells and terminal titles must not create command rows"
        );

        let mut running = attention(id.replica_id(), PaneStatus::Working, true);
        running.panes[0].process_name = Some("cargo test".into());
        running.panes[0].process_is_agent = false;
        running.panes[0].agent_name = None;
        let rows = CommandSidebarItem::from_pool(&pool, &[running.clone()]);
        assert_eq!(rows[0].title, "cargo test");
        assert_eq!(rows[0].detail, "command-workspace@tmux@local");
        assert_eq!(rows[0].indicator, ActivityIndicator::Running);

        running.panes[0].status = PaneStatus::Done;
        running.panes[0].acknowledged = false;
        let rows = CommandSidebarItem::from_pool(&pool, &[running.clone()]);
        assert_eq!(rows[0].title, "cargo test");
        assert_eq!(rows[0].indicator, ActivityIndicator::Done);

        running.panes[0].acknowledged = true;
        let rows = CommandSidebarItem::from_pool(&pool, &[running]);
        assert!(
            rows.is_empty(),
            "a read command that returned to an idle shell must disappear"
        );
    }

    #[test]
    fn commands_classify_current_activity_separately_from_retained_agents() {
        let workspace = workspace("agent-workspace");
        let id = workspace.id().clone();
        let mut pool =
            WorkspacePool::new(crate::core::workspace::pool::WorkspacePoolPolicy::new(8));
        pool.insert_connected(workspace);
        let mut agent_attention = attention(id.replica_id(), PaneStatus::Idle, true);
        agent_attention.panes[0].shell_name = None;

        assert!(CommandSidebarItem::from_pool(&pool, &[agent_attention.clone()]).is_empty());

        agent_attention.panes[0].status = PaneStatus::Working;
        agent_attention.panes[0].process_name = Some("cargo".into());
        agent_attention.panes[0].process_is_agent = false;
        let commands = CommandSidebarItem::from_pool(&pool, &[agent_attention]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].title, "cargo");
        assert_eq!(commands[0].indicator, ActivityIndicator::Running);
    }

    #[test]
    fn commands_prefer_command_over_remote_pane_title_and_show_project_transport() {
        let id = WorkspaceId::new("ssh", Some("ryzen"), "default", "tmux", "/home/wlz/Devexx");
        let mut runtime = MockRuntime::with_single_pane();
        runtime.panes[0].title = "(ryzen) ~/Devexx · zsh".into();
        let workspace = Workspace::new(id.clone(), "Devexx".into(), Box::new(runtime));
        let mut pool =
            WorkspacePool::new(crate::core::workspace::pool::WorkspacePoolPolicy::new(8));
        pool.insert_connected(workspace);

        let mut running = attention(id.replica_id(), PaneStatus::Working, true);
        running.panes[0].process_name = Some("cargo test --workspace".into());
        running.panes[0].process_is_agent = false;
        running.panes[0].agent_name = None;

        let commands = CommandSidebarItem::from_pool(&pool, &[running]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].title, "cargo test --workspace");
        assert_eq!(commands[0].detail, "Devexx@tmux@ryzen");
        assert_ne!(commands[0].title, "(ryzen) ~/Devexx · zsh");
    }

    #[test]
    fn blocked_and_done_agents_turn_clear_after_acknowledgement() {
        let unread = attention("muxterm@local".into(), PaneStatus::Done, false);
        let seen = attention("muxterm@local".into(), PaneStatus::Done, true);
        assert_eq!(
            structured_indicator(PaneAgentStatus::Done, Some(&unread.panes[0])),
            ActivityIndicator::Done
        );
        assert_eq!(
            structured_indicator(PaneAgentStatus::Done, Some(&seen.panes[0])),
            ActivityIndicator::None
        );
        assert_eq!(
            structured_indicator(PaneAgentStatus::Blocked, None),
            ActivityIndicator::Done
        );
    }
}
