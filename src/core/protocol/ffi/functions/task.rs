//! Task conversion and execution C ABI functions.

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::core::config::parse_hex;
use crate::core::protocol::layout::SplitDir;
use crate::core::protocol::task::{Task, TaskOutcome};
use crate::core::types::{PaneId, TabId};
use crate::core::workspace::workspace::Workspace;

use super::super::api::{cstr_opt, json_error, json_string, resolve_c_io_pane, MuxtermHandle};
use super::super::types::{
    CTask, DIR_HORIZONTAL, DIR_VERTICAL, TAB_MOVE_BEFORE, TASK_BREAK_PANE, TASK_CLOSE_PANE,
    TASK_CLOSE_TAB, TASK_DETACH, TASK_MOVE_TAB, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE,
    TASK_REFRESH_TABS, TASK_RENAME_TAB, TASK_RENAME_WORKSPACE, TASK_REQUEST_PANE_SNAPSHOT,
    TASK_SHUTDOWN, TASK_SPLIT_PANE, TASK_SWITCH_PANE, TASK_SWITCH_TAB, TASK_TOGGLE_PANE_FULLSCREEN,
};

pub(crate) fn task_result_code(result: anyhow::Result<TaskOutcome>) -> i32 {
    match result {
        Ok(TaskOutcome::Done) => 0,
        // Accepted means the asynchronous mutation was queued; the legacy ABI
        // treats that request as successful.
        Ok(TaskOutcome::Accepted { .. }) => 0,
        Ok(TaskOutcome::Rejected { .. }) | Err(_) => -1,
    }
}

pub(crate) fn ctask_to_task(task: &CTask, ws: &Workspace) -> Option<Task> {
    let name = cstr_opt(task.name);
    match task.type_ {
        TASK_SPLIT_PANE => {
            let dir = if task.dir == DIR_VERTICAL {
                SplitDir::Vertical
            } else {
                SplitDir::Horizontal
            };
            let target = Some(resolve_c_task_pane(task.target_pane, ws));
            Some(Task::SplitPane {
                target,
                dir,
                command: None,
                workdir: None,
            })
        }
        TASK_NEW_TAB => Some(Task::NewTab {
            name,
            command: None,
            workdir: None,
        }),
        TASK_SWITCH_TAB => Some(Task::SwitchTab {
            target: TabId(task.target_tab),
        }),
        TASK_CLOSE_PANE => {
            let pane = resolve_c_task_pane(task.target_pane, ws);
            Some(Task::ClosePane { target: pane })
        }
        TASK_CLOSE_TAB => Some(Task::CloseTab {
            target: TabId(task.target_tab),
        }),
        TASK_NEXT_PANE => Some(Task::NextPane),
        TASK_PREV_PANE => Some(Task::PrevPane),
        TASK_SWITCH_PANE => {
            let pane = resolve_c_task_pane(task.target_pane, ws);
            Some(Task::SwitchPane { target: pane })
        }
        TASK_SHUTDOWN => Some(Task::Shutdown),
        TASK_DETACH => Some(Task::Detach),
        TASK_TOGGLE_PANE_FULLSCREEN => {
            let pane = resolve_c_task_pane(task.target_pane, ws);
            Some(Task::TogglePaneFullscreen { target: pane })
        }
        TASK_MOVE_TAB => Some(Task::MoveTab {
            from: TabId(task.target_tab),
            target: TabId(task.target_pane),
            before: task.dir == TAB_MOVE_BEFORE,
        }),
        TASK_BREAK_PANE => {
            let pane = resolve_c_task_pane(task.target_pane, ws);
            Some(Task::BreakPane { target: pane })
        }
        TASK_REFRESH_TABS => Some(Task::RefreshTabs),
        TASK_RENAME_TAB => name.map(|name| Task::RenameTab {
            target: TabId(task.target_tab),
            name,
        }),
        TASK_RENAME_WORKSPACE => name.map(|name| Task::RenameWorkspace { name }),
        TASK_REQUEST_PANE_SNAPSHOT => Some(Task::RequestPaneSnapshot {
            target: resolve_c_task_pane(task.target_pane, ws),
        }),
        _ => None,
    }
}

