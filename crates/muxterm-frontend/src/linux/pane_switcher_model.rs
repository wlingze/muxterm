//! Pane switcher data model and Quick Pick projection.

use crate::linux::quick_pick::QuickPickItem;

/// Pane switcher identity retained for compatibility with the product model;
/// layout ownership remains in the Core snapshot and `LayoutHost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabKey {
    Local(u64),
    TmuxWindow(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalPaneId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneKey {
    Local(LocalPaneId),
    Tmux(u32),
}

/// One pane that can be selected by the user.
#[derive(Debug, Clone)]
pub struct PaneEntry {
    pub tab: TabKey,
    pub pane: PaneKey,
    pub name: String,
    /// Display text such as `1:bash` or `2:vim · pane2`.
    pub label: String,
    pub detail: Option<String>,
}

/// Project pane entries into the shared Quick Pick model.
pub fn pane_entries_to_items(panes: &[PaneEntry]) -> Vec<QuickPickItem> {
    panes
        .iter()
        .enumerate()
        .map(|(i, pane)| QuickPickItem {
            id: i.to_string(),
            label: pane.label.clone(),
            detail: pane.detail.clone(),
        })
        .collect()
}
