//! TUI-owned view models built from the frontend ViewStore.

use std::collections::HashMap;

use crate::frontend::utils::corebridge::{ClientLayout, ClientPane, ClientTab};
use crate::frontend::view_store::WorkspaceView;

pub type TuiLayout = ClientLayout;
pub type TuiPane = ClientPane;
pub type TuiTab = ClientTab;

/// All data needed to render one TUI frame.
#[derive(Debug, Clone, Default)]
pub struct FrameSnapshot {
    pub tabs: Vec<TuiTab>,
    pub panes: Vec<TuiPane>,
    pub layout: Option<TuiLayout>,
    /// Unused on the live Scene path; kept so render tests can still inject bytes.
    pub outputs: HashMap<u32, Vec<u8>>,
    pub status: String,
    pub active_tab: u32,
    pub active_pane: u32,
}

impl FrameSnapshot {
    /// Compose a render view from the owned workspace snapshot.
    ///
    /// `visible_tab` is the Scene-local tab. Switching tabs does not query Core.
    pub fn from_workspace_view(
        view: &WorkspaceView,
        status: impl Into<String>,
        visible_tab: Option<u32>,
    ) -> Self {
        let active_tab = visible_tab.or_else(|| view.active_tab_id()).unwrap_or(0);
        let mut tabs = view.tabs.clone();
        for tab in &mut tabs {
            tab.is_active = tab.id == active_tab;
        }
        let panes = view.panes_for_tab(active_tab).to_vec();
        let active_pane = panes
            .iter()
            .find(|pane| pane.is_active)
            .map(|pane| pane.id)
            .or_else(|| panes.first().map(|pane| pane.id))
            .unwrap_or(0);
        let layout = view.layouts.get(&active_tab).cloned();
        Self {
            tabs,
            panes,
            layout,
            outputs: HashMap::new(),
            status: status.into(),
            active_tab,
            active_pane,
        }
    }
}

pub(crate) fn status_label(code: u32) -> &'static str {
    match code {
        0 => "disconnected",
        1 => "connecting",
        2 => "connected",
        3 => "error",
        4 => "exited",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::utils::corebridge::{ClientPane, ClientTab, ClientWorkspace};
    use crate::frontend::view_store::ViewStore;

    #[test]
    fn snapshot_uses_local_visible_tab_without_outputs() {
        let mut store = ViewStore::default();
        store.replace_topology(
            ClientWorkspace {
                id: "ws".into(),
                name: "ws".into(),
                runtime: "shell".into(),
                active: true,
                resolved_target: None,
            },
            vec![
                ClientTab {
                    id: 1,
                    name: "one".into(),
                    is_active: true,
                },
                ClientTab {
                    id: 2,
                    name: "two".into(),
                    is_active: false,
                },
            ],
            vec![
                (
                    1,
                    vec![ClientPane {
                        id: 10,
                        cols: 80,
                        rows: 24,
                        is_active: true,
                        title: "a".into(),
                    }],
                ),
                (
                    2,
                    vec![ClientPane {
                        id: 20,
                        cols: 80,
                        rows: 24,
                        is_active: true,
                        title: "b".into(),
                    }],
                ),
            ],
        );
        let view = store.workspace("ws").expect("view");
        let snap = FrameSnapshot::from_workspace_view(view, "connected", Some(2));
        assert_eq!(snap.active_tab, 2);
        assert!(snap.tabs[1].is_active);
        assert_eq!(snap.panes[0].id, 20);
        assert!(snap.outputs.is_empty());
    }
}
