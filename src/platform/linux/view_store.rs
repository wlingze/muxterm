//! Frontend-owned, workspace-keyed view snapshots.
//!
//! `ViewStore` contains only owned DTOs.  Core never receives a reference to
//! this store; the future `EventPump` (and the current compatibility event
//! adapter) are the only writers.  Render mailboxes are bounded so a chatty
//! hidden pane cannot grow frontend memory without limit.

use std::collections::{HashMap, VecDeque};

use super::super::ffi_client::{
    ClientEvent, ClientEventKind, ClientPane, ClientTab, ClientWorkspace, ClientWorkspaceEvent,
};

const RENDER_MAILBOX_CAPACITY: usize = 128;

/// One owned workspace view snapshot.
#[derive(Debug, Default)]
pub struct WorkspaceView {
    pub workspace: Option<ClientWorkspace>,
    pub tabs: Vec<ClientTab>,
    pub panes: HashMap<u32, Vec<ClientPane>>,
    render_mailboxes: HashMap<u32, VecDeque<ClientEvent>>,
}

/// Owned snapshots and render mailboxes keyed by stable workspace identity.
#[derive(Debug, Default)]
pub struct ViewStore {
    workspaces: HashMap<String, WorkspaceView>,
}

impl ViewStore {
    pub fn ensure_workspace(&mut self, workspace_id: &str) -> &mut WorkspaceView {
        self.workspaces.entry(workspace_id.to_string()).or_default()
    }

    pub fn remove_workspace(&mut self, workspace_id: &str) -> Option<WorkspaceView> {
        self.workspaces.remove(workspace_id)
    }

    pub fn workspace(&self, workspace_id: &str) -> Option<&WorkspaceView> {
        self.workspaces.get(workspace_id)
    }

    pub fn workspace_ids(&self) -> impl Iterator<Item = &str> {
        self.workspaces.keys().map(String::as_str)
    }

    pub fn replace_topology(
        &mut self,
        workspace: ClientWorkspace,
        tabs: Vec<ClientTab>,
        panes: Vec<(u32, Vec<ClientPane>)>,
    ) {
        let view = self.ensure_workspace(&workspace.id);
        view.workspace = Some(workspace);
        view.tabs = tabs;
        view.panes = panes.into_iter().collect();
    }

    /// Retain only render-data events. Control events are represented by the
    /// owned topology snapshot and do not belong in a pane byte mailbox.
    pub fn push_render_event(&mut self, workspace_id: &str, event: ClientEvent) {
        if !matches!(
            event.kind(),
            ClientEventKind::PaneOutput
                | ClientEventKind::PaneFrame
                | ClientEventKind::PaneSnapshot
                | ClientEventKind::PaneHistory
        ) {
            return;
        }
        let mailbox = self
            .ensure_workspace(workspace_id)
            .render_mailboxes
            .entry(event.pane_id)
            .or_default();
        if mailbox.len() == RENDER_MAILBOX_CAPACITY {
            mailbox.pop_front();
        }
        mailbox.push_back(event);
    }

    /// Apply one owned FFI event without exposing the C event buffer to the
    /// store. Control/topology events are refreshed by `EventPump`; only
    /// render-data events enter the pane mailbox here.
    pub fn apply_workspace_event(&mut self, event: ClientWorkspaceEvent) {
        let workspace_id = event.workspace_id;
        self.push_render_event(&workspace_id, event.event);
    }

    pub fn take_pane_render_events(
        &mut self,
        workspace_id: &str,
        pane_id: u32,
    ) -> Vec<ClientEvent> {
        self.workspaces
            .get_mut(workspace_id)
            .and_then(|workspace| workspace.render_mailboxes.remove(&pane_id))
            .map(|events| events.into_iter().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientEvent, ClientPane, ClientTab, ClientWorkspace, ViewStore};

    fn event(type_: u32, pane_id: u32, byte: u8) -> ClientEvent {
        ClientEvent {
            type_,
            pane_id,
            tab_id: 1,
            window_id: 0,
            data: vec![byte],
            name: String::new(),
        }
    }

    #[test]
    fn topology_is_owned_and_render_mailboxes_are_bounded() {
        let mut store = ViewStore::default();
        store.replace_topology(
            ClientWorkspace {
                id: "local//one/shell/".into(),
                name: "one".into(),
                runtime: "shell".into(),
                active: true,
            },
            vec![ClientTab {
                id: 1,
                name: "tab".into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 7,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "bash".into(),
                }],
            )],
        );
        for byte in 0..=u8::MAX {
            store.push_render_event(
                "local//one/shell/",
                event(crate::ffi::types::STATE_PANE_OUTPUT, 7, byte),
            );
        }
        let view = store
            .workspace("local//one/shell/")
            .expect("workspace snapshot");
        assert_eq!(view.tabs[0].name, "tab");
        assert_eq!(view.panes[&1][0].title, "bash");

        let events = store.take_pane_render_events("local//one/shell/", 7);
        assert_eq!(events.len(), 128);
        assert_eq!(events.first().map(|event| event.data[0]), Some(128));
        assert_eq!(events.last().map(|event| event.data[0]), Some(255));
    }

    #[test]
    fn control_events_do_not_enter_render_mailboxes() {
        let mut store = ViewStore::default();
        store.push_render_event(
            "local//one/shell/",
            event(crate::ffi::types::STATE_TAB_ADDED, 0, 1),
        );
        assert!(store
            .take_pane_render_events("local//one/shell/", 0)
            .is_empty());
    }
}
