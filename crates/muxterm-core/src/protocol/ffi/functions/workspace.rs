//! Workspace pool and legacy workspace-open C ABI functions.

use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use crate::muxterm::Muxterm;
use crate::protocol::ffi::api::{
    configured_scrollback_lines, cstr_opt, discovery_timeout, json_error, json_string,
    resolve_c_io_pane, MuxtermHandle,
};
use crate::protocol::layout::{LayoutNode, SplitDir};
use crate::types::TabId;
use crate::workspace::spec::WorkspaceSpec;

use super::super::types::{CLayoutNode, CPane, CTab, LAYOUT_LEAF, LAYOUT_SPLIT_H, LAYOUT_SPLIT_V};
use super::catalog::resolved_target_json;
use super::support::parse_workspace_id;

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
            "local" => {
                crate::discovery::create_local_tmux_session(socket.as_deref(), &session, &directory)
            }
            "ssh" => {
                let Some(alias) = target.as_deref().filter(|value| !value.trim().is_empty()) else {
                    return json_error("SSH session creation requires a host alias");
                };
                crate::discovery::create_ssh_tmux_session(
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
        let result = {
            let (rt, catalog, connections, pool) = (
                &handle.rt,
                &handle.catalog,
                &mut handle.connections,
                &mut handle.pool,
            );
            rt.block_on(Muxterm::open_spec_parts(catalog, connections, pool, &spec))
        };
        match result {
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

/// List tabs in the active workspace.
///
/// # Safety
/// `out` points to at least `max_count` elements; names remain valid until
/// the next tabs query or handle free.
#[no_mangle]
pub unsafe extern "C" fn muxterm_get_tabs(
    h: *mut MuxtermHandle,
    out: *mut CTab,
    max_count: i32,
) -> i32 {
    if h.is_null() || out.is_null() || max_count <= 0 {
        return -1;
    }
    let handle = &mut *h;
    handle.tab_names.clear();
    let Some(ws) = handle.active_workspace() else {
        return 0;
    };
    let tabs: Vec<(u32, String, bool)> = ws
        .state()
        .tabs()
        .iter()
        .map(|t| (t.id.0, t.name.clone(), t.active))
        .collect();
    let n = tabs.len().min(max_count as usize);
    let slice = std::slice::from_raw_parts_mut(out, n);
    for (i, (id, name, active)) in tabs.iter().take(n).enumerate() {
        let name_ptr = match CString::new(name.as_str()) {
            Ok(cs) => {
                handle.tab_names.push(cs);
                handle.tab_names.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        };
        slice[i] = CTab {
            id: *id,
            name: name_ptr,
            is_active: u8::from(*active),
        };
    }
    n as i32
}

/// List tabs in a specific workspace without changing pool activation.
///
/// # Safety
/// `h`, `workspace_id`, and `out` are valid pointers; `workspace_id` is a
/// NUL-terminated UTF-8 string and `out` points to `max_count` entries.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_get_tabs(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    out: *mut CTab,
    max_count: i32,
) -> i32 {
    if h.is_null() || workspace_id.is_null() || out.is_null() || max_count <= 0 {
        return -1;
    }
    let Some(workspace_id) = cstr_opt(workspace_id) else {
        return -1;
    };
    let workspace_id = parse_workspace_id(&workspace_id);
    let handle = &mut *h;
    let tabs = {
        let Some(ws) = handle.pool().get(&workspace_id) else {
            return -1;
        };
        ws.state()
            .tabs()
            .iter()
            .map(|t| (t.id.0, t.name.clone(), t.active))
            .collect::<Vec<_>>()
    };
    handle.tab_names.clear();
    let n = tabs.len().min(max_count as usize);
    let slice = std::slice::from_raw_parts_mut(out, n);
    for (i, (id, name, active)) in tabs.iter().take(n).enumerate() {
        let name_ptr = match CString::new(name.as_str()) {
            Ok(cs) => {
                handle.tab_names.push(cs);
                handle.tab_names.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        };
        slice[i] = CTab {
            id: *id,
            name: name_ptr,
            is_active: u8::from(*active),
        };
    }
    n as i32
}

/// List panes in a tab of the active workspace.
///
/// # Safety
/// `out` points to at least `max_count` elements.
#[no_mangle]
pub unsafe extern "C" fn muxterm_get_panes(
    h: *mut MuxtermHandle,
    tab_id: u32,
    out: *mut CPane,
    max_count: i32,
) -> i32 {
    if h.is_null() || out.is_null() || max_count <= 0 {
        return -1;
    }
    let handle = &*h;
    let tid = TabId(tab_id);
    let Some(ws) = handle.active_workspace() else {
        return 0;
    };
    let panes = ws.state().panes(&tid);
    let n = panes.len().min(max_count as usize);
    let slice = std::slice::from_raw_parts_mut(out, n);
    for (i, p) in panes.iter().take(n).enumerate() {
        slice[i] = CPane {
            id: p.id.0,
            cols: p.cols,
            rows: p.rows,
            is_active: u8::from(p.active),
        };
    }
    n as i32
}

/// List panes in a tab of a specific workspace without changing activation.
///
/// # Safety
/// `h`, `workspace_id`, and `out` are valid pointers; `workspace_id` is a
/// NUL-terminated UTF-8 string and `out` points to `max_count` entries.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_get_panes(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    tab_id: u32,
    out: *mut CPane,
    max_count: i32,
) -> i32 {
    if h.is_null() || workspace_id.is_null() || out.is_null() || max_count <= 0 {
        return -1;
    }
    let Some(workspace_id) = cstr_opt(workspace_id) else {
        return -1;
    };
    let workspace_id = parse_workspace_id(&workspace_id);
    let handle = &*h;
    let Some(ws) = handle.pool().get(&workspace_id) else {
        return -1;
    };
    let panes = ws.state().panes(&TabId(tab_id));
    let n = panes.len().min(max_count as usize);
    let slice = std::slice::from_raw_parts_mut(out, n);
    for (i, p) in panes.iter().take(n).enumerate() {
        slice[i] = CPane {
            id: p.id.0,
            cols: p.cols,
            rows: p.rows,
            is_active: u8::from(p.active),
        };
    }
    n as i32
}

/// Read the most recent accumulated pane output into `buf`.
///
/// When the buffer is smaller than the output, the newest bytes are copied.
///
/// # Safety
/// `buf` points to at least `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_get_pane_output(
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
        let Some(out) = ws.state().pane_output(&pane) else {
            return 0;
        };
        let n = out.len().min(buf_len);
        let start = out.len() - n;
        std::ptr::copy_nonoverlapping(out.as_ptr().add(start), buf, n);
        n as i32
    }))
    .unwrap_or(-1)
}