/// Resolve a C task pane id while preserving the legacy zero sentinel.
fn resolve_c_task_pane(raw: u32, ws: &Workspace) -> PaneId {
    if raw == 0 && ws.state().pane(&PaneId(0)).is_none() {
        ws.state().active_pane().map(|p| p.id).unwrap_or(PaneId(0))
    } else {
        PaneId(raw)
    }
}

/// Execute a task. Returns 0 on success and -1 on error.
///
/// # Safety
/// `h` and `task` are valid pointers.
#[no_mangle]
pub unsafe extern "C" fn muxterm_execute(h: *mut MuxtermHandle, task: *const CTask) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || task.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let ctask = &*task;
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        let Some(rust_task) = ctask_to_task(ctask, ws) else {
            return -1;
        };
        tracing::debug!(target: "muxterm::ffi", task = ?rust_task, "execute task");
        task_result_code(ws.execute(rust_task))
    }))
    .unwrap_or(-1)
}

/// Parse key descriptions used by `muxterm_execute_json`.
fn key_event_from_json(text: &str) -> Vec<crate::core::protocol::terminal::input::KeyEvent> {
    use crate::core::protocol::terminal::input::{ArrowDir, KeyEvent};
    match text {
        "enter" => vec![KeyEvent::Enter],
        "tab" => vec![KeyEvent::Tab],
        "backspace" => vec![KeyEvent::Backspace],
        "escape" => vec![KeyEvent::Escape],
        "up" => vec![KeyEvent::Arrow(ArrowDir::Up)],
        "down" => vec![KeyEvent::Arrow(ArrowDir::Down)],
        "left" => vec![KeyEvent::Arrow(ArrowDir::Left)],
        "right" => vec![KeyEvent::Arrow(ArrowDir::Right)],
        _ if text.len() == 2 && text.starts_with('c') && text.as_bytes()[1] == b'-' => text
            .chars()
            .nth(1)
            .map(|_| KeyEvent::Ctrl(text[2..].chars().next().unwrap_or('c')))
            .into_iter()
            .collect(),
        _ if text.len() == 2 && text.starts_with('m') && text.as_bytes()[1] == b'-' => text
            .chars()
            .nth(1)
            .map(|_| KeyEvent::Alt(text[2..].chars().next().unwrap_or('m')))
            .into_iter()
            .collect(),
        _ if text.len() >= 2 && text.starts_with('f') && text[1..].parse::<u8>().is_ok() => {
            vec![KeyEvent::Function(text[1..].parse().unwrap_or(1))]
        }
        _ => text.chars().map(KeyEvent::Char).collect(),
    }
}

