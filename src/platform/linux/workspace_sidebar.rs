//! Linux workspace sidebar.
//!
//! The sidebar is a transient overlay on the terminal area. It is opened from
//! the window title bar and lists every workspace currently held by the Core
//! pool, including the active workspace and background workspaces.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Label, ListBox, ListBoxRow, Orientation, Revealer,
    RevealerTransitionType, ScrolledWindow, SelectionMode, ToggleButton,
};

use crate::core::model::state::BackendStatus;
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::WorkspacePool;
use crate::core::workspace::workspace::Workspace;

/// A workspace row in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSidebarItem {
    pub id: WorkspaceId,
    pub name: String,
    pub runtime: String,
    pub status: String,
    pub active: bool,
}

impl WorkspaceSidebarItem {
    /// Build the row model from a Core workspace.
    pub fn from_workspace(workspace: &Workspace, active_id: Option<&WorkspaceId>) -> Self {
        let state = workspace.state();
        Self {
            id: workspace.id().clone(),
            name: workspace.name().to_string(),
            runtime: state.workspace_runtime().to_string(),
            status: status_text(state.status()).to_string(),
            active: active_id == Some(workspace.id()),
        }
    }

    /// Build every row currently owned by the pool.
    pub fn from_pool(pool: &WorkspacePool) -> Vec<Self> {
        let active_id = pool.active_id();
        let mut items: Vec<Self> = pool
            .list()
            .into_iter()
            .map(|workspace| Self::from_workspace(workspace, active_id))
            .collect();
        items.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.cmp(&b.name)));
        items
    }
}

fn status_text(status: BackendStatus) -> &'static str {
    match status {
        BackendStatus::Connected => "connected",
        BackendStatus::Connecting => "connecting",
        BackendStatus::Disconnected => "disconnected",
        BackendStatus::Error => "error",
        BackendStatus::Exited => "exited",
    }
}

type WorkspaceActivateCb = Rc<RefCell<Option<Box<dyn Fn(&WorkspaceId)>>>>;

/// The title-bar toggle plus the overlay sidebar.
pub struct WorkspaceSidebar {
    pub revealer: Revealer,
    pub list: ListBox,
    pub toggle: ToggleButton,
    ids: Rc<RefCell<Vec<WorkspaceId>>>,
    on_activate: WorkspaceActivateCb,
}

impl WorkspaceSidebar {
    pub fn new() -> Self {
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

        let title = Label::builder()
            .label("Workspaces")
            .halign(Align::Start)
            .xalign(0.0)
            .margin_top(10)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        title.set_widget_name("muxterm-sidebar-title");
        title.add_css_class("muxterm-sidebar-title");

        let panel = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        panel.set_widget_name("muxterm-sidebar");
        panel.add_css_class("muxterm-sidebar");
        panel.append(&title);
        panel.append(&scrolled);

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
        revealer.set_size_request(280, -1);

        let ids = Rc::new(RefCell::new(Vec::new()));
        let on_activate: WorkspaceActivateCb = Rc::new(RefCell::new(None));

        {
            let revealer = revealer.clone();
            toggle.connect_toggled(move |button| {
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
            revealer,
            list,
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
                .label(format!("{} · {}", item.runtime, item.status))
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
        let id = WorkspaceId::new("local", None, name, "shell", name);
        Workspace::new(id, name.into(), Box::new(MockRuntime::with_single_pane()))
    }

    #[test]
    fn item_marks_active_workspace() {
        let ws = workspace("alpha");
        let item = WorkspaceSidebarItem::from_workspace(&ws, Some(ws.id()));
        assert!(item.active);
        assert_eq!(item.name, "alpha");
        assert_eq!(item.runtime, "tmux");
        assert_eq!(item.status, "connected");
    }

    #[test]
    fn item_marks_background_workspace() {
        let ws = workspace("beta");
        let item = WorkspaceSidebarItem::from_workspace(&ws, None);
        assert!(!item.active);
    }
}
