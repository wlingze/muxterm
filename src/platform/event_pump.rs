//! 唯一的 FFI workspace-event 消费入口。
//!
//! `FfiClient` 仍可用于发送命令和读取 owned snapshot，但事件只能从这里
//! poll。这样后续 GTK 的主线程桥可以把同一批带身份事件写入 `ViewStore`，
//! 而不会再出现多个 frontend 路径分别读取同一个 Core handle。

use crate::platform::ffi_client::{ClientWorkspaceEvent, FfiClient};

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
            store.apply_workspace_event(event);
        }
        count
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
        store.replace_topology(workspace, tabs, panes);
    }
}

#[cfg(all(test, feature = "gtk"))]
mod tests {
    use super::EventPump;
    use crate::platform::ffi_client::FfiClient;
    use crate::platform::linux::view_store::ViewStore;

    #[test]
    fn catalog_pump_can_seed_and_poll_an_empty_view_store() {
        let pump = EventPump::new(FfiClient::new_catalog().expect("catalog handle"));
        let mut store = ViewStore::default();
        assert_eq!(pump.sync_view_store(&mut store).expect("workspace list"), 0);
        assert_eq!(pump.poll_into(&mut store), 0);
        assert!(store.workspace_ids().next().is_none());
    }
}
