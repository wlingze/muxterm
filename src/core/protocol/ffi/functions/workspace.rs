//! Workspace pool and legacy workspace-open C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::protocol::ffi::api::{
    configured_scrollback_lines, cstr_opt, discovery_timeout, json_error, json_string,
    MuxtermHandle,
};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::spec::WorkspaceSpec;

use super::catalog::resolved_target_json;

/// Create a detached tmux session through the Core discovery service.
#[no_mangle]
pub extern "C" fn muxterm_workspace_create(
    transport_type: *const c_char,
    target: *const c_char,
    socket: *const c_char,
    config_path: *const c_char,
    session: *const c_char,
    directory: *const c_char,
    timeout_ms: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let transport = cstr_opt(transport_type)
            .unwrap_or_else(|| "local".into())
            .to_ascii_lowercase();
        let target = cstr_opt(target);
        let socket = cstr_opt(socket);
        let config_path = cstr_opt(config_path);
        let Some(session) = cstr_opt(session).filter(|value| !value.trim().is_empty()) else {
            return json_error("tmux session name is required");
        };
        let Some(directory) = cstr_opt(directory).filter(|value| !value.trim().is_empty()) else {
            return json_error("tmux working directory is required");
        };

        let result = match transport.as_str() {
            "local" => crate::core::discovery::create_local_tmux_session(
                socket.as_deref(),
                &session,
                &directory,
            ),
            "ssh" => {
                let Some(alias) = target.as_deref().filter(|value| !value.trim().is_empty()) else {
                    return json_error("SSH session creation requires a host alias");
                };
                crate::core::discovery::create_ssh_tmux_session(
                    alias,
                    config_path.as_deref(),
                    socket.as_deref(),
                    &session,
                    &directory,
                    discovery_timeout(timeout_ms),
                )
            }
            _ => return json_error(format!("unsupported session transport: {transport}")),
        };
        match result {
            Ok(()) => json_string(serde_json::json!({
                "ok": true,
                "session": session,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("tmux session creation panic"))
}

/// Deprecated alias for `muxterm_workspace_create`.
#[no_mangle]
pub extern "C" fn muxterm_create_tmux_session_json(
    transport_type: *const c_char,
    target: *const c_char,
    socket: *const c_char,
    config_path: *const c_char,
    session: *const c_char,
    directory: *const c_char,
    timeout_ms: u32,
) -> *mut c_char {
    muxterm_workspace_create(
        transport_type,
        target,
        socket,
        config_path,
        session,
        directory,
        timeout_ms,
    )
}

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
