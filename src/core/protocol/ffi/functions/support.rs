//! Shared implementation details for the domain-oriented C ABI functions.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use crate::core::types::PaneId;
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::workspace::Workspace;

/// The C handle is one composed product session, not a runtime instance.
pub type MuxtermHandle = crate::core::muxterm::Muxterm;

pub(crate) fn cstr_opt(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

pub(crate) fn json_string(value: serde_json::Value) -> *mut c_char {
    let text = value.to_string();
    CString::new(text)
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}

pub(crate) fn json_error(error: impl std::fmt::Display) -> *mut c_char {
    json_string(serde_json::json!({
        "ok": false,
        "error": error.to_string(),
    }))
}

pub(crate) fn discovery_timeout(timeout_ms: u32) -> std::time::Duration {
    std::time::Duration::from_millis(u64::from(timeout_ms.clamp(100, 60_000)))
}

/// C ABI 中 `0` 既是历史上的 active-pane 哨兵，也可能是真实的 tmux pane id。
/// 只有当前状态不存在 PaneId(0) 时才使用旧哨兵语义。
pub(crate) fn resolve_c_io_pane(raw: u32, ws: &Workspace) -> Option<PaneId> {
    if raw == 0 && ws.state().pane(&PaneId(0)).is_none() {
        ws.state().active_pane().map(|p| p.id)
    } else {
        Some(PaneId(raw))
    }
}

/// Decode the stable five-component product WorkspaceId used at the C ABI.
pub(crate) fn parse_workspace_id(id: &str) -> WorkspaceId {
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
