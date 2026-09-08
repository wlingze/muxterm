//! Workspace search C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::super::api::{cstr_opt, json_error, json_string, MuxtermHandle};

/// Search pane text across all live workspaces.
///
/// # Safety
/// `h` is valid and has not been freed; `query` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_search_all(
    h: *mut MuxtermHandle,
    query: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let query = cstr_opt(query).unwrap_or_default();
        let handle = &*h;
        let hits: Vec<serde_json::Value> = handle
            .pool()
            .search_all(&query)
            .into_iter()
            .map(|hit| {
                serde_json::json!({
                    "workspace_id": hit.workspace_id,
                    "tab_id": hit.tab_id.0,
                    "pane_id": hit.pane_id.0,
                    "seq": hit.seq,
                    "line": hit.line,
                })
            })
            .collect();
        json_string(serde_json::json!({ "ok": true, "hits": hits }))
    }))
    .unwrap_or_else(|_| json_error("search panic"))
}
