//! Workspace-scoped asynchronous runtime update API.
use super::support::{cstr_opt, json_error, json_string, parse_workspace_id, MuxtermHandle};
use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// # Safety
/// `h` and `workspace_id` must be live, exclusively accessed C ABI values.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_update_start_json(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = h.as_mut() else {
            return json_error("handle is null");
        };
        let Some(id) = cstr_opt(workspace_id) else {
            return json_error("workspace id is empty");
        };
        let Some(workspace) = handle.pool_mut().get_mut(&parse_workspace_id(&id)) else {
            return json_error("workspace does not exist");
        };
        match workspace.runtime_mut().start_update() {
            Ok(()) => json_string(serde_json::json!({"ok":true})),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("runtime update start panic"))
}

/// # Safety
/// `h` must be a live, exclusively accessed C ABI handle.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_updates_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(handle) = h.as_ref() else {
            return json_error("handle is null");
        };
        let updates: Vec<_> = handle
            .pool()
            .list()
            .into_iter()
            .filter_map(|workspace| {
                let status = workspace.runtime().update_status()?;
                Some(serde_json::json!({
                    "workspace_id": workspace.id().to_string(),
                    "status": status,
                }))
            })
            .collect();
        json_string(serde_json::json!({"ok":true,"updates":updates}))
    }))
    .unwrap_or_else(|_| json_error("runtime update status panic"))
}
