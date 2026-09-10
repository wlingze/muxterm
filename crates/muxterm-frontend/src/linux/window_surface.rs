//! PaneSurface baseline seeding and deferred render-mailbox projection.

use muxterm_protocol::WorkspaceId;

use super::{ClientEventKind, PaneSurface, UiState};

/// 把 core 里已就绪的 attach 快照播种进尚未播种的 VTE。
///
/// 窗口 present/realize 前 feed 会被 VTE 丢弃（白屏），所以只在 widget
/// 已 realized 时播种；未 realized 的 pane 保持 unseeded，等布局挂载后
/// 由下一次 refresh_ui / sync_pane_outputs 补种。
pub(super) fn seed_unseeded_pane(
    s: &mut UiState,
    view: &std::rc::Rc<PaneSurface>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) {
    let wid = s.active_ws_id();
    seed_unseeded_pane_for(s, &wid, view, pane_id, cols, rows);
}

/// VTE 只有在 realize 且二维分配都有效时才能可靠接收首帧。
pub(super) fn surface_allocation_is_seedable(realized: bool, width: i32, height: i32) -> bool {
    realized && width > 0 && height > 0
}

pub(super) fn seed_unseeded_pane_for(
    s: &mut UiState,
    wid: &WorkspaceId,
    view: &std::rc::Rc<PaneSurface>,
    pane_id: u32,
    cols: u16,
    rows: u16,
) {
    if view.is_seeded() || !view.can_paint_surface() {
        if view.is_seeded() && view.can_paint_surface() {
            view.flush_deferred_history();
            view.flush_deferred_feed();
        }
        return;
    }
    let workspace_key = wid.as_str();
    let bytes = s
        .view_store
        .take_pane_baseline(&workspace_key, pane_id)
        .or_else(|| {
            let bytes = s
                .event_pump
                .client()
                .get_workspace_pane_output(&workspace_key, pane_id);
            (!bytes.is_empty()).then_some(bytes)
        });
    if let Some(bytes) = bytes {
        tracing::info!(
            target: "muxterm::surface",
            pane = pane_id,
            bytes = bytes.len(),
            "surface baseline seed from current-generation frame"
        );
        view.seed_raw(&bytes, cols, rows);
        s.snapshot_seeded_this_batch.insert(pane_id);
        drain_view_store_render_events(s, wid, view, pane_id);
    } else {
        tracing::info!(
            target: "muxterm::surface",
            pane = pane_id,
            "pane view unseeded and no queued or compatibility baseline is available"
        );
    }
}

/// Flush render events retained while a pane had no realized Surface.
pub(super) fn drain_view_store_render_events(
    s: &mut UiState,
    wid: &WorkspaceId,
    view: &std::rc::Rc<PaneSurface>,
    pane_id: u32,
) {
    let workspace_key = wid.as_str();
    for event in s
        .view_store
        .take_pane_render_events(&workspace_key, pane_id)
    {
        match event.kind() {
            ClientEventKind::PaneHistory => view.prepend_history(&event.data),
            ClientEventKind::PaneFrame => view.feed_full(&event.data),
            ClientEventKind::PaneOutput => view.feed_output(&event.data),
            ClientEventKind::PaneSnapshot
            | ClientEventKind::PaneClosed
            | ClientEventKind::PaneResized
            | ClientEventKind::Other(_) => {}
        }
    }
    view.flush_deferred_history();
    view.flush_deferred_feed();
}
