//! Status bar and window chrome projection for the GTK frontend.

use std::time::Instant;

use gtk4::prelude::*;
use gtk4::Window;

use crate::frontend::linux::attention_ui::window_title;
use crate::frontend::linux::quickconnect::status_style::StatusBarSnapshot;
use crate::frontend::linux::status_bar::ConnectionSummary;
use crate::frontend::utils::i18n::{self, Key};

use super::window_event_pump::activity_snapshot;
use super::{parse_workspace_id, ClientRuntimeCapability, UiState};

pub(super) fn refresh_attention_chrome(s: &UiState, window: &Window) {
    let n = activity_snapshot(s).blocked_count;
    s.status.set_attention(n);
    let active_workspace = s.active_workspace_key();
    let workspace = s
        .view_store
        .workspace(&active_workspace)
        .and_then(|view| view.workspace.as_ref())
        .map(|workspace| workspace.name.clone())
        .unwrap_or_else(|| "muxterm".into());
    window.set_title(Some(&window_title(n, &workspace)));
}

pub(super) fn refresh_connection_summary(s: &mut UiState) {
    let workspace_id = s.active_workspace_key();
    let Some(workspace) = s
        .view_store
        .workspace(&workspace_id)
        .and_then(|view| view.workspace.as_ref())
    else {
        return;
    };
    let Some(id) = parse_workspace_id(&workspace.id) else {
        return;
    };
    let kind = match id.runtime.as_str() {
        "tmux-ssh" | "ssh" => "ssh",
        "tmux" => "tmux",
        _ => "local",
    };
    let host = id
        .alias
        .clone()
        .or_else(|| (!id.session.is_empty()).then(|| id.session.clone()));
    let status = match s.runtime_status {
        crate::protocol::ffi::types::BACKEND_STATUS_CONNECTED => "connected",
        crate::protocol::ffi::types::BACKEND_STATUS_CONNECTING => "connecting",
        _ => "disconnected",
    };
    let (down, up) = s.event_pump.client().traffic_bytes();
    let now = Instant::now();
    let (down_rate, up_rate) = match (s.last_traffic, s.last_traffic_at) {
        (Some((pdown, pup)), Some(at)) => {
            let dt = now.duration_since(at);
            (
                crate::frontend::format::rate_bps(pdown, down, dt),
                crate::frontend::format::rate_bps(pup, up, dt),
            )
        }
        _ => (0, 0),
    };
    s.last_traffic = Some((down, up));
    s.last_traffic_at = Some(now);
    s.status.set_connection_summary(&ConnectionSummary {
        kind: kind.into(),
        host,
        status: status.into(),
        down,
        up,
        down_rate,
        up_rate,
    });
}

pub(super) fn local_status_snapshot(
    npanes: usize,
    tabs: &[(u32, String, bool)],
) -> StatusBarSnapshot {
    let connected = i18n::tr(Key::StatusConnected);
    let panes = i18n::tr(Key::Panes);
    let close_hint = i18n::tr(Key::WindowCloseHint);
    let mut snap = crate::frontend::linux::quickconnect::status_style::snapshot_from_tabs(
        "local", npanes, tabs,
    );
    snap.left = format!("{connected} | {npanes} {panes}");
    snap.right = close_hint;
    snap.interval = 1;
    snap
}

pub(super) fn maybe_refresh_status(s: &mut UiState, force: bool) {
    let workspace_key = s.active_ws_id().as_str();
    let Some(view) = s.view_store.workspace(&workspace_key) else {
        return;
    };
    let session = view
        .workspace
        .as_ref()
        .map(|workspace| workspace.name.clone())
        .unwrap_or_default();
    let active_tab = view
        .tabs
        .iter()
        .find(|tab| tab.id == s.active_tab_id())
        .map(|tab| tab.id)
        .unwrap_or(s.active_tab);
    let npanes = view.panes.get(&active_tab).map(Vec::len).unwrap_or(0);
    let rows: Vec<(u32, String, bool)> = view
        .tabs
        .iter()
        .map(|tab| (tab.id, tab.name.clone(), tab.id == active_tab))
        .collect();
    let mut snap = if s.uses_tmux() {
        crate::frontend::linux::quickconnect::status_style::snapshot_from_tabs(
            &session, npanes, &rows,
        )
    } else {
        local_status_snapshot(npanes, &rows)
    };
    // tmux ≥3.2 订阅推送的 status-left/right 覆盖默认文案（零轮询）。
    if let Some(left) = s.status_left.as_deref() {
        snap.left = left.to_string();
    }
    if let Some(right) = s.status_right.as_deref() {
        snap.right = right.to_string();
    }
    let _ = force;
    s.status.apply(&snap);
    sync_chrome_visibility(s);
}

pub(super) fn sync_chrome_visibility(s: &UiState) {
    // 唯一 chrome：status bar 永远可见，没有第二条 tab 带。
    // worktree 创建入口只按 support() 露出（禁止 if runtime == "herdr"）。
    let worktree = s.active_supports(ClientRuntimeCapability::WorktreeList);
    s.status.set_worktree_visible(worktree);
}
