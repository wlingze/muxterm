//! Resize projection from resident GTK surfaces into the Core command queue.

use std::collections::HashMap;
use std::time::Instant;

use gtk4::prelude::*;
use vte4::prelude::*;

use crate::frontend::linux::quickconnect::event_policy::ClientSizePolicy;

use super::window_event_pump::enqueue_workspace_resize;
use super::{active_workspace_key, ClientRuntimeCapability, UiState};

const CLIENT_SIZE_STABLE_HITS: u8 = 10;

pub(super) fn sync_window_size(s: &mut UiState) {
    let shared_client_resize = s.active_supports(ClientRuntimeCapability::SharedClientResize);
    if !shared_client_resize {
        sync_visible_pane_sizes(s);
        return;
    }
    let Some(view) = s.active_layout().pane(s.active_pane) else {
        return;
    };
    let term = view.terminal();
    let cw = term.char_width();
    let ch = term.char_height();
    if cw <= 0 || ch <= 0 {
        return;
    }
    let root_w = s.active_layout().root_box.width().max(0) as u64;
    let root_h = s.active_layout().root_box.height().max(0) as u64;
    if root_w == 0 || root_h == 0 {
        return;
    }
    let allocated = term.width() > 0 && term.height() > 0;
    let workspace_key = s.active_ws_id().as_str();
    let active_tab = s.active_tab_id();
    let multi_pane = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.panes.get(&active_tab))
        .is_some_and(|panes| panes.len() > 1);
    let cols = match ClientSizePolicy::cols(term.column_count(), allocated, root_w, cw, multi_pane)
    {
        Some(cols) => cols,
        None => return,
    };
    let rows = match ClientSizePolicy::rows(root_h, ch) {
        Some(rows) => rows,
        None => return,
    };
    if s.last_client_size == Some((None, cols, rows)) {
        s.pending_client_size = None;
        s.pending_client_hits = 0;
        return;
    }
    if s.pending_client_size == Some((cols, rows)) {
        s.pending_client_hits = s.pending_client_hits.saturating_add(1);
    } else {
        s.pending_client_size = Some((cols, rows));
        s.pending_client_hits = 1;
    }
    if s.pending_client_hits < CLIENT_SIZE_STABLE_HITS {
        return;
    }
    s.last_client_size = Some((None, cols, rows));
    s.pending_client_size = None;
    s.pending_client_hits = 0;
    let workspace_id = active_workspace_key(s);
    enqueue_workspace_resize(s, &workspace_id, None, cols, rows);
}

/// Herdr 没有 SharedClientResize：每个可见 split 格子按自己的 VTE 分配
/// 发 ResizePane。只同步 active pane 会让 0218.log 里 54/57 停在 27×12。
pub(super) fn sync_visible_pane_sizes(s: &mut UiState) {
    if s.hold_pane_resize_until
        .is_some_and(|until| Instant::now() < until)
    {
        return;
    }
    s.hold_pane_resize_until = None;
    let workspace_key = s.active_ws_id().as_str();
    let active_tab = s.active_tab_id();
    let pane_ids: Vec<u32> = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.panes.get(&active_tab))
        .map(|panes| panes.iter().map(|pane| pane.id).collect())
        .unwrap_or_default();
    let mut measured = Vec::new();
    for pane in pane_ids {
        let Some(view) = s.active_layout().pane(pane) else {
            continue;
        };
        let term = view.terminal();
        let cw = term.char_width();
        let ch = term.char_height();
        if cw <= 0 || ch <= 0 || term.width() <= 0 || term.height() <= 0 {
            continue;
        }
        let cols = (i64::from(term.width()) / cw).clamp(2, i64::from(u16::MAX)) as u16;
        let rows = (i64::from(term.height()) / ch).clamp(1, i64::from(u16::MAX)) as u16;
        view.ensure_grid_size(cols, rows);
        measured.push((pane, cols, rows));
        if pane == s.active_pane {
            s.last_client_size = Some((Some(pane), cols, rows));
        }
    }
    let resizes = pending_pane_resizes(&s.last_pane_sizes, &measured);
    for &(pane, cols, rows) in &resizes {
        s.last_pane_sizes.insert(pane, (cols, rows));
    }
    for (pane, cols, rows) in resizes {
        let workspace_id = active_workspace_key(s);
        enqueue_workspace_resize(s, &workspace_id, Some(pane), cols, rows);
    }
}

/// 0218.log：三个可见格子都要有自己的 ResizePane；已同步过的尺寸跳过。
pub(super) fn pending_pane_resizes(
    last: &HashMap<u32, (u16, u16)>,
    measured: &[(u32, u16, u16)],
) -> Vec<(u32, u16, u16)> {
    measured
        .iter()
        .copied()
        .filter(|(pane, cols, rows)| last.get(pane) != Some(&(*cols, *rows)))
        .collect()
}
