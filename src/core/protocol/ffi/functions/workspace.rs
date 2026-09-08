//! Workspace pool and legacy workspace-open C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::protocol::ffi::api::{
    configured_scrollback_lines, cstr_opt, json_error, json_string, MuxtermHandle,
};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::spec::WorkspaceSpec;

use super::catalog::resolved_target_json;

/// Open a workspace through the compatibility WorkspaceSpec-shaped ABI.
///
/// Product callers should use `muxterm_open_json`; this function remains for
/// tests and migration clients while the frontend moves to OpenRequest.
///
/// # Safety
/// `h` is valid and not freed; all string arguments are NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_open(
    h: *mut MuxtermHandle,
    transport: *const c_char,
    alias: *const c_char,
    session: *const c_char,
    runtime: *const c_char,
    path: *const c_char,
    socket: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let transport = cstr_opt(transport).unwrap_or_else(|| "local".into());
        let alias = cstr_opt(alias);
        let session = cstr_opt(session).unwrap_or_default();
        let runtime = cstr_opt(runtime).unwrap_or_else(|| "shell".into());
        let path = cstr_opt(path).unwrap_or_default();
        let socket = cstr_opt(socket);

        let handle = &mut *h;
        let spec = WorkspaceSpec {
            transport: transport.clone(),
            alias: alias.clone(),
            session: session.clone(),
            runtime: runtime.clone(),
            path: path.clone(),
            socket: socket.clone(),
            create: false,
            scrollback_lines: configured_scrollback_lines() as u32,
            provenance: None,
            template: None,
        };
        let fut = handle.catalog.open(&spec);
        match handle.rt.block_on(fut) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }))
    .unwrap_or(-1)
}

/// List all live workspaces in the Core-owned pool.
///
/// # Safety
/// `h` is valid and not freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_list(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &*h;
        let workspaces: Vec<serde_json::Value> = handle
            .pool()
            .list()
            .into_iter()
            .map(|w| {
                let resolved = w.resolved_target().map(resolved_target_json);
                serde_json::json!({
                    "id": w.id().as_str(),
                    "name": w.name(),
                    "runtime": w.state().workspace_runtime(),
                    "active": handle.pool().active_id() == Some(w.id()),
                    "resolved_target": resolved,
                })
            })
            .collect();
        json_string(serde_json::json!({ "ok": true, "workspaces": workspaces }))
    }))
    .unwrap_or_else(|_| json_error("workspace list panic"))
}

/// Activate a workspace in the Core-owned pool.
///
/// # Safety
/// `h` is valid and not freed; `id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_activate(
    h: *mut MuxtermHandle,
    id: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let Some(id) = cstr_opt(id) else {
            return -1;
        };
        let wid = parse_workspace_id(&id);
        let handle = &mut *h;
        match handle.pool_mut().activate(&wid) {
            Some(_) => 0,
            None => -1,
        }
    }))
    .unwrap_or(-1)
}

/// Close a workspace in the Core-owned pool.
///
/// # Safety
/// `h` is valid and not freed; `id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_close(h: *mut MuxtermHandle, id: *const c_char) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let Some(id) = cstr_opt(id) else {
            return -1;
        };
        let wid = parse_workspace_id(&id);
        let handle = &mut *h;
        if handle.pool_mut().close(&wid) {
            0
        } else {
            -1
        }
    }))
    .unwrap_or(-1)
}

fn parse_workspace_id(id: &str) -> WorkspaceId {
    let parts: Vec<&str> = id.splitn(5, '/').collect();
    let transport = parts.first().copied().unwrap_or("").to_string();
    let alias = parts
        .get(1)
        .copied()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let session = parts.get(2).copied().unwrap_or("").to_string();
    let runtime = parts.get(3).copied().unwrap_or("").to_string();
    let path = parts.get(4).copied().unwrap_or("").to_string();
    WorkspaceId::new(&transport, alias.as_deref(), &session, &runtime, &path)
}
