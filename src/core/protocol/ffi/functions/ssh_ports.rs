//! SSH application port discovery and explicit local forwarding.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::support::{cstr_opt, json_error, json_string, parse_workspace_id, MuxtermHandle};

/// Return application ports detected from this SSH workspace's pane output.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_ports_json(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return json_error("handle is null");
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return json_error("workspace id is empty");
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        if let Err(error) = (*handle).refresh_ssh_ports(&workspace_id) {
            return json_error(error);
        }
        match (*handle).ssh_port_snapshot(&workspace_id) {
            Ok(listing) => json_string(serde_json::json!({
                "ok": true,
                "ports": listing.ports,
                "scan_pending": listing.scan_pending,
                "scan_error": listing.scan_error,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("SSH port snapshot panic"))
}

/// Start an explicit local loopback forward. OpenSSH startup is asynchronous.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_forward(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
) -> *mut c_char {
    ssh_port_forward_json(handle, workspace_id, remote_port, false)
}

/// Start an SSH port forward with optional LAN access (0.0.0.0 bind).
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_forward_with_access(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
    allow_lan: bool,
) -> *mut c_char {
    ssh_port_forward_json(handle, workspace_id, remote_port, allow_lan)
}

unsafe fn ssh_port_forward_json(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
    allow_lan: bool,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return json_error("handle is null");
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return json_error("workspace id is empty");
        };
        let Ok(remote_port) = u16::try_from(remote_port) else {
            return json_error("remote port is outside the valid range");
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        match (*handle).forward_ssh_port(&workspace_id, remote_port, allow_lan) {
            Ok(()) => json_string(serde_json::json!({ "ok": true, "pending": true })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("SSH port forward panic"))
}

/// Ignore a discovered port for the lifetime of this workspace.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_ignore(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return -1;
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return -1;
        };
        let Ok(remote_port) = u16::try_from(remote_port) else {
            return -1;
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        i32::from((*handle).ignore_ssh_port(&workspace_id, remote_port))
    }))
    .unwrap_or(-1)
}

/// Stop an active local port forward.
///
/// # Safety
/// `handle`, when non-null, must point to a live Muxterm handle. `workspace_id`,
/// when non-null, must point to a NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_ssh_port_stop(
    handle: *mut MuxtermHandle,
    workspace_id: *const c_char,
    remote_port: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return -1;
        }
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return -1;
        };
        let Ok(remote_port) = u16::try_from(remote_port) else {
            return -1;
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        i32::from((*handle).stop_ssh_port_forward(&workspace_id, remote_port))
    }))
    .unwrap_or(-1)
}
