//! GTK 主窗口 UiState 的 ViewStore / CommandQueue 查询边界。

use crate::frontend::command_queue::ClientCommand;
use crate::frontend::ffi_client::{ClientRuntimeCapability, ClientTask};
use crate::frontend::linux::layout_host::LayoutHost;

use muxterm_protocol::WorkspaceId;

use super::UiState;

impl UiState {
    pub(super) fn active_ws_id(&self) -> WorkspaceId {
        self.visible_workspace.clone()
    }

    pub(super) fn active_workspace_key(&self) -> String {
        self.visible_workspace.as_str()
    }

    pub(super) fn active_layout(&self) -> &LayoutHost {
        let id = self.active_ws_id();
        self.scenes
            .get(&id)
            .expect("active workspace 必须有 layout")
    }

    pub(super) fn active_layout_mut(&mut self) -> &mut LayoutHost {
        let id = self.active_ws_id().clone();
        self.scenes
            .get_mut(&id)
            .expect("active workspace 必须有 layout")
    }

    /// Return the tab currently shown by the frontend for one workspace.
    ///
    /// Core's `ClientTab::is_active` remains the fallback for startup and for
    /// mutations that Core performs itself. Once the frontend has shown a
    /// tab, its choice wins until that tab disappears or Core reports a new
    /// active-tab mutation.
    pub(super) fn visible_tab_id(&self, workspace_key: &str) -> Option<u32> {
        let view = self.view_store.workspace(workspace_key)?;
        self.visible_tabs
            .get(workspace_key)
            .copied()
            .filter(|tab_id| view.tabs.iter().any(|tab| tab.id == *tab_id))
            .or_else(|| view.tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id))
            .or_else(|| view.tabs.first().map(|tab| tab.id))
    }

    pub(super) fn active_tab_id(&self) -> u32 {
        let workspace_key = self.active_workspace_key();
        self.visible_tab_id(&workspace_key)
            .unwrap_or(self.active_tab)
    }

    /// 当前前台是否 tmux/SSH 控制 client（local shell 不支持 detach）。
    pub(super) fn uses_tmux(&self) -> bool {
        self.active_workspace_runtime()
            .is_some_and(|runtime| matches!(runtime, "tmux" | "ssh" | "tmux-ssh"))
    }

    pub(super) fn active_workspace_runtime(&self) -> Option<&str> {
        let id = self.active_workspace_key();
        self.view_store
            .workspace(&id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.as_str())
    }

    pub(super) fn active_supports(&self, capability: ClientRuntimeCapability) -> bool {
        let workspace_id = self.active_workspace_key();
        self.workspace_supports(&workspace_id, capability)
    }

    pub(super) fn workspace_supports(
        &self,
        workspace_id: &str,
        capability: ClientRuntimeCapability,
    ) -> bool {
        let Some(runtime) = self
            .view_store
            .workspace(workspace_id)
            .and_then(|view| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.as_str())
        else {
            return false;
        };
        self.runtime_info
            .iter()
            .find(|provider| provider.id == runtime)
            .is_some_and(|provider| provider.supports(capability))
    }

    pub(super) fn execute_active_task(&self, task: ClientTask) -> anyhow::Result<()> {
        let workspace_id = self.active_workspace_key();
        if self.view_store.workspace(&workspace_id).is_none() {
            anyhow::bail!("没有激活的 workspace");
        }
        self.command_queue.borrow_mut().push(ClientCommand::Task {
            workspace_id: Some(workspace_id),
            task,
        });
        Ok(())
    }
}
