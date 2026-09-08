//! 唯一的 FFI workspace-event 消费入口。
//!
//! `FfiClient` 仍可用于发送命令和读取 owned snapshot，但事件只能从这里
//! poll。这样后续 GTK 的主线程桥可以把同一批带身份事件写入 `ViewStore`，
//! 而不会再出现多个 frontend 路径分别读取同一个 Core handle。

use crate::platform::ffi_client::{ClientWorkspaceEvent, FfiClient};

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

    pub fn client(&self) -> &FfiClient {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut FfiClient {
        &mut self.client
    }

    pub fn replace_client(&mut self, client: FfiClient) {
        self.client = client;
    }
}