/// Read pane output from a specific workspace without changing activation.
///
/// # Safety
/// `h`, `workspace_id`, and `buf` are valid pointers; `workspace_id` is a
/// NUL-terminated UTF-8 string and `buf` points to `buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_get_pane_output(
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
        let Some(workspace_id) = cstr_opt(workspace_id) else {
            return -1;
        };
        let workspace_id = parse_workspace_id(&workspace_id);
        let handle = &*h;
        let Some(ws) = handle.pool().get(&workspace_id) else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        let Some(out) = ws.state().pane_output(&pane) else {
            return 0;
        };
        let n = out.len().min(buf_len);
        let start = out.len() - n;
        std::ptr::copy_nonoverlapping(out.as_ptr().add(start), buf, n);
        n as i32
    }))
    .unwrap_or(-1)
}

/// Export a tab layout tree to a C layout node and its stable child pool.
///
/// # Safety
/// `out` is non-null; child pointers remain valid until the next layout query
/// or handle free.
#[no_mangle]
pub unsafe extern "C" fn muxterm_get_layout(
    h: *mut MuxtermHandle,
    tab_id: u32,
    out: *mut CLayoutNode,
) -> i32 {
    if h.is_null() || out.is_null() {
        return -1;
    }
    let handle = &mut *h;
    handle.layout_nodes.clear();
    let tid = TabId(tab_id);
    let Some(ws) = handle.active_workspace() else {
        return -1;
    };
    let Some(tl) = ws.state().layout(&tid) else {
        return -1;
    };
    let tree = tl.tree.clone();
    let root_idx = push_layout_node(&mut handle.layout_nodes, &tree);
    *out = handle.layout_nodes[root_idx];
    fixup_layout_pointers(&mut handle.layout_nodes);
    *out = handle.layout_nodes[root_idx];
    0
}

/// Export a tab layout from a specific workspace without changing activation.
///
/// # Safety
/// `h`, `workspace_id`, and `out` are valid pointers; `workspace_id` is a
/// NUL-terminated UTF-8 string and the layout pointers remain valid until the
/// next layout query or handle free.
#[no_mangle]
pub unsafe extern "C" fn muxterm_workspace_get_layout(
    h: *mut MuxtermHandle,
    workspace_id: *const c_char,
    tab_id: u32,
    out: *mut CLayoutNode,
) -> i32 {
    if h.is_null() || workspace_id.is_null() || out.is_null() {
        return -1;
    }
    let Some(workspace_id) = cstr_opt(workspace_id) else {
        return -1;
    };
    let workspace_id = parse_workspace_id(&workspace_id);
    let handle = &mut *h;
    let tree = {
        let Some(ws) = handle.pool().get(&workspace_id) else {
            return -1;
        };
        let Some(tl) = ws.state().layout(&TabId(tab_id)) else {
            return -1;
        };
        tl.tree.clone()
    };
    handle.layout_nodes.clear();
    let root_idx = push_layout_node(&mut handle.layout_nodes, &tree);
    *out = handle.layout_nodes[root_idx];
    fixup_layout_pointers(&mut handle.layout_nodes);
    *out = handle.layout_nodes[root_idx];
    0
}

fn push_layout_node(pool: &mut Vec<CLayoutNode>, node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Leaf(pid) => {
            let idx = pool.len();
            pool.push(CLayoutNode {
                type_: LAYOUT_LEAF,
                pane_id: pid.0,
                ratio: 0,
                first: ptr::null(),
                second: ptr::null(),
            });
            idx
        }
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            let type_ = match dir {
                SplitDir::Horizontal => LAYOUT_SPLIT_H,
                SplitDir::Vertical => LAYOUT_SPLIT_V,
            };
            let idx = pool.len();
            pool.push(CLayoutNode {
                type_,
                pane_id: 0,
                ratio: u32::from(*ratio),
                first: ptr::null(),
                second: ptr::null(),
            });
            let a = push_layout_node(pool, first);
            let b = push_layout_node(pool, second);
            pool[idx].first = a as *const CLayoutNode;
            pool[idx].second = b as *const CLayoutNode;
            idx
        }
    }
}

fn fixup_layout_pointers(pool: &mut [CLayoutNode]) {
    let base = pool.as_ptr();
    let len = pool.len();
    for node in pool.iter_mut() {
        if node.type_ == LAYOUT_LEAF {
            continue;
        }
        let a = node.first as usize;
        let b = node.second as usize;
        if a < len {
            node.first = unsafe { base.add(a) };
        }
        if b < len {
            node.second = unsafe { base.add(b) };
        }
    }
}
