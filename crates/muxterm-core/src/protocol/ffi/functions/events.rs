//! Workspace event conversion and polling C ABI functions.

use crate::protocol::state::StateChange;
use crate::protocol::WorkspaceId;

use super::super::types::{
    CStateChange, CWorkspaceStateChange, BACKEND_STATUS_CONNECTED, BACKEND_STATUS_CONNECTING,
    BACKEND_STATUS_DISCONNECTED, BACKEND_STATUS_ERROR, BACKEND_STATUS_EXITED,
    STATE_ACTIVE_PANE_CHANGED, STATE_ACTIVE_TAB_CHANGED, STATE_BACKEND_STATUS,
    STATE_LAYOUT_CHANGED, STATE_MUTATION_SETTLED, STATE_OTHER, STATE_PANE_ADDED,
    STATE_PANE_AGENT_CHANGED, STATE_PANE_CLOSED, STATE_PANE_FRAME, STATE_PANE_HISTORY,
    STATE_PANE_OUTPUT, STATE_PANE_RESIZED, STATE_PANE_SNAPSHOT, STATE_POOL_CHANGED,
    STATE_STATUS_SUBSCRIPTION, STATE_TAB_ADDED, STATE_TAB_CLOSED, STATE_TAB_ORDER_CHANGED,
    STATE_TAB_RENAMED, STATE_WORKSPACE_RENAMED,
};
use super::support::MuxtermHandle;

