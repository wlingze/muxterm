//! Transport registry and discovery C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use super::super::api::{cstr_opt, json_error, json_string, MuxtermHandle};

/// List the registered transport providers.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_transport_list_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let list = (*h).catalog.transport_list();
        json_string(serde_json::json!({
            "ok": true,
            "transports": list.iter().map(|t| serde_json::json!({
                "id": t.id,
                "name": t.name,
            })).collect::<Vec<_>>(),
        }))
    }))
    .unwrap_or_else(|_| json_error("transport list panic"))
}

/// List targets exposed by a transport (local singleton or SSH hosts).
///
/// # Safety
/// `h` is valid and has not been freed; `transport` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_discover_targets_json(
    h: *mut MuxtermHandle,
    transport: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let transport = cstr_opt(transport).unwrap_or_else(|| "local".into());
        match (*h).catalog.discover_targets(&transport) {
            Ok(targets) => json_string(serde_json::json!({
                "ok": true,
                "targets": targets.iter().map(|t| serde_json::json!({
                    "id": t.id,
                    "name": t.name,
                })).collect::<Vec<_>>(),
            })),
            Err(e) => json_error(e),
        }
    }))
    .unwrap_or_else(|_| json_error("discover targets panic"))
}

/// Fan out discovery of attachable sessions on a target.
///
/// # Safety
/// `h` is valid and has not been freed; `transport` and `target` are
/// NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_discover_sessions_json(
    h: *mut MuxtermHandle,
    transport: *const c_char,
    target: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let transport = cstr_opt(transport).unwrap_or_else(|| "local".into());
        let target = cstr_opt(target).unwrap_or_default();
        // transport=all fans out across every configured connection target.
        match (*h).catalog.discover_sessions(&transport, &target) {
            Ok(rows) => json_string(serde_json::json!({
                "ok": true,
                "workspaces": rows.iter().map(session_candidate_json).collect::<Vec<_>>(),
            })),
            Err(e) => json_error(e),
        }
    }))
    .unwrap_or_else(|_| json_error("discover sessions panic"))
}

/// Discover local or SSH sessions through the Core catalog.
///
/// `transport_type` is `local` or `ssh`; for SSH, `target` is the
/// `~/.ssh/config` alias. The socket/config/timeout parameters remain part of
/// the compatibility ABI; catalog discovery owns the actual connection path.
#[no_mangle]
pub extern "C" fn muxterm_discover_workspaces_json(
    transport_type: *const c_char,
    target: *const c_char,
    socket: *const c_char,
    config_path: *const c_char,
    timeout_ms: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let transport = cstr_opt(transport_type)
            .unwrap_or_else(|| "local".into())
            .to_ascii_lowercase();
        let target = cstr_opt(target).unwrap_or_default();
        let _socket = cstr_opt(socket);
        let _config_path = cstr_opt(config_path);
        let _timeout_ms = timeout_ms;
        let mut catalog = crate::core::catalog::Catalog::with_builtins();
        match catalog.discover_sessions(&transport, &target) {
            Ok(rows) => json_string(serde_json::json!({
                "ok": true,
                "workspaces": rows.iter().map(session_candidate_json).collect::<Vec<_>>(),
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("tmux session discovery panic"))
}

/// List sessions on a specific local or SSH tmux server.
#[no_mangle]
pub extern "C" fn muxterm_discover_tmux_sessions_json(
    transport_type: *const c_char,
    target: *const c_char,
    socket: *const c_char,
    config_path: *const c_char,
    timeout_ms: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let transport = cstr_opt(transport_type)
            .unwrap_or_else(|| "local".into())
            .to_ascii_lowercase();
        let target = cstr_opt(target).unwrap_or_default();
        let socket = cstr_opt(socket);
        let config_path = cstr_opt(config_path);
        let timeout = Duration::from_millis(u64::from(timeout_ms.max(1)));
        let result = match transport.as_str() {
            "local" => Ok(crate::core::discovery::list_local_tmux_sessions(
                socket.as_deref(),
            )),
            "ssh" => crate::core::discovery::list_ssh_tmux_sessions(
                &target,
                config_path.as_deref(),
                socket.as_deref(),
                timeout,
            ),
            other => Err(anyhow::anyhow!(
                "unknown tmux discovery transport '{other}'"
            )),
        };
        match result {
            Ok(sessions) => json_string(serde_json::json!({
                "ok": true,
                "sessions": sessions.iter().map(|session| serde_json::json!({
                    "name": session.name,
                    "windows": session.windows,
                    "attached": session.attached,
                    "created": session.created,
                })).collect::<Vec<_>>(),
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("tmux session discovery panic"))
}

pub(crate) fn session_candidate_json(
    candidate: &crate::core::protocol::candidate::ExistingCandidate,
) -> serde_json::Value {
    let target = if candidate.transport_id == "local" {
        "local".to_string()
    } else {
        candidate.target.clone()
    };
    serde_json::json!({
        "id": format!("{}/{}/{}/{}", candidate.transport_id, target, candidate.runtime_id, candidate.name),
        "name": candidate.name,
        "runtime": candidate.runtime_id,
        "transport": candidate.transport_id,
        "target": target,
        "in_pool": false,
        "session": candidate.session,
        "socket": candidate.socket,
        "workspace_id": candidate.workspace_id,
    })
}
