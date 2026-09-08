//! Pane snapshot, viewport, and history query C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::types::PaneId;
use crate::core::workspace::workspace::Workspace;

use super::super::api::{cstr_opt, json_error, json_string, resolve_c_io_pane, MuxtermHandle};
use super::support::parse_workspace_id;

fn workspace_pane(
    handle: &MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
) -> Option<(&Workspace, PaneId)> {
    let workspace_id = cstr_opt(workspace_id)?;
    let workspace_id = parse_workspace_id(&workspace_id);
    let workspace = handle.pool().get(&workspace_id)?;
    let pane = resolve_c_io_pane(pane_id, workspace)?;
    Some((workspace, pane))
}

fn workspace_pane_mut(
    handle: &mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
) -> Option<(&mut Workspace, PaneId)> {
    let workspace_id = cstr_opt(workspace_id)?;
    let workspace_id = parse_workspace_id(&workspace_id);
    let workspace = handle.pool_mut().get_mut(&workspace_id)?;
    let pane = resolve_c_io_pane(pane_id, workspace)?;
    Some((workspace, pane))
}

/// Take parser-generated OSC/CSI replies for one workspace pane.
///
/// The returned pointer is owned by the handle and remains valid until the
/// next Core buffer reset or handle free. Callers must copy it immediately.
///
/// # Safety
/// `h`, `workspace_id`, and `len_out` are valid pointers; `workspace_id` is a
/// NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_take_pane_reply(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    len_out: *mut usize,
) -> *const u8 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() || len_out.is_null() {
            return std::ptr::null();
        }
        let handle = &mut *h;
        let bytes = {
            let Some((workspace, pane)) = workspace_pane_mut(handle, workspace_id, pane_id) else {
                return std::ptr::null();
            };
            workspace.take_reply(pane)
        };
        let (ptr, len) = handle.push_data(&bytes);
        *len_out = len;
        ptr
    }))
    .unwrap_or(std::ptr::null())
}

/// Read a pane's scrollback window as ANSI bytes.
///
/// # Safety
/// `h` and `buf` are valid; `buf` has at least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_scroll_ansi(
    h: *mut MuxtermHandle,
    pane_id: u32,
    offset: u32,
    rows: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || buf.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        let bytes = ws.pane_scroll_ansi(pane, offset, rows);
        let n = bytes.len().min(buf_len);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        n as i32
    }))
    .unwrap_or(-1)
}

/// Read the current visible VT grid as ANSI bytes for initial seeding.
///
/// # Safety
/// `h` and `buf` are valid; `buf` has at least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_visible_ansi(
    h: *mut MuxtermHandle,
    pane_id: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || buf.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        let bytes = ws.pane_visible_ansi(pane);
        let n = bytes.len().min(buf_len);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        n as i32
    }))
    .unwrap_or(-1)
}

/// Read a one-time Surface seed for a pane.
///
/// A null or zero-length buffer queries the required length. The returned seed
/// is for creating a new Surface and is not a live output replay mechanism.
///
/// # Safety
/// `h` is valid; a non-null `buf` has at least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_surface_seed_ansi(
    h: *mut MuxtermHandle,
    pane_id: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        let bytes = ws.pane_surface_seed_ansi(pane);
        if !buf.is_null() && buf_len > 0 {
            let n = bytes.len().min(buf_len);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        }
        i32::try_from(bytes.len()).unwrap_or(-1)
    }))
    .unwrap_or(-1)
}

/// Return a pane's viewport scroll offset (0 is the latest screen).
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_viewport(h: *mut MuxtermHandle, pane_id: u32) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        ws.pane_viewport(pane) as i32
    }))
    .unwrap_or(-1)
}

/// Set a pane's viewport scroll offset.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_set_pane_viewport(
    h: *mut MuxtermHandle,
    pane_id: u32,
    offset: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        ws.set_pane_viewport(pane, offset);
        0
    }))
    .unwrap_or(-1)
}

/// Return the maximum scrollback offset available for a pane.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_history_max_offset(
    h: *mut MuxtermHandle,
    pane_id: u32,
    rows: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        ws.pane_history_max_offset(pane, rows.max(1)) as i32
    }))
    .unwrap_or(-1)
}

/// Return OSC 133 command marks for a pane as JSON.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_command_marks_json(
    h: *mut MuxtermHandle,
    pane_id: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return json_error("无前台工作区");
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return json_error("pane 不存在");
        };
        let marks: Vec<serde_json::Value> = ws
            .pane_command_marks(pane)
            .into_iter()
            .map(|mark| {
                serde_json::json!({
                    "seq": mark.seq,
                    "command": mark.command,
                    "exit_code": mark.exit_code,
                    "history_offset": ws.pane_viewport_offset_for_seq_checked(pane, mark.seq),
                })
            })
            .collect();
        json_string(serde_json::json!({ "ok": true, "marks": marks }))
    }))
    .unwrap_or_else(|_| json_error("pane command marks panic"))
}

/// Return the latest stable line sequence for a pane.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_latest_line_seq(h: *mut MuxtermHandle, pane_id: u32) -> i64 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        i64::try_from(ws.pane_latest_line_seq(pane)).unwrap_or(-1)
    }))
    .unwrap_or(-1)
}

