//! Runtime provider and lifecycle C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::protocol::task::Task;

use super::super::api::{json_error, json_string, task_result_code, MuxtermHandle};

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
        let MuxtermHandle { catalog, rt, .. } = handle;
        let Some(ws) = catalog.pool_mut().active_mut() else {
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
