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

use crate::frontend::ffi_client::{ClientActivitySnapshot, ClientAttentionPane, ClientWorkspace};
use crate::frontend::linux::view_store::ViewStore;
use muxterm_protocol::WorkspaceId;

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
    /// Build sidebar rows from the frontend-owned workspace snapshot.
    pub fn from_views(store: &ViewStore) -> Vec<Self> {
        Self::from_views_with_active(store, store.active_workspace_id())
    }

    /// Build rows using the frontend-visible workspace instead of Core's
    /// transport-level active marker.  Scene switching is local and must not
    /// rewrite the Core snapshot just to update sidebar selection.
    pub fn from_views_with_active(store: &ViewStore, active_id: Option<&str>) -> Vec<Self> {
        let mut workspaces: Vec<&ClientWorkspace> = store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .collect();
        workspaces.sort_by(|left, right| left.id.cmp(&right.id));
        workspaces
            .into_iter()
            .enumerate()
            .filter_map(|(index, workspace)| {
                let id = parse_workspace_id(&workspace.id)?;
                Some(Self {
                    id,
                    name: workspace.name.clone(),
                    runtime: workspace.runtime.clone(),
                    transport: transport_label(&workspace.id),
                    active: active_id == Some(workspace.id.as_str()),
                    shortcut: (index < 5).then_some((index + 1) as u8),
                })
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
    /// Build agent rows from owned topology and Core's owned activity DTO.
    pub fn from_views(store: &ViewStore, activity: &ClientActivitySnapshot) -> Vec<Self> {
        let by_pane = activity_by_pane(activity);
        let mut items = Vec::new();
        for (workspace_key, view) in store.workspaces() {
            let Some(workspace) = view.workspace.as_ref() else {
                continue;
            };
            let Some(workspace_id) = parse_workspace_id(workspace_key) else {
                continue;
            };
            let activity_key = workspace_id.replica_id();
            for panes in view.panes.values() {
                for pane in panes {
                    let Some(attention) = by_pane.get(&(activity_key.as_str(), pane.id)) else {
                        continue;
                    };
                    let agent_name = attention.agent_name.as_deref().or_else(|| {
                        attention
                            .process_is_agent
                            .then_some(attention.process_name.as_deref())
                            .flatten()
                    });
                    let Some(agent_name) = agent_name else {
                        continue;
                    };
                    items.push(Self {
                        workspace_id: workspace_id.clone(),
                        pane_id: pane.id,
                        title: agent_name.to_string(),
                        detail: client_activity_detail(workspace, attention),
                        indicator: client_attention_indicator(attention),
                    });
                }
            }
        }
        items
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
    /// Build command rows from the owned activity projection.  A command row
    /// must have Core's process name; pane titles are never used as a command
    /// fallback because they are presentation text, not activity identity.
    pub fn from_views(store: &ViewStore, activity: &ClientActivitySnapshot) -> Vec<Self> {
        let by_pane = activity_by_pane(activity);
        let mut items = Vec::new();
        for (workspace_key, view) in store.workspaces() {
            let Some(workspace) = view.workspace.as_ref() else {
                continue;
            };
            let Some(workspace_id) = parse_workspace_id(workspace_key) else {
                continue;
            };
            let activity_key = workspace_id.replica_id();
            for panes in view.panes.values() {
                for pane in panes {
                    let Some(attention) = by_pane.get(&(activity_key.as_str(), pane.id)) else {
                        continue;
                    };
                    let active = matches!(attention.status.as_str(), "working")
                        || (matches!(attention.status.as_str(), "blocked" | "done")
                            && !attention.acknowledged);
                    if !active || attention.process_is_agent {
                        continue;
                    }
                    let Some(title) = attention
                        .process_name
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    else {
                        continue;
                    };
                    items.push(Self {
                        workspace_id: workspace_id.clone(),
                        pane_id: pane.id,
                        title: title.to_string(),
                        detail: client_activity_detail(workspace, attention),
                        indicator: client_attention_indicator(attention),
                    });
                }
            }
        }
        items
    }
}

fn parse_workspace_id(value: &str) -> Option<WorkspaceId> {
    let mut parts = value.splitn(5, '/');
    let transport = parts.next()?;
    let alias = parts.next()?;
    let session = parts.next()?;
    let runtime = parts.next()?;
    let path = parts.next().unwrap_or_default();
    if transport.is_empty() || runtime.is_empty() {
        return None;
    }
    Some(WorkspaceId::new(
        transport,
        (!alias.is_empty()).then_some(alias),
        session,
        runtime,
        path,
    ))
}

fn transport_label(value: &str) -> String {
    parse_workspace_id(value)
        .map(|id| {
            if id.transport == "ssh" {
                id.alias.unwrap_or_else(|| "ssh".into())
            } else {
                "local".into()
            }
        })
        .unwrap_or_else(|| "local".into())
}

fn activity_by_pane(
    activity: &ClientActivitySnapshot,
) -> HashMap<(&str, u32), &ClientAttentionPane> {
    activity
        .workspaces
        .iter()
        .flat_map(|workspace| {
            workspace
                .panes
                .iter()
                .map(move |pane| (workspace.workspace_id.as_str(), pane.pane_id, pane))
        })
        .map(|(workspace, pane, attention)| ((workspace, pane), attention))
        .collect()
}

fn client_attention_indicator(attention: &ClientAttentionPane) -> ActivityIndicator {
    match attention.status.as_str() {
        "working" => ActivityIndicator::Running,
        "blocked" | "done" if !attention.acknowledged => ActivityIndicator::Done,
        _ => ActivityIndicator::None,
    }
}

fn client_activity_detail(workspace: &ClientWorkspace, attention: &ClientAttentionPane) -> String {
    let identity = parse_workspace_id(&workspace.id)
        .map(|id| {
            let transport = if id.transport == "ssh" {
                id.alias.unwrap_or_else(|| "ssh".into())
            } else {
                "local".into()
            };
            format!("{}@{}@{}", workspace.name, workspace.runtime, transport)
        })
        .unwrap_or_else(|| workspace.name.clone());
    let path = attention
        .last_line
        .trim()
        .is_empty()
        .then(|| parse_workspace_id(&workspace.id).map(|id| id.path))
        .flatten()
        .filter(|path| !path.trim().is_empty());
    path.map(|path| format!("{identity} · {path}"))
        .unwrap_or(identity)
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
        command_section.set_size_request(-1, SIDEBAR_SECTION_HEADER_PX * 2);
        command_section.append(&command_section_toggle);
        command_section.append(&command_scrolled);
        command_section.append(&hidden_command_section_toggle);
        command_section.append(&hidden_command_scrolled);

        let lower_sections = Paned::new(Orientation::Vertical);
        lower_sections.set_widget_name("muxterm-sidebar-lower-sections");
        lower_sections.add_css_class("muxterm-sidebar-sections");
        lower_sections.set_wide_handle(false);
        lower_sections.set_shrink_start_child(true);
        lower_sections.set_shrink_end_child(true);
        lower_sections.set_start_child(Some(&agent_section));
        lower_sections.set_end_child(Some(&command_section));
        lower_sections.set_position(220);
        lower_sections.set_vexpand(true);

        let sections = Paned::new(Orientation::Vertical);
        sections.set_widget_name("muxterm-sidebar-sections");
        sections.add_css_class("muxterm-sidebar-sections");
        sections.set_wide_handle(false);
        sections.set_shrink_start_child(true);
        sections.set_shrink_end_child(true);
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
                apply_sidebar_split(
                    &sections,
                    workspaces_open,
                    lower_open,
                    saved_divider.get(),
                    SIDEBAR_SECTION_HEADER_PX * 2,
                );
                apply_sidebar_split(
                    &lower_sections,
                    agents_open,
                    command_group_open,
                    saved_lower_divider.get(),
                    SIDEBAR_SECTION_HEADER_PX * 2,
                );
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
            let update_sections = update_sections.clone();
            sections.connect_notify_local(Some("height"), move |_, _| update_sections());
        }
        {
            let update_sections = update_sections.clone();
            lower_sections.connect_notify_local(Some("height"), move |_, _| update_sections());
        }
        {
            let update_sections = update_sections.clone();
            sections.connect_realize(move |_| update_sections());
        }
        {
            let update_sections = update_sections.clone();
            lower_sections.connect_realize(move |_| update_sections());
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
                    if let Some(position) =
                        persist_sidebar_divider(paned.position(), paned.height())
                    {
                        saved_divider.set(position);
                    }
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
                    if let Some(position) =
                        persist_sidebar_divider(paned.position(), paned.height())
                    {
                        saved_lower_divider.set(position);
                    }
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
            let update_sections = update_sections.clone();
            toggle.connect_toggled(move |button| {
                container.set_visible(button.is_active());
                revealer.set_reveal_child(button.is_active());
                if button.is_active() {
                    update_sections();
                }
            });
        }
        {
            let update_sections = update_sections.clone();
            revealer.connect_notify_local(Some("reveal-child"), move |revealer, _| {
                if revealer.reveals_child() {
                    update_sections();
                }
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

    /// 从 frontend-owned 快照生成并应用 Sidebar 的全部页面模型。
    pub fn refresh_from_views(
        &self,
        store: &ViewStore,
        active_id: Option<&str>,
        activity: &ClientActivitySnapshot,
    ) {
        let workspaces = WorkspaceSidebarItem::from_views_with_active(store, active_id);
        let agents = AgentSidebarItem::from_views(store, activity);
        let commands = CommandSidebarItem::from_views(store, activity);
        self.set_workspaces(&workspaces);
        self.set_agents(&agents);
        self.set_commands(&commands);
    }

    /// 只刷新 workspace 区域，保留 agent/command 列表的现有 widget。
    pub fn refresh_workspaces_from_views(&self, store: &ViewStore, active_id: Option<&str>) {
        let workspaces = WorkspaceSidebarItem::from_views_with_active(store, active_id);
        self.set_workspaces(&workspaces);
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

/// 折叠段仍要露出标题；`i32::MAX` 会把整段（含 COMMANDS 标题）压成 0。
const SIDEBAR_SECTION_HEADER_PX: i32 = 32;
/// GtkPaned 分割条自身占位，折叠端高度要额外留出，否则标题落在 handle 底下点不到。
const SIDEBAR_SPLIT_HANDLE_PX: i32 = 8;

fn sidebar_split_position(
    start_open: bool,
    end_open: bool,
    total: i32,
    saved: i32,
    end_headers: i32,
) -> i32 {
    // GtkPaned 未分配时 height=0。此时若算成 1px，列表会被钉死，
    // 侧栏只剩 WORKSPACES/AGENTS 标题、看不到行。
    if total <= 2 {
        return saved.max(1);
    }
    let total = total.max(1);
    let end_headers = end_headers.max(SIDEBAR_SECTION_HEADER_PX);
    match (start_open, end_open) {
        (true, true) => {
            let min_end = end_headers.max(80);
            let min_start = SIDEBAR_SECTION_HEADER_PX.max(80);
            if total <= min_start + min_end {
                (total / 2).max(1)
            } else {
                saved.clamp(min_start, total - min_end)
            }
        }
        (true, false) => {
            let keep = end_headers.saturating_add(SIDEBAR_SPLIT_HANDLE_PX);
            if total <= keep + SIDEBAR_SECTION_HEADER_PX {
                (total / 2).max(1)
            } else {
                total - keep
            }
        }
        (false, true) | (false, false) => SIDEBAR_SECTION_HEADER_PX.min(total).max(1),
    }
}

fn apply_sidebar_split(
    paned: &Paned,
    start_open: bool,
    end_open: bool,
    saved: i32,
    end_headers: i32,
) {
    let total = paned.height();
    // 与 layout_host 相同：realize 前不要 set_position，否则 1px 会粘住。
    if total <= 2 {
        return;
    }
    let want = sidebar_split_position(start_open, end_open, total, saved, end_headers);
    if (paned.position() - want).abs() > 1 {
        paned.set_position(want);
    }
}

fn persist_sidebar_divider(position: i32, total: i32) -> Option<i32> {
    let total = total.max(1);
    if position <= 1 || position >= total.saturating_sub(1) {
        return None;
    }
    Some(position)
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

    #[test]
    fn collapsed_commands_keep_header_space() {
        let pos = sidebar_split_position(true, false, 400, 220, 64);
        assert_eq!(
            pos, 328,
            "Agents 展开、Commands 折叠时必须露出 Commands 标题（含 handle）"
        );
        assert_eq!(400 - pos, 72);
        let open = sidebar_split_position(true, true, 400, 220, 64);
        assert_eq!(open, 220);
        let clamped = sidebar_split_position(true, true, 400, 390, 64);
        assert!(
            clamped <= 320,
            "saved divider 贴底时必须给 Commands 留高度: {clamped}"
        );
        assert!(persist_sidebar_divider(i32::MAX, 400).is_none());
        assert!(persist_sidebar_divider(0, 400).is_none());
        assert_eq!(persist_sidebar_divider(180, 400), Some(180));
    }

    #[test]
    fn unallocated_split_keeps_saved_divider() {
        // GtkPaned 在 realize 前 height=0。若此时把 position 算成 1，
        // WORKSPACES/AGENTS 列表会被钉在 1px，侧栏只剩标题没有内容。
        assert_eq!(
            sidebar_split_position(true, true, 0, 260, 64),
            260,
            "height=0 must keep the construction divider"
        );
        assert_eq!(
            sidebar_split_position(true, false, 1, 220, 64),
            220,
            "height=1 must not collapse Agents onto the Commands header"
        );
        assert_eq!(sidebar_split_position(false, true, 0, 260, 64), 260);
    }

    use crate::frontend::ffi_client::{
        ClientActivitySnapshot, ClientAttentionPane, ClientPane, ClientTab, ClientWorkspace,
        ClientWorkspaceAttention,
    };
    use crate::frontend::linux::view_store::ViewStore;

    fn workspace_id(
        transport: &str,
        alias: Option<&str>,
        session: &str,
        runtime: &str,
        path: &str,
    ) -> WorkspaceId {
        WorkspaceId::new(transport, alias, session, runtime, path)
    }

    fn add_workspace(store: &mut ViewStore, id: &WorkspaceId, name: &str, active: bool) {
        store.replace_topology(
            ClientWorkspace {
                id: id.as_str(),
                name: name.into(),
                runtime: id.runtime.clone(),
                active,
                resolved_target: None,
            },
            vec![ClientTab {
                id: 1,
                name: "main".into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 1,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "shell".into(),
                }],
            )],
        );
    }

    fn activity(
        id: &WorkspaceId,
        status: &str,
        acknowledged: bool,
        process_name: Option<&str>,
        process_is_agent: bool,
        agent_name: Option<&str>,
    ) -> ClientActivitySnapshot {
        ClientActivitySnapshot {
            blocked_count: 0,
            workspaces: vec![ClientWorkspaceAttention {
                workspace_id: id.replica_id(),
                path: id.path.clone(),
                blocked: usize::from(status == "blocked"),
                done: usize::from(status == "done"),
                working: usize::from(status == "working"),
                panes: vec![ClientAttentionPane {
                    workspace_id: id.replica_id(),
                    pane_id: 1,
                    status: status.into(),
                    acknowledged,
                    last_line: String::new(),
                    seq: 1,
                    process_name: process_name.map(str::to_owned),
                    process_is_agent,
                    agent_name: agent_name.map(str::to_owned),
                    shell_name: Some("zsh".into()),
                }],
            }],
        }
    }

    #[test]
    fn workspace_rows_use_owned_topology_and_preserve_shortcuts() {
        let alpha = workspace_id("local", None, "alpha", "shell", "/work/alpha");
        let beta = workspace_id("ssh", Some("archmini"), "default", "herdr", "/work/beta");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &beta, "beta", false);
        add_workspace(&mut store, &alpha, "alpha", true);

        let items = WorkspaceSidebarItem::from_views(&store);
        assert_eq!(
            items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert!(items[0].active);
        assert_eq!(items[0].shortcut, Some(1));
        assert!(!items[1].active);
        assert_eq!(items[1].runtime, "herdr");
        assert_eq!(items[1].transport, "archmini");
        assert_eq!(items[1].shortcut, Some(2));

        let beta_key = beta.as_str();
        let visible_beta = WorkspaceSidebarItem::from_views_with_active(&store, Some(&beta_key));
        assert!(!visible_beta[0].active);
        assert!(visible_beta[1].active);
    }

    #[test]
    fn agent_rows_use_core_activity_identity_without_pane_title_guessing() {
        let id = workspace_id("local", None, "muxterm", "herdr", "/work/muxterm");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &id, "muxterm", true);
        let snapshot = activity(&id, "working", false, Some("codex"), true, Some("Codex"));

        let items = AgentSidebarItem::from_views(&store, &snapshot);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].workspace_id, id);
        assert_eq!(items[0].pane_id, 1);
        assert_eq!(items[0].title, "Codex");
        assert_eq!(items[0].detail, "muxterm@herdr@local · /work/muxterm");
        assert_eq!(items[0].indicator, ActivityIndicator::Running);

        let no_activity = ClientActivitySnapshot::default();
        assert!(AgentSidebarItem::from_views(&store, &no_activity).is_empty());
    }

    #[test]
    fn command_rows_follow_activity_lifecycle_and_ignore_agents() {
        let id = workspace_id("local", None, "command-workspace", "tmux", "/work/command");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &id, "command-workspace", true);

        let idle = activity(&id, "idle", true, Some("zsh"), false, None);
        assert!(CommandSidebarItem::from_views(&store, &idle).is_empty());

        let running = activity(&id, "working", true, Some("cargo test"), false, None);
        let rows = CommandSidebarItem::from_views(&store, &running);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "cargo test");
        assert_eq!(
            rows[0].detail,
            "command-workspace@tmux@local · /work/command"
        );
        assert_eq!(rows[0].indicator, ActivityIndicator::Running);

        let mut done = running.clone();
        done.workspaces[0].panes[0].status = "done".into();
        done.workspaces[0].panes[0].acknowledged = false;
        let rows = CommandSidebarItem::from_views(&store, &done);
        assert_eq!(rows[0].indicator, ActivityIndicator::Done);

        done.workspaces[0].panes[0].acknowledged = true;
        assert!(CommandSidebarItem::from_views(&store, &done).is_empty());

        let agent = activity(&id, "working", false, Some("codex"), true, Some("codex"));
        assert!(CommandSidebarItem::from_views(&store, &agent).is_empty());
        assert_eq!(
            AgentSidebarItem::from_views(&store, &agent)[0].title,
            "codex"
        );
    }

    #[test]
    fn command_rows_prefer_process_name_and_show_ssh_identity() {
        let id = workspace_id("ssh", Some("ryzen"), "default", "tmux", "/home/wlz/Devexx");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &id, "Devexx", true);
        let snapshot = activity(
            &id,
            "working",
            true,
            Some("cargo test --workspace"),
            false,
            None,
        );

        let rows = CommandSidebarItem::from_views(&store, &snapshot);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "cargo test --workspace");
        assert_eq!(rows[0].detail, "Devexx@tmux@ryzen · /home/wlz/Devexx");
    }
}