/// Return the viewport offset for a stable line sequence.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_viewport_for_seq(
    h: *mut MuxtermHandle,
    pane_id: u32,
    seq: u64,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        ws.pane_viewport_offset_for_seq_checked(pane, seq)
            .and_then(|offset| i32::try_from(offset).ok())
            .unwrap_or(-1)
    }))
    .unwrap_or(-1)
}

/// Return the last `n` text lines for a pane as JSON.
///
/// # Safety
/// `h` is valid and has not been freed.
#[no_mangle]
pub unsafe extern "C" fn muxterm_pane_last_n_lines(
    h: *mut MuxtermHandle,
    pane_id: u32,
    n: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &*h;
        let Some(ws) = handle.active_workspace() else {
            return json_error("无前台工作区");
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return json_error("pane 不存在");
        };
        let lines: Vec<String> = ws.pane_last_n_lines(pane, n.max(1) as usize);
        json_string(serde_json::json!({ "ok": true, "lines": lines }))
    }))
    .unwrap_or_else(|_| json_error("pane last lines panic"))
}

/// Read a pane's scrollback window from a specific workspace without
/// changing pool activation.
///
/// # Safety
/// `h`, `workspace_id`, and `buf` are valid; `workspace_id` is
/// NUL-terminated and `buf` has at least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_scroll_ansi(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    offset: u32,
    rows: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() || buf.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        let bytes = ws.pane_scroll_ansi(pane, offset, rows);
        let n = bytes.len().min(buf_len);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        n as i32
    }))
    .unwrap_or(-1)
}

/// Read the current visible VT grid from a specific workspace.
///
/// # Safety
/// `h`, `workspace_id`, and `buf` are valid; `workspace_id` is
/// NUL-terminated and `buf` has at least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_visible_ansi(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() || buf.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        let bytes = ws.pane_visible_ansi(pane);
        let n = bytes.len().min(buf_len);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        n as i32
    }))
    .unwrap_or(-1)
}

/// Read a one-time Surface seed from a specific workspace.
///
/// A null or zero-length buffer queries the required length.
///
/// # Safety
/// `h` is valid; `workspace_id` is NUL-terminated; a non-null `buf` has at
/// least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_surface_seed_ansi(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        let bytes = ws.pane_surface_seed_ansi(pane);
        if !buf.is_null() && buf_len > 0 {
            let n = bytes.len().min(buf_len);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        }
        i32::try_from(bytes.len()).unwrap_or(-1)
    }))
    .unwrap_or(-1)
}

/// Return a pane's viewport scroll offset in a specific workspace.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_viewport(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        ws.pane_viewport(pane) as i32
    }))
    .unwrap_or(-1)
}

/// Set a pane's viewport scroll offset in a specific workspace.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_set_pane_viewport(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    offset: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some((ws, pane)) = workspace_pane_mut(handle, workspace_id, pane_id) else {
            return -1;
        };
        ws.set_pane_viewport(pane, offset);
        0
    }))
    .unwrap_or(-1)
}

/// Return the maximum scrollback offset for a pane in a specific workspace.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_history_max_offset(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    rows: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        ws.pane_history_max_offset(pane, rows.max(1)) as i32
    }))
    .unwrap_or(-1)
}

/// Return OSC 133 command marks for a pane in a specific workspace as JSON.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_command_marks_json(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return json_error("handle 或 workspace_id 为空");
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return json_error("workspace 或 pane 不存在");
        };
        let marks: Vec<serde_json::Value> = ws
            .pane_command_marks(pane)
            .into_iter()
            .map(|mark| {
                serde_json::json!({
                    "seq": mark.seq,
                    "command": mark.command,
                    "exit_code": mark.exit_code,
                    "history_offset": ws.pane_viewport_offset_for_seq_checked(pane, mark.seq),
                })
            })
            .collect();
        json_string(serde_json::json!({ "ok": true, "marks": marks }))
    }))
    .unwrap_or_else(|_| json_error("workspace pane command marks panic"))
}

/// Return the latest stable line sequence for a pane in a specific workspace.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_latest_line_seq(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
) -> i64 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        i64::try_from(ws.pane_latest_line_seq(pane)).unwrap_or(-1)
    }))
    .unwrap_or(-1)
}

/// Return the viewport offset for a stable line sequence in a specific
/// workspace.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_viewport_for_seq(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    seq: u64,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return -1;
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return -1;
        };
        ws.pane_viewport_offset_for_seq_checked(pane, seq)
            .and_then(|offset| i32::try_from(offset).ok())
            .unwrap_or(-1)
    }))
    .unwrap_or(-1)
}

/// Return the last `n` text lines for a pane in a specific workspace as JSON.
///
/// # Safety
/// `h` and `workspace_id` are valid; `workspace_id` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_pane_last_n_lines(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    pane_id: u32,
    n: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || workspace_id.is_null() {
            return json_error("handle 或 workspace_id 为空");
        }
        let handle = &*h;
        let Some((ws, pane)) = workspace_pane(handle, workspace_id, pane_id) else {
            return json_error("workspace 或 pane 不存在");
        };
        let lines = ws.pane_last_n_lines(pane, n.max(1) as usize);
        json_string(serde_json::json!({ "ok": true, "lines": lines }))
    }))
    .unwrap_or_else(|_| json_error("workspace pane last lines panic"))
}
