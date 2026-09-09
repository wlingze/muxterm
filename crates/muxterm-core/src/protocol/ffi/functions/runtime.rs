//! Runtime provider and lifecycle C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::protocol::task::Task;
use crate::runtime::HerdrRuntime;
use crate::types::PaneId;

use super::support::{json_error, json_string, parse_workspace_id, MuxtermHandle};
use super::task::task_result_code;

/// Return whether the active tmux backend has status-bar subscriptions enabled.
///
/// # Safety
/// `handle` is a live handle returned by a constructor and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_status_subscription_active(handle: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return 0;
        }
        let h = &*handle;
        i32::from(
            h.active_workspace()
                .map(|w| w.runtime().status_subscriptions_active())
                .unwrap_or(false),
        )
    }))
    .unwrap_or(0)
}

/// Return cumulative bytes read by the active runtime.
///
/// # Safety
/// `handle` is a live handle returned by a constructor and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_traffic_down(handle: *mut MuxtermHandle) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return 0;
        }
        let h = &*handle;
        h.active_workspace()
            .map(|w| w.runtime().traffic_bytes().0)
            .unwrap_or(0)
    }))
    .unwrap_or(0)
}

/// Return cumulative bytes written by the active runtime.
///
/// # Safety
/// `handle` is a live handle returned by a constructor and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_traffic_up(handle: *mut MuxtermHandle) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return 0;
        }
        let h = &*handle;
        h.active_workspace()
            .map(|w| w.runtime().traffic_bytes().1)
            .unwrap_or(0)
    }))
    .unwrap_or(0)
}

/// List the registered runtime providers and their capabilities.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_runtime_list_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let list = (*h).catalog.runtime_list();
        json_string(serde_json::json!({
            "ok": true,
            "runtimes": list.iter().map(|r| serde_json::json!({
                "id": r.id,
                "name": r.name,
                "support": r.support.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>(),
                "accepted_transports": r.accepted_transports,
            })).collect::<Vec<_>>(),
        }))
    }))
    .unwrap_or_else(|_| json_error("runtime list panic"))
}

/// Return Herdr stream diagnostics for one product workspace pane.
///
/// This is intentionally a Core-owned diagnostic DTO: GTK must not downcast a
/// live Runtime or borrow the WorkspacePool just to support an E2E probe.
/// Non-Herdr workspaces return a successful `null` probe.
///
/// # Safety
/// `h` is a live handle and `workspace_id` is a NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_herdr_probe_json(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let Some(workspace_id) = super::support::cstr_opt(workspace_id) else {
            return json_error("workspace id 为空");
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        let handle = &*h;
        let Some(workspace) = handle.pool().get(&workspace_id) else {
            return json_error("workspace 不存在");
        };
        let Some(runtime) = workspace.runtime().as_any().downcast_ref::<HerdrRuntime>() else {
            return json_string(serde_json::json!({"ok": true, "probe": null}));
        };
        let pane = PaneId(pane_id);
        json_string(serde_json::json!({
            "ok": true,
            "probe": {
                "stream_starts": runtime.test_stream_starts(pane),
                "control_takeover_starts": runtime.test_control_takeover_starts(pane),
                "takeover_suppressed": runtime.test_takeover_suppressed(pane),
                "actual_mode": format!("{:?}", runtime.test_actual_mode(pane)),
            },
        }))
    }))
    .unwrap_or_else(|_| json_error("Herdr probe panic"))
}

/// Connect the active workspace backend. Returns 0 on success, -1 on error.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_connect(h: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let (rt, pool) = (&handle.rt, &mut handle.pool);
        let Some(ws) = pool.active_mut() else {
            return -1;
        };
        match rt.block_on(ws.connect()) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    }))
    .unwrap_or(-1)
}

/// Shut down all workspace backends. Returns 0 on success.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_shutdown(h: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        handle.pool_mut().shutdown_all();
        0
    }))
    .unwrap_or(-1)
}

/// Detach the current control client while keeping the tmux session/daemon.
///
/// This is distinct from `muxterm_shutdown`; callers should still free the
/// handle afterwards. All failures are converted to -1 at the FFI boundary.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_detach(h: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        match handle.active_workspace_mut() {
            Some(ws) => task_result_code(ws.execute(Task::Detach)),
            None => -1,
        }
    }))
    .unwrap_or(-1)
}
