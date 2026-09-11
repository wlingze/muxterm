//! Activity lane C ABI functions.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::support::{json_error, json_string, MuxtermHandle};

/// Return the current revisioned Activity records owned by Core.
///
/// The workspace event poll drives normalization first; this endpoint is a
/// snapshot query and does not mutate the Activity store.
///
/// # Safety
/// `h` is either null or a live Muxterm handle.
#[no_mangle]
pub unsafe extern "C" fn muxterm_activity_snapshot_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &*h;
        let records = handle
            .activity
            .records
            .records()
            .cloned()
            .collect::<Vec<_>>();
        json_string(serde_json::json!({
            "ok": true,
            "records": records,
        }))
    }))
    .unwrap_or_else(|_| json_error("activity snapshot panic"))
}

/// Drain Activity lane Upsert/Remove events since the previous call.
///
/// The returned JSON owns serialized record data.  The Core-side queue is
/// drained only after serialization succeeds inside the panic boundary.
///
/// # Safety
/// `h` is either null or a live Muxterm handle.
#[no_mangle]
pub unsafe extern "C" fn muxterm_activity_take_events_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let events = handle
            .take_activity_events()
            .into_iter()
            .map(|(workspace_id, event)| {
                serde_json::json!({
                    "workspace_id": workspace_id.to_string(),
                    "event": event,
                })
            })
            .collect::<Vec<_>>();
        json_string(serde_json::json!({
            "ok": true,
            "events": events,
        }))
    }))
    .unwrap_or_else(|_| json_error("activity event poll panic"))
}