pub(crate) fn state_change_to_c(handle: &mut MuxtermHandle, ev: &StateChange) -> CStateChange {
    let mut out = CStateChange::default();
    match ev {
        StateChange::PaneOutput { pane, data } => {
            out.type_ = STATE_PANE_OUTPUT;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::PaneFrame { pane, data } => {
            out.type_ = STATE_PANE_FRAME;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::PaneSnapshot { pane, data } => {
            out.type_ = STATE_PANE_SNAPSHOT;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::PaneHistory { pane, data } => {
            out.type_ = STATE_PANE_HISTORY;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::TabAdded { tab } => {
            out.type_ = STATE_TAB_ADDED;
            out.tab_id = tab.0;
        }
        StateChange::TabClosed { tab } => {
            out.type_ = STATE_TAB_CLOSED;
            out.tab_id = tab.0;
        }
        StateChange::LayoutChanged { tab, .. } => {
            out.type_ = STATE_LAYOUT_CHANGED;
            out.tab_id = tab.0;
        }
        StateChange::PaneAdded { pane, tab } => {
            out.type_ = STATE_PANE_ADDED;
            out.pane_id = pane.0;
            out.tab_id = tab.0;
        }
        StateChange::PaneClosed { pane } => {
            out.type_ = STATE_PANE_CLOSED;
            out.pane_id = pane.0;
        }
        StateChange::ActiveTabChanged { tab } => {
            out.type_ = STATE_ACTIVE_TAB_CHANGED;
            out.tab_id = tab.0;
        }
        StateChange::ActivePaneChanged { tab, pane } => {
            out.type_ = STATE_ACTIVE_PANE_CHANGED;
            out.tab_id = tab.0;
            out.pane_id = pane.0;
        }
        StateChange::TabRenamed { tab, name } => {
            out.type_ = STATE_TAB_RENAMED;
            out.tab_id = tab.0;
            out.name = handle.push_name(name);
        }
        StateChange::TabOrderChanged => {
            out.type_ = STATE_TAB_ORDER_CHANGED;
        }
        StateChange::PaneResized { pane, cols, rows } => {
            out.type_ = STATE_PANE_RESIZED;
            out.pane_id = pane.0;
            let bytes = [
                *cols as u8,
                (*cols >> 8) as u8,
                *rows as u8,
                (*rows >> 8) as u8,
            ];
            let (p, n) = handle.push_data(&bytes);
            out.data = p;
            out.data_len = n;
        }
        StateChange::PaneAgentChanged {
            pane,
            agent,
            initial,
        } => {
            out.type_ = STATE_PANE_AGENT_CHANGED;
            out.pane_id = pane.0;
            let payload = serde_json::to_vec(&serde_json::json!({
                "initial": initial,
                "agent": agent,
            }))
            .unwrap_or_else(|_| b"{\"initial\":false,\"agent\":null}".to_vec());
            let (ptr, len) = handle.push_data(&payload);
            out.data = ptr;
            out.data_len = len;
        }
        StateChange::StatusBarSubscription { name, value, pane } => {
            out.type_ = STATE_STATUS_SUBSCRIPTION;
            out.pane_id = pane.map(|p| p.0).unwrap_or(0);
            out.name = handle.push_name(name);
            let (ptr, len) = handle.push_data(value.as_bytes());
            out.data = ptr;
            out.data_len = len;
        }
        StateChange::WorkspaceRenamed { name } => {
            out.type_ = STATE_WORKSPACE_RENAMED;
            out.name = handle.push_name(name);
        }
        StateChange::PoolChanged => {
            out.type_ = STATE_POOL_CHANGED;
        }
        StateChange::BackendStatusChanged(status) => {
            out.type_ = STATE_BACKEND_STATUS;
            out.pane_id = match status {
                crate::protocol::state::BackendStatus::Disconnected => BACKEND_STATUS_DISCONNECTED,
                crate::protocol::state::BackendStatus::Connecting => BACKEND_STATUS_CONNECTING,
                crate::protocol::state::BackendStatus::Connected => BACKEND_STATUS_CONNECTED,
                crate::protocol::state::BackendStatus::Error => BACKEND_STATUS_ERROR,
                crate::protocol::state::BackendStatus::Exited => BACKEND_STATUS_EXITED,
            };
        }
        StateChange::PaneTitleChanged { pane, title } => {
            out.type_ = STATE_OTHER;
            out.pane_id = pane.0;
            out.name = handle.push_name(title);
        }
        StateChange::PaneIndexSnapshot { .. } => {
            out.type_ = STATE_OTHER;
        }
        StateChange::MutationSettled {
            operation_id,
            kind,
            result,
        } => {
            out.type_ = STATE_MUTATION_SETTLED;
            let payload = serde_json::to_vec(&serde_json::json!({
                "operation_id": operation_id,
                "kind": kind,
                "result": result,
            }))
            .unwrap_or_else(|_| {
                b"{\"operation_id\":0,\"kind\":\"new_tab\",\"result\":{\"stage\":\"queue\"}}"
                    .to_vec()
            });
            let (ptr, len) = handle.push_data(&payload);
            out.data = ptr;
            out.data_len = len;
        }
    }
    out
}

/// Poll active workspace events through the legacy workspace-less ABI.
///
/// # Safety
/// `out` points to at least `max_count` elements and remains valid for the call.
#[no_mangle]
pub unsafe extern "C" fn muxterm_poll_events(
    h: *mut MuxtermHandle,
    out: *mut CStateChange,
    max_count: i32,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if h.is_null() || out.is_null() || max_count <= 0 {
            return -1;
        }
        let handle = &mut *h;
        handle.clear_event_bufs();
        for (ws_id, batch) in handle.pool_mut().poll_background_batches() {
            handle.apply_attention_for_batch(&ws_id, &batch);
            handle.defer_batch(ws_id, batch);
        }
        let active_id = handle.pool().active_id().cloned();
        let active_batch = handle
            .active_workspace_mut()
            .map(|workspace| workspace.refresh_batch());
        if let (Some(ws_id), Some(batch)) = (active_id.as_ref(), active_batch) {
            handle.apply_attention_for_batch(ws_id, &batch);
            handle.defer_batch(ws_id.clone(), batch);
        }
        let ready = handle.take_deferred_events(active_id.as_ref(), max_count as usize);
        let n = ready.len();
        let slice = std::slice::from_raw_parts_mut(out, n);
        for (i, (_ws_id, ev)) in ready.iter().enumerate() {
            let c = state_change_to_c(handle, ev);
            if let StateChange::PaneOutput { pane, data } | StateChange::PaneFrame { pane, data } =
                ev
            {
                if let Some(cb) = handle.callbacks.on_output {
                    cb(pane.0, data.as_ptr(), data.len());
                }
            }
            if let Some(cb) = handle.callbacks.on_state_change {
                cb(&c);
            }
            slice[i] = c;
        }
        n as i32
    }))
    .unwrap_or(-1)
}

/// Poll active and background workspace events with full WorkspaceId wrappers.
///
/// # Safety
/// `out` points to at least `max_count` elements and remains valid for the call.
#[no_mangle]
pub unsafe extern "C" fn muxterm_poll_workspace_events(
    h: *mut MuxtermHandle,
    out: *mut CWorkspaceStateChange,
    max_count: i32,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if h.is_null() || out.is_null() || max_count <= 0 {
            return -1;
        }
        let handle = &mut *h;
        handle.clear_event_bufs();
        for (ws_id, batch) in handle.pool_mut().poll_background_batches() {
            handle.apply_attention_for_batch(&ws_id, &batch);
            handle.defer_batch(ws_id, batch);
        }
        let active_id = handle.pool().active_id().cloned();
        let active_batch = handle
            .active_workspace_mut()
            .map(|workspace| workspace.refresh_batch());
        if let (Some(ws_id), Some(batch)) = (active_id.as_ref(), active_batch) {
            handle.apply_attention_for_batch(ws_id, &batch);
            handle.defer_batch(ws_id.clone(), batch);
        }
        let ready = handle.take_deferred_events(None, max_count as usize);
        let n = ready.len();
        let slice = std::slice::from_raw_parts_mut(out, n);
        for (i, (ws_id, ev)) in ready.iter().enumerate() {
            let c = state_change_to_c(handle, ev);
            let ws_name = handle.push_workspace_id(&ws_id.to_string());
            slice[i] = CWorkspaceStateChange {
                workspace_id: ws_name,
                event: c,
            };
        }
        n as i32
    }))
    .unwrap_or(-1)
}