fn task_from_json(ws: &Workspace, value: &serde_json::Value) -> Option<Task> {
    let obj = value.as_object()?;
    let kind = obj.get("task")?.as_str()?;
    let dir = |v: &serde_json::Value| match v.as_str() {
        Some("h") | Some("horizontal") | Some("left") | Some("right") => Some(SplitDir::Horizontal),
        Some("v") | Some("vertical") | Some("up") | Some("down") => Some(SplitDir::Vertical),
        _ => None,
    };
    let opt_string =
        |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_str).map(String::from);
    let opt_strings = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(String::from)
                .collect::<Vec<_>>()
        })
    };
    let target_pane = |obj: &serde_json::Map<String, serde_json::Value>| {
        obj.get("target")
            .and_then(serde_json::Value::as_u64)
            .map(|n| resolve_c_task_pane(n as u32, ws))
    };
    match kind {
        "new_tab" => Some(Task::NewTab {
            name: opt_string(obj.get("name")),
            command: opt_strings(obj.get("command")),
            workdir: opt_string(obj.get("workdir")),
        }),
        "split_pane" => Some(Task::SplitPane {
            target: target_pane(obj),
            dir: dir(obj.get("dir").unwrap_or(&serde_json::Value::Null))?,
            command: opt_strings(obj.get("command")),
            workdir: opt_string(obj.get("workdir")),
        }),
        "close_pane" => Some(Task::ClosePane {
            target: target_pane(obj)?,
        }),
        "switch_pane" => Some(Task::SwitchPane {
            target: target_pane(obj)?,
        }),
        "switch_tab" => Some(Task::SwitchTab {
            target: TabId(obj.get("target")?.as_u64()? as u32),
        }),
        "close_tab" => Some(Task::CloseTab {
            target: TabId(obj.get("target")?.as_u64()? as u32),
        }),
        "rename_tab" => Some(Task::RenameTab {
            target: TabId(obj.get("target")?.as_u64()? as u32),
            name: opt_string(obj.get("name"))?,
        }),
        "rename_workspace" => Some(Task::RenameWorkspace {
            name: opt_string(obj.get("name"))?,
        }),
        "resize_pane" => Some(Task::ResizePane {
            target: target_pane(obj)?,
            cols: obj.get("cols")?.as_u64()? as u16,
            rows: obj.get("rows")?.as_u64()? as u16,
        }),
        "write_raw" => Some(Task::WriteRaw {
            target: target_pane(obj)?,
            data: opt_string(obj.get("data"))?.into_bytes(),
        }),
        "send_keys" => Some(Task::SendKeys {
            target: target_pane(obj)?,
            keys: obj
                .get("keys")
                .and_then(serde_json::Value::as_str)
                .map(key_event_from_json)
                .or_else(|| {
                    obj.get("keys")
                        .and_then(serde_json::Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(serde_json::Value::as_str)
                                .flat_map(key_event_from_json)
                                .collect()
                        })
                })?,
        }),
        "detach" => Some(Task::Detach),
        "shutdown" => Some(Task::Shutdown),
        _ => None,
    }
}

/// Execute a task from a JSON request and return a structured result.
///
/// # Safety
/// `h` is valid and has not been freed; `json` is NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_execute_json(
    h: *mut MuxtermHandle,
    json: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || json.is_null() {
            return json_error("handle 或 json 为空");
        }
        let Ok(json) = CStr::from_ptr(json).to_str() else {
            return json_error("json 不是合法 UTF-8");
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return json_error("json 解析失败");
        };
        let handle = &mut *h;
        let Some(ws) = handle.active_workspace_mut() else {
            return json_error("没有 active workspace");
        };
        let Some(rust_task) = task_from_json(&*ws, &value) else {
            return json_error("未知 task 或参数缺失");
        };
        match ws.execute(rust_task) {
            Ok(TaskOutcome::Done) => json_string(serde_json::json!({
                "ok": true,
                "result": "done",
            })),
            Ok(TaskOutcome::Accepted { operation_id }) => json_string(serde_json::json!({
                "ok": true,
                "result": "accepted",
                "operation_id": operation_id,
            })),
            Ok(TaskOutcome::Rejected { reason }) => json_string(serde_json::json!({
                "ok": false,
                "result": "rejected",
                "reason": reason,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("execute_json panic"))
}

/// Write raw bytes to a pane and mark the user input for attention handling.
///
/// # Safety
/// `h` is live; `data` points to at least `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_send_input(
    h: *mut MuxtermHandle,
    pane_id: u32,
    data: *const u8,
    len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || data.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let bytes = std::slice::from_raw_parts(data, len).to_vec();
        let pane = {
            let Some(ws) = handle.active_workspace() else {
                return -1;
            };
            resolve_c_io_pane(pane_id, ws)
        };
        let Some(pane) = pane else {
            return -1;
        };
        let ws_id = handle.pool().active_id().cloned();
        if let Some(ws_id) = ws_id {
            handle.attention.on_user_input(&ws_id.replica_id(), pane.0);
        }
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        task_result_code(ws.execute(Task::WriteRaw {
            target: pane,
            data: bytes,
        }))
    }))
    .unwrap_or(-1)
}

