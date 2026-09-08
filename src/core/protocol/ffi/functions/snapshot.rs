//! Pane snapshot, viewport, and history query C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::super::api::{json_error, json_string, resolve_c_io_pane, MuxtermHandle};

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
