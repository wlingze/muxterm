//! TUI-owned view models built from owned FFI client values.

use std::collections::HashMap;

use crate::ffi_client::{ClientLayout, ClientPane, ClientTab, FfiClient};

pub type TuiLayout = ClientLayout;
pub type TuiPane = ClientPane;
pub type TuiTab = ClientTab;

/// All data needed to render one TUI frame.
#[derive(Debug, Clone, Default)]
pub struct FrameSnapshot {
    pub tabs: Vec<TuiTab>,
    pub panes: Vec<TuiPane>,
    pub layout: Option<TuiLayout>,
    /// pane_id → cumulative output copied from the FFI boundary.
    pub outputs: HashMap<u32, Vec<u8>>,
    pub status: String,
    pub active_tab: u32,
    pub active_pane: u32,
}

impl FrameSnapshot {
    /// Compose a render view from owned values exposed by the shared client.
    pub fn from_client(client: &FfiClient) -> Self {
        let tabs = client.get_tabs();
        let active_tab = tabs
            .iter()
            .find(|tab| tab.is_active)
            .map(|tab| tab.id)
            .or_else(|| tabs.first().map(|tab| tab.id))
            .unwrap_or(0);
        let panes = client.get_panes(active_tab);
        let active_pane = panes
            .iter()
            .find(|pane| pane.is_active)
            .map(|pane| pane.id)
            .or_else(|| panes.first().map(|pane| pane.id))
            .unwrap_or(0);
        let layout = client.get_layout(active_tab);
        let mut outputs = HashMap::new();
        for pane in &panes {
            outputs.insert(pane.id, client.get_pane_output(pane.id));
        }
        if let Some(layout) = &layout {
            collect_layout_panes(layout, &mut |pane_id| {
                outputs
                    .entry(pane_id)
                    .or_insert_with(|| client.get_pane_output(pane_id));
            });
        }

        Self {
            tabs,
            panes,
            layout,
            outputs,
            status: status_label(client.status_code()).to_string(),
            active_tab,
            active_pane,
        }
    }
}

fn status_label(code: u32) -> &'static str {
    match code {
        0 => "disconnected",
        1 => "connecting",
        2 => "connected",
        3 => "error",
        4 => "exited",
        _ => "unknown",
    }
}

fn collect_layout_panes(layout: &TuiLayout, visit: &mut dyn FnMut(u32)) {
    match layout {
        TuiLayout::Leaf { pane_id } => visit(*pane_id),
        TuiLayout::Split { first, second, .. } => {
            collect_layout_panes(first, visit);
            collect_layout_panes(second, visit);
        }
    }
}