/// Write raw bytes without clearing attention state.
///
/// # Safety
/// `h` is live; `data` points to at least `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn muxterm_send_input_quiet(
    h: *mut MuxtermHandle,
    pane_id: u32,
    data: *const u8,
    len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || data.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let bytes = std::slice::from_raw_parts(data, len).to_vec();
        let pane = {
            let Some(ws) = handle.active_workspace() else {
                return -1;
            };
            resolve_c_io_pane(pane_id, ws)
        };
        let Some(pane) = pane else {
            return -1;
        };
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        task_result_code(ws.execute(Task::WriteRaw {
            target: pane,
            data: bytes,
        }))
    }))
    .unwrap_or(-1)
}

/// Report one pane's foreground/background colours.
///
/// # Safety
/// `h` is live; colour strings are NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_report_pane_colours(
    h: *mut MuxtermHandle,
    pane_id: u32,
    fg_hex: *const c_char,
    bg_hex: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let (Some(fg_hex), Some(bg_hex)) = (cstr_opt(fg_hex), cstr_opt(bg_hex)) else {
            return -1;
        };
        let (Ok(fg), Ok(bg)) = (parse_hex(&fg_hex), parse_hex(&bg_hex)) else {
            return -1;
        };
        let handle = &mut *h;
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        task_result_code(ws.execute(Task::ReportPaneColours {
            target: pane,
            fg,
            bg,
        }))
    }))
    .unwrap_or(-1)
}

/// Report foreground/background colours for every pane.
///
/// # Safety
/// `h` is live; colour strings are NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn muxterm_report_all_pane_colours(
    h: *mut MuxtermHandle,
    fg_hex: *const c_char,
    bg_hex: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let (Some(fg_hex), Some(bg_hex)) = (cstr_opt(fg_hex), cstr_opt(bg_hex)) else {
            return -1;
        };
        let (Ok(fg), Ok(bg)) = (parse_hex(&fg_hex), parse_hex(&bg_hex)) else {
            return -1;
        };
        let handle = &mut *h;
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        let panes: Vec<PaneId> = ws
            .state()
            .tabs()
            .iter()
            .flat_map(|t| ws.state().panes(&t.id))
            .map(|p| p.id)
            .collect();
        let mut dispatched = 0;
        for pane in panes {
            if let Ok(TaskOutcome::Done) = ws.execute(Task::ReportPaneColours {
                target: pane,
                fg,
                bg,
            }) {
                dispatched += 1;
            }
        }
        if dispatched > 0 {
            0
        } else {
            -1
        }
    }))
    .unwrap_or(-1)
}

/// Resize a pane's pty grid.
///
/// # Safety
/// `h` is live.
#[no_mangle]
pub unsafe extern "C" fn muxterm_resize_pane(
    h: *mut MuxtermHandle,
    pane_id: u32,
    cols: u16,
    rows: u16,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || cols == 0 || rows == 0 {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        task_result_code(ws.execute(Task::ResizePane {
            target: pane,
            cols,
            rows,
        }))
    }))
    .unwrap_or(-1)
}

/// Resize the active tmux control client.
///
/// # Safety
/// `h` is live.
#[no_mangle]
pub unsafe extern "C" fn muxterm_resize_client(h: *mut MuxtermHandle, cols: u16, rows: u16) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || cols == 0 || rows == 0 {
            return -1;
        }
        let handle = &mut *h;
        match handle.active_workspace_mut() {
            Some(ws) => task_result_code(ws.execute(Task::ResizeClient { cols, rows })),
            None => -1,
        }
    }))
    .unwrap_or(-1)
}

/// Resize one axis of a pane split.
///
/// # Safety
/// `h` is live.
#[no_mangle]
pub unsafe extern "C" fn muxterm_resize_pane_axis(
    h: *mut MuxtermHandle,
    pane_id: u32,
    axis: u32,
    size: u16,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || size == 0 || (axis != DIR_HORIZONTAL && axis != DIR_VERTICAL) {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws) = handle.active_workspace_mut() else {
            return -1;
        };
        let Some(pane) = resolve_c_io_pane(pane_id, ws) else {
            return -1;
        };
        let dir = if axis == DIR_VERTICAL {
            SplitDir::Vertical
        } else {
            SplitDir::Horizontal
        };
        task_result_code(ws.execute(Task::ResizePaneAxis {
            target: pane,
            dir,
            size,
        }))
    }))
    .unwrap_or(-1)
}
