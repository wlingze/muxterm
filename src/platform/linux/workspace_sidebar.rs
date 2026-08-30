//! Linux workspace sidebar.
//!
//! The sidebar is the resizable left column of the main window. It is opened
//! from the window title bar and lists every workspace currently held by the
//! Core pool, including the active workspace and background workspaces.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Label, ListBox, ListBoxRow, Orientation, Paned, Revealer,
    RevealerTransitionType, ScrolledWindow, SelectionMode, ToggleButton,
};

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
        }
    }

    /// Build every row currently owned by the pool.
    pub fn from_pool(pool: &WorkspacePool) -> Vec<Self> {
        let active_id = pool.active_id();
        pool.list()
            .into_iter()
            .map(|workspace| Self::from_workspace(workspace, active_id))
            .collect()
    }
}

type WorkspaceActivateCb = Rc<RefCell<Option<Box<dyn Fn(&WorkspaceId)>>>>;

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
    pub sections: Paned,
    pub workspace_section_toggle: ToggleButton,
    pub agent_section_toggle: ToggleButton,
    pub toggle: ToggleButton,
    ids: Rc<RefCell<Vec<WorkspaceId>>>,
    on_activate: WorkspaceActivateCb,
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

        let sections = Paned::new(Orientation::Vertical);
        sections.set_widget_name("muxterm-sidebar-sections");
        sections.add_css_class("muxterm-sidebar-sections");
        sections.set_wide_handle(false);
        sections.set_shrink_start_child(false);
        sections.set_shrink_end_child(false);
        sections.set_start_child(Some(&workspace_section));
        sections.set_end_child(Some(&agent_section));
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
        let update_sections: Rc<dyn Fn()> = Rc::new({
            let sections = sections.clone();
            let workspace_section_toggle = workspace_section_toggle.clone();
            let agent_section_toggle = agent_section_toggle.clone();
            let workspace_arrow = workspace_arrow.clone();
            let agent_arrow = agent_arrow.clone();
            let scrolled = scrolled.clone();
            let agent_scrolled = agent_scrolled.clone();
            let saved_divider = saved_divider.clone();
            move || {
                let workspaces_open = workspace_section_toggle.is_active();
                let agents_open = agent_section_toggle.is_active();
                workspace_arrow.set_label(if workspaces_open { "▾" } else { "▸" });
                agent_arrow.set_label(if agents_open { "▾" } else { "▸" });
                scrolled.set_visible(workspaces_open);
                agent_scrolled.set_visible(agents_open);
                sections.set_vexpand(workspaces_open || agents_open);
                sections.set_resize_start_child(workspaces_open);
                sections.set_resize_end_child(agents_open);
                match (workspaces_open, agents_open) {
                    (true, true) => sections.set_position(saved_divider.get()),
                    (true, false) => sections.set_position(i32::MAX),
                    (false, true) | (false, false) => sections.set_position(0),
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
            let workspace_section_toggle = workspace_section_toggle.clone();
            let agent_section_toggle = agent_section_toggle.clone();
            let saved_divider = saved_divider.clone();
            sections.connect_notify_local(Some("position"), move |paned, _| {
                if workspace_section_toggle.is_active() && agent_section_toggle.is_active() {
                    saved_divider.set(paned.position().max(1));
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
        let on_activate: WorkspaceActivateCb = Rc::new(RefCell::new(None));

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

        Self {
            container,
            revealer,
            list,
            agent_list,
            sections,
            workspace_section_toggle,
            agent_section_toggle,
            toggle,
            ids,
            on_activate,
        }
    }

    /// Set every row from the Core pool.
    pub fn set_workspaces(&self, items: &[WorkspaceSidebarItem]) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        *self.ids.borrow_mut() = items.iter().map(|item| item.id.clone()).collect();

        for item in items {
            let row = ListBoxRow::new();
            row.set_widget_name("muxterm-sidebar-row");
            row.set_can_focus(false);
            row.add_css_class("muxterm-sidebar-row");
            if item.active {
                row.add_css_class("active");
            }

            let box_ = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(2)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(12)
                .margin_end(12)
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

            box_.append(&name);
            box_.append(&detail);
            row.set_child(Some(&box_));
            self.list.append(&row);

            if item.active {
                self.list.select_row(Some(&row));
            }
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

    fn workspace(name: &str) -> Workspace {
        let id = WorkspaceId::new("local", None, name, "tmux", name);
        Workspace::new(id, name.into(), Box::new(MockRuntime::with_single_pane()))
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
        for name in ["alpha", "beta", "gamma"] {
            let workspace_id = WorkspaceId::new("local", None, name, "shell", name);
            pool.open(workspace_id, name.into(), |_| {
                Box::new(MockRuntime::with_single_pane())
            })
            .await
            .unwrap();
        }
        let beta = WorkspaceId::new("local", None, "beta", "shell", "beta");
        pool.activate(&beta);

        let names: Vec<String> = WorkspaceSidebarItem::from_pool(&pool)
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
    }
}
