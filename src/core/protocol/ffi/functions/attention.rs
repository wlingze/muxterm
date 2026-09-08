//! Attention/activity query and acknowledgement C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::attention::engine::AttentionNotificationKind;

use super::super::api::{cstr_opt, json_error, json_string, MuxtermHandle};

/// Return the current attention snapshot as JSON.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_snapshot(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &*h;
        let workspaces: Vec<serde_json::Value> = handle
            .attention
            .snapshot()
            .into_iter()
            .map(|ws| {
                let path = handle
                    .pool()
                    .list()
                    .iter()
                    .find(|w| w.id().replica_id() == ws.workspace_id)
                    .map(|w| w.id().path.as_str())
                    .filter(|p| !p.trim().is_empty())
                    .unwrap_or("~");
                serde_json::json!({
                    "workspace_id": ws.workspace_id,
                    "path": path,
                    "blocked": ws.blocked,
                    "done": ws.done,
                    "working": ws.working,
                    "panes": ws.panes.iter().map(|p| {
                        serde_json::json!({
                            "pane_id": p.pane_id,
                            "status": format!("{:?}", p.status).to_lowercase(),
                            "acknowledged": p.acknowledged,
                            "last_line": p.last_line,
                            "seq": p.seq,
                            "process_name": p.process_name,
                            "process_is_agent": p.process_is_agent,
                            "agent_name": p.agent_name,
                            "shell_name": p.shell_name,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        json_string(serde_json::json!({
            "ok": true,
            "blocked_count": handle.attention.blocked_workspace_count(),
            "workspaces": workspaces,
        }))
    }))
    .unwrap_or_else(|_| json_error("attention snapshot panic"))
}

/// Take notifications emitted since the previous call.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_take_notifications(
    h: *mut MuxtermHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let notifications = handle.attention.take_notifications();
        let blocked = notifications
            .iter()
            .filter(|n| n.kind == AttentionNotificationKind::Blocked)
            .map(|n| n.workspace_id.clone())
            .collect::<Vec<_>>();
        let done = notifications
            .iter()
            .filter(|n| n.kind == AttentionNotificationKind::Done)
            .map(|n| n.workspace_id.clone())
            .collect::<Vec<_>>();
        let records = notifications
            .into_iter()
            .map(|n| {
                serde_json::json!({
                    "workspace_id": n.workspace_id,
                    "pane_id": n.pane_id,
                    "kind": match n.kind {
                        AttentionNotificationKind::Blocked => "blocked",
                        AttentionNotificationKind::Done => "done",
                    },
                    "process_name": n.process_name,
                    "last_line": n.last_line,
                    "seq": n.seq,
                })
            })
            .collect::<Vec<_>>();
        json_string(serde_json::json!({
            "ok": true,
            "notifications": records,
            "blocked": blocked,
            "done": done,
        }))
    }))
    .unwrap_or_else(|_| json_error("attention notifications panic"))
}

/// Mark an active pane visible (Done becomes Idle; Blocked remains).
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_on_became_visible(
    h: *mut MuxtermHandle,
    pane_id: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle
            .attention
            .on_became_visible(&ws_id.replica_id(), pane_id);
        0
    }))
    .unwrap_or(-1)
}

/// Acknowledge an active pane's notification.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_acknowledge(h: *mut MuxtermHandle, pane_id: u32) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle.attention.acknowledge(&ws_id.replica_id(), pane_id);
        0
    }))
    .unwrap_or(-1)
}

/// Update a pane's process name for attention display.
///
/// # Safety
/// `h` is valid and has not been freed; `name` is NUL-terminated or null.
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_set_process_name(
    h: *mut MuxtermHandle,
    pane_id: u32,
    name: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle
            .attention
            .set_process_name(&ws_id.replica_id(), pane_id, cstr_opt(name));
        0
    }))
    .unwrap_or(-1)
}

/// Mute an active pane for a number of seconds.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_mute(
    h: *mut MuxtermHandle,
    pane_id: u32,
    seconds: u64,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle.attention.mute_for(
            &ws_id.replica_id(),
            pane_id,
            std::time::Duration::from_secs(seconds),
        );
        0
    }))
    .unwrap_or(-1)
}
