//! 唯一的 FFI workspace-event 消费入口。
//!
//! `FfiClient` 仍可用于发送命令和读取 owned snapshot，但事件只能从这里
//! poll。这样后续 GTK 的主线程桥可以把同一批带身份事件写入 `ViewStore`，
//! 而不会再出现多个 frontend 路径分别读取同一个 Core handle。

use crate::ffi_client::{
    ClientActivityWorkspaceEvent, ClientConfigEvent, ClientWorkspaceEvent, FfiClient,
};

use crate::view_store::ViewStore;

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

    /// Drain Activity lane events after the workspace batch has normalized the
    /// corresponding runtime facts. Values are already owned by FfiClient.
    pub fn poll_activity(&self) -> Vec<ClientActivityWorkspaceEvent> {
        match self.client.take_activity_events() {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(target = "muxterm::activity", %error, "activity event poll failed");
                Vec::new()
            }
        }
    }

    /// Drain the Core configuration lane through the same single frontend
    /// event consumer.  Values are copied into owned DTOs by `FfiClient`.
    pub fn poll_config_events(&self) -> Vec<ClientConfigEvent> {
        match self.client.config_events() {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(target = "muxterm::config", %error, "configuration event poll failed");
                Vec::new()
            }
        }
    }

    /// Drain one FFI batch into the frontend-owned view store. Topology events
    /// refresh the owned DTO snapshot after Core has applied the batch; render
    /// events are retained in their workspace/pane mailbox.
    pub fn poll_into(&self, store: &mut ViewStore) -> usize {
        self.poll_into_with_events(store).len()
    }

    /// Drain one batch, apply it to the owned store, and return the copied
    /// events for activity/control adapters that still need the same batch.
    pub fn poll_into_with_events(&self, store: &mut ViewStore) -> Vec<ClientWorkspaceEvent> {
        let events = self.poll();
        for event in &events {
            if event.event.is_topology() {
                self.refresh_workspace(store, &event.workspace_id);
            }
            Self::apply_workspace_event(store, event.clone());
        }
        for event in self.poll_activity() {
            store.apply_activity_event(event);
        }
        events
    }

    /// Apply an already-owned event to the frontend-owned store. Tests and
    /// main-thread adapters can use this without touching the C handle.
    pub fn apply_workspace_event(store: &mut ViewStore, event: ClientWorkspaceEvent) {
        store.apply_workspace_event(event);
    }

    /// Send input through the production FFI command sink.
    pub fn send_input(&self, workspace_id: &str, pane_id: u32, data: &[u8]) -> anyhow::Result<()> {
        let rc = self
            .client
            .send_workspace_input(workspace_id, pane_id, data);
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!(
                "Core FFI input dispatch failed: workspace={workspace_id}, pane={pane_id}, code={rc}"
            );
        }
    }

    /// Seed all currently live workspace DTOs without activating any of them.
    pub fn sync_view_store(&self, store: &mut ViewStore) -> anyhow::Result<usize> {
        let workspaces = self.client.workspace_list()?;
        let count = workspaces.len();
        for workspace in workspaces {
            self.replace_workspace(store, workspace);
        }
        store.replace_activity_records(self.client.activity_records()?);
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

    fn replace_workspace(
        &self,
        store: &mut ViewStore,
        workspace: crate::ffi_client::ClientWorkspace,
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

#[cfg(test)]
mod tests {
    use super::EventPump;
    use crate::ffi_client::{ClientEvent, ClientWorkspaceEvent, FfiClient};
    use crate::view_store::ViewStore;

    #[test]
    fn catalog_pump_can_seed_and_poll_an_empty_view_store() {
        let pump = EventPump::new(FfiClient::new_catalog().expect("catalog handle"));
        let mut store = ViewStore::default();
        assert_eq!(pump.sync_view_store(&mut store).expect("workspace list"), 0);
        assert_eq!(pump.poll_into(&mut store), 0);
        assert!(store.workspace_ids().next().is_none());
    }

    #[test]
    fn owned_events_use_the_same_view_sink() {
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
    fn ffi_input_sink_propagates_core_errors() {
        let pump = EventPump::new(FfiClient::new_catalog().expect("catalog handle"));
        let error = pump
            .send_input("local//missing/shell/", 7, b"x")
            .expect_err("catalog handle has no workspace");
        assert!(error.to_string().contains("Core FFI input dispatch failed"));
    }
}
