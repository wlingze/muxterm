//! 唯一的 FFI workspace-event 消费入口。
//!
//! `FfiClient` 仍可用于发送命令和读取 owned snapshot，但事件只能从这里
//! poll。这样后续 GTK 的主线程桥可以把同一批带身份事件写入 `ViewStore`，
//! 而不会再出现多个 frontend 路径分别读取同一个 Core handle。

use crate::platform::ffi_client::{ClientWorkspaceEvent, FfiClient};

#[cfg(feature = "gtk")]
use crate::core::protocol::state::StateChange;
#[cfg(feature = "gtk")]
use crate::core::workspace::id::WorkspaceId;
#[cfg(feature = "gtk")]
use crate::core::workspace::pool::WorkspacePool;
#[cfg(feature = "gtk")]
use crate::platform::linux::view_store::ViewStore;

/// Owns the FFI client while providing the single workspace-event poll path.
pub struct EventPump {
    client: FfiClient,
}

impl EventPump {
    pub fn new(client: FfiClient) -> Self {
        Self { client }
    }

    /// Drain one owned batch from Core, preserving the workspace identity of
    /// every event.  Callers must not poll the client directly.
    pub fn poll(&self) -> Vec<ClientWorkspaceEvent> {
        self.client.poll_workspace_events()
    }

    /// Drain one FFI batch into the frontend-owned view store. Topology events
    /// refresh the owned DTO snapshot after Core has applied the batch; render
    /// events are retained in their workspace/pane mailbox.
    #[cfg(feature = "gtk")]
    pub fn poll_into(&self, store: &mut ViewStore) -> usize {
        let events = self.poll();
        let count = events.len();
        for event in events {
            if event.event.is_topology() {
                self.refresh_workspace(store, &event.workspace_id);
            }
            Self::apply_workspace_event(store, event);
        }
        count
    }

    /// Apply an already-owned event from a compatibility source. This keeps
    /// the GTK pool adapter and the real FFI source on the same ViewStore path.
    #[cfg(feature = "gtk")]
    pub fn apply_workspace_event(store: &mut ViewStore, event: ClientWorkspaceEvent) {
        store.apply_workspace_event(event);
    }

    /// Seed all currently live workspace DTOs without activating any of them.
    #[cfg(feature = "gtk")]
    pub fn sync_view_store(&self, store: &mut ViewStore) -> anyhow::Result<usize> {
        let workspaces = self.client.workspace_list()?;
        let count = workspaces.len();
        for workspace in workspaces {
            self.replace_workspace(store, workspace);
        }
        Ok(count)
    }

    pub fn client(&self) -> &FfiClient {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut FfiClient {
        &mut self.client
    }

    pub fn replace_client(&mut self, client: FfiClient) {
        self.client = client;
    }

    /// Poll the legacy GTK pool through the same event-pump boundary used by
    /// the FFI source. This adapter is temporary: it keeps the production
    /// migration to one consumer from adding a second runtime owner.
    #[cfg(feature = "gtk")]
    pub fn poll_pool_background(pool: &mut WorkspacePool) -> Vec<(WorkspaceId, Vec<StateChange>)> {
        pool.poll_background()
    }

    /// Poll the active workspace through the compatibility source and retain
    /// its stable identity next to the batch for the eventual FFI path.
    #[cfg(feature = "gtk")]
    pub fn poll_pool_active(pool: &mut WorkspacePool) -> Option<(WorkspaceId, Vec<StateChange>)> {
        let workspace_id = pool.active_id()?.clone();
        let events = pool.active_mut()?.refresh();
        Some((workspace_id, events))
    }

    #[cfg(feature = "gtk")]
    fn refresh_workspace(&self, store: &mut ViewStore, workspace_id: &str) {
        let Ok(workspaces) = self.client.workspace_list() else {
            return;
        };
        let Some(workspace) = workspaces
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            store.remove_workspace(workspace_id);
            return;
        };
        self.replace_workspace(store, workspace);
    }

    #[cfg(feature = "gtk")]
    fn replace_workspace(
        &self,
        store: &mut ViewStore,
        workspace: crate::platform::ffi_client::ClientWorkspace,
    ) {
        let workspace_id = workspace.id.clone();
        let tabs = self.client.get_workspace_tabs(&workspace.id);
        let panes = tabs
            .iter()
            .map(|tab| {
                (
                    tab.id,
                    self.client.get_workspace_panes(&workspace.id, tab.id),
                )
            })
            .collect();
        let layouts = tabs
            .iter()
            .filter_map(|tab| {
                self.client
                    .get_workspace_layout(&workspace.id, tab.id)
                    .map(|layout| (tab.id, layout))
            })
            .collect();
        store.replace_topology(workspace, tabs, panes);
        store.replace_layouts(&workspace_id, layouts);
    }
}

#[cfg(all(test, feature = "gtk"))]
mod tests {
    use super::EventPump;
    use crate::core::runtime::mock::MockRuntime;
    use crate::core::workspace::id::WorkspaceId;
    use crate::core::workspace::pool::WorkspacePool;
    use crate::core::workspace::workspace::Workspace;
    use crate::platform::ffi_client::{ClientEvent, ClientWorkspaceEvent, FfiClient};
    use crate::platform::linux::view_store::ViewStore;

    #[test]
    fn catalog_pump_can_seed_and_poll_an_empty_view_store() {
        let pump = EventPump::new(FfiClient::new_catalog().expect("catalog handle"));
        let mut store = ViewStore::default();
        assert_eq!(pump.sync_view_store(&mut store).expect("workspace list"), 0);
        assert_eq!(pump.poll_into(&mut store), 0);
        assert!(store.workspace_ids().next().is_none());
    }

    #[test]
    fn compatibility_events_use_the_same_owned_view_sink() {
        let mut store = ViewStore::default();
        EventPump::apply_workspace_event(
            &mut store,
            ClientWorkspaceEvent {
                workspace_id: "local//one/shell/".into(),
                event: ClientEvent {
                    type_: 0,
                    pane_id: 7,
                    tab_id: 1,
                    window_id: 0,
                    data: b"output".to_vec(),
                    name: String::new(),
                },
            },
        );
        let events = store.take_pane_render_events("local//one/shell/", 7);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, b"output");
    }

    #[test]
    fn compatibility_pool_poll_keeps_workspace_identity_with_the_batch() {
        let mut pool = WorkspacePool::default();
        let id = WorkspaceId::new("local", None, "pump", "shell", "");
        pool.insert_connected(Workspace::new(
            id.clone(),
            "pump".into(),
            Box::new(MockRuntime::with_single_pane()),
        ));

        let (observed, _) = EventPump::poll_pool_active(&mut pool).expect("active workspace");
        assert_eq!(observed, id);
    }
}
