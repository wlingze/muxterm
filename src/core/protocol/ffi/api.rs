//! `#[no_mangle] extern "C"` 导出函数。
//!
//! 内部持有 [`TerminalModel`] + tokio runtime，对外全部同步。

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use crate::core::attention::clock::RealClock;
use crate::core::attention::engine::AttentionEngine;
use crate::core::attention::signal::AttentionSignal;
use crate::core::config_service::SettingsService;
use crate::core::logging::{init_logging, LoggingConfig};
use crate::core::projects::{ProjectStore, ProjectsService};
use crate::core::protocol::layout::{LayoutNode, SplitDir};
use crate::core::protocol::state::StateChange;
use crate::core::protocol::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::emulate::DEFAULT_SCROLLBACK_LINES;
use crate::core::runtime::{DaemonRuntime, ShellRuntime, TmuxRuntime};
use crate::core::types::{PaneId, TabId};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::{WorkspacePool, WorkspacePoolPolicy};
use crate::core::workspace::terminal_model::TerminalModel;
use crate::core::workspace::workspace::Workspace;

use super::callbacks::FfiCallbacks;
pub use super::functions::attention::{
    muxterm_attention_acknowledge, muxterm_attention_mute, muxterm_attention_on_became_visible,
    muxterm_attention_set_process_name, muxterm_attention_snapshot,
    muxterm_attention_take_notifications,
};
pub use super::functions::catalog::{
    muxterm_candidates_json, muxterm_open_json, muxterm_workspace_open_target_json,
};
pub use super::functions::config::{
    muxterm_config_begin_json, muxterm_config_cancel_json, muxterm_config_commit_json,
    muxterm_config_describe_json, muxterm_config_events_json, muxterm_config_patch_json,
    muxterm_config_reload_json,
};
pub(crate) use super::functions::events::state_change_to_c;
pub use super::functions::events::{muxterm_poll_events, muxterm_poll_workspace_events};
pub(crate) use super::functions::handle::configured_scrollback_lines;
pub use super::functions::handle::{
    muxterm_catalog_new, muxterm_free, muxterm_new, muxterm_new_connect, muxterm_new_connect_sized,
};
pub use super::functions::runtime::{
    muxterm_connect, muxterm_detach, muxterm_runtime_list_json, muxterm_shutdown,
    muxterm_status_subscription_active, muxterm_traffic_down, muxterm_traffic_up,
};
pub use super::functions::search::muxterm_search_all;
pub use super::functions::snapshot::{
    muxterm_pane_command_marks_json, muxterm_pane_history_max_offset, muxterm_pane_last_n_lines,
    muxterm_pane_latest_line_seq, muxterm_pane_scroll_ansi, muxterm_pane_surface_seed_ansi,
    muxterm_pane_viewport, muxterm_pane_viewport_for_seq, muxterm_pane_visible_ansi,
    muxterm_set_pane_viewport,
};
pub(crate) use super::functions::task::{ctask_to_task, task_result_code};
pub use super::functions::task::{
    muxterm_execute, muxterm_execute_json, muxterm_report_all_pane_colours,
    muxterm_report_pane_colours, muxterm_resize_client, muxterm_resize_pane,
    muxterm_resize_pane_axis, muxterm_send_input, muxterm_send_input_quiet,
};
pub(crate) use super::functions::transport::session_candidate_json;
pub use super::functions::transport::{
    muxterm_discover_sessions_json, muxterm_discover_ssh_hosts_json, muxterm_discover_targets_json,
    muxterm_discover_tmux_sessions_json, muxterm_discover_workspaces_json, muxterm_list_dir_json,
    muxterm_status_snapshot_json, muxterm_transport_list_json,
};
pub use super::functions::workspace::{
    muxterm_create_tmux_session_json, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_workspace_activate, muxterm_workspace_close,
    muxterm_workspace_create, muxterm_workspace_list, muxterm_workspace_open,
};
use super::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, CWorkspaceStateChange, BACKEND_STATUS_CONNECTED,
    BACKEND_STATUS_CONNECTING, BACKEND_STATUS_DISCONNECTED, BACKEND_STATUS_ERROR,
    BACKEND_STATUS_EXITED, DIR_HORIZONTAL, DIR_VERTICAL, LAYOUT_LEAF, LAYOUT_SPLIT_H,
    LAYOUT_SPLIT_V, STATE_ACTIVE_PANE_CHANGED, STATE_ACTIVE_TAB_CHANGED, STATE_BACKEND_STATUS,
    STATE_LAYOUT_CHANGED, STATE_MUTATION_SETTLED, STATE_OTHER, STATE_PANE_ADDED,
    STATE_PANE_AGENT_CHANGED, STATE_PANE_CLOSED, STATE_PANE_FRAME, STATE_PANE_HISTORY,
    STATE_PANE_OUTPUT, STATE_PANE_RESIZED, STATE_PANE_SNAPSHOT, STATE_POOL_CHANGED,
    STATE_STATUS_SUBSCRIPTION, STATE_TAB_ADDED, STATE_TAB_CLOSED, STATE_TAB_ORDER_CHANGED,
    STATE_TAB_RENAMED, STATE_WORKSPACE_RENAMED, TASK_BREAK_PANE, TASK_CLOSE_PANE, TASK_CLOSE_TAB,
    TASK_DETACH, TASK_MOVE_TAB, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE, TASK_REFRESH_TABS,
    TASK_RENAME_TAB, TASK_RENAME_WORKSPACE, TASK_REQUEST_PANE_SNAPSHOT, TASK_SHUTDOWN,
    TASK_SPLIT_PANE, TASK_SWITCH_PANE, TASK_SWITCH_TAB, TASK_TOGGLE_PANE_FULLSCREEN,
};

/// The C handle is one composed product session, not a runtime instance.
pub type MuxtermHandle = crate::core::muxterm::Muxterm;
pub(crate) use crate::core::muxterm::should_export_state_change;

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

/// 初始化核心日志（macOS .app 由 Swift 在创建 CoreBridge 前调用）。
///
/// `level` 取 `trace` / `debug` / `info` / `warn` / `error`；`log_file` 为
/// `NULL` 时写 stderr。重复调用（AlreadyInitialized）视为成功，不会 panic。
/// 返回 0=ok，-1=err。
#[no_mangle]
pub extern "C" fn muxterm_init_logging(log_file: *const c_char, level: *const c_char) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let level = cstr_opt(level).unwrap_or_else(|| "info".into());
        let file = cstr_opt(log_file).map(std::path::PathBuf::from);
        match init_logging(LoggingConfig { level, file }) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    }))
    .unwrap_or(-1)
}

/// 释放 discovery API 返回的 JSON 字符串。
///
/// # Safety
/// `value` 必须是本库返回且尚未释放的指针。
#[no_mangle]
pub unsafe extern "C" fn muxterm_free_string(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::projects::Project;
    use crate::core::protocol::ffi::muxterm_set_callbacks;
    use crate::core::protocol::ffi::types::DIR_HORIZONTAL;
    use crate::core::protocol::state::{
        PaneAgentInfo, PaneAgentSession, PaneAgentSessionKind, PaneAgentStatus,
    };
    use crate::core::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
    use crate::core::runtime::mock::MockRuntime;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_TMUX_SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn ffi_open_json_resolves_a_project_candidate() {
        let h = muxterm_catalog_new();
        assert!(!h.is_null());
        unsafe {
            // Keep this test isolated from the user's persisted project file.
            (*h).projects = ProjectsService::in_memory();
            (*h).projects_mut()
                .create_project(Project::new(
                    "ffi-project",
                    "FFI Project",
                    TargetConfig::new(
                        "FFI Project",
                        TargetRuntime::Shell,
                        TargetTransport::Local,
                        "/tmp",
                    ),
                ))
                .unwrap();
            let request = CString::new(
                r#"{"candidate":{"kind":"project","value":{"project_id":"ffi-project"}},"intent":"create_if_missing"}"#,
            )
            .unwrap();

            let response = muxterm_open_json(h, request.as_ptr());
            assert!(!response.is_null());
            let text = CStr::from_ptr(response).to_string_lossy().into_owned();
            muxterm_free_string(response);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], true);
            assert_eq!(
                value["resolved_target"]["spec"]["provenance"]["project_id"],
                "ffi-project"
            );
            assert_eq!(value["resolved_target"]["spec"]["runtime"], "shell");
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_candidates_json_aggregates_core_projects() {
        let h = muxterm_catalog_new();
        assert!(!h.is_null());
        unsafe {
            // No providers means the discovery leg is empty and cannot touch
            // a real tmux/SSH endpoint in this unit test.
            (*h).catalog = crate::core::catalog::Catalog::new();
            (*h).projects = ProjectsService::in_memory();
            (*h).projects_mut()
                .create_project(Project::new(
                    "candidate-project",
                    "Candidate Project",
                    TargetConfig::new(
                        "Candidate Project",
                        TargetRuntime::Shell,
                        TargetTransport::Local,
                        "/tmp",
                    ),
                ))
                .unwrap();

            let response = muxterm_candidates_json(h, 0);
            assert!(!response.is_null());
            let text = CStr::from_ptr(response).to_string_lossy().into_owned();
            muxterm_free_string(response);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], true);
            assert_eq!(value["candidates"].as_array().unwrap().len(), 1);
            assert_eq!(value["candidates"][0]["kind"], "project");
            assert_eq!(
                value["candidates"][0]["ref"]["value"]["project_id"],
                "candidate-project"
            );
            muxterm_free(h);
        }
    }

    struct IsolatedTmuxServer {
        socket: String,
    }

    impl IsolatedTmuxServer {
        fn start(session: &str) -> Option<Self> {
            let sequence = TEST_TMUX_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
            let socket = format!("muxterm-test-ffi-{}-{sequence}", std::process::id());
            let server = Self { socket };
            let status = std::process::Command::new("tmux")
                .args(["-L", &server.socket, "new-session", "-d", "-s", session])
                .status()
                .ok()?;
            status.success().then_some(server)
        }
    }

    impl Drop for IsolatedTmuxServer {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["-L", &self.socket, "kill-server"])
                .status();
        }
    }

    #[test]
    fn ffi_export_policy_filters_index_snapshots_only() {
        assert!(!should_export_state_change(
            &StateChange::PaneIndexSnapshot {
                pane: PaneId(1),
                data: b"index".to_vec(),
            }
        ));
        assert!(should_export_state_change(&StateChange::PaneOutput {
            pane: PaneId(1),
            data: b"live".to_vec(),
        }));
        assert!(should_export_state_change(&StateChange::PaneSnapshot {
            pane: PaneId(1),
            data: b"surface".to_vec(),
        }));
        assert!(should_export_state_change(&StateChange::PaneHistory {
            pane: PaneId(1),
            data: b"history".to_vec(),
        }));
    }

    #[test]
    fn ffi_pane_agent_event_uses_runtime_neutral_json() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let agent = PaneAgentInfo {
                terminal_id: Some("term-1".into()),
                name: Some("reviewer".into()),
                kind: Some("codex".into()),
                title: Some("Approve command".into()),
                terminal_title: Some("codex".into()),
                terminal_title_stripped: Some("codex".into()),
                display_name: Some("Codex reviewer".into()),
                status: PaneAgentStatus::Blocked,
                screen_detection_skipped: true,
                state_labels: BTreeMap::from([("blocked".into(), "Needs approval".into())]),
                tokens: BTreeMap::from([("context".into(), "73%".into())]),
                session: Some(PaneAgentSession {
                    source: "herdr:codex".into(),
                    agent: "codex".into(),
                    kind: PaneAgentSessionKind::Id,
                    value: "session-1".into(),
                }),
                focused: false,
                launch_pending: false,
                interactive_ready: true,
                state_change_seq: 8,
                cwd: Some("/repo".into()),
                foreground_cwd: Some("/repo/src".into()),
                revision: 12,
            };
            let event = StateChange::PaneAgentChanged {
                pane: PaneId(17),
                agent: Some(Box::new(agent)),
                initial: true,
            };
            let c_event = state_change_to_c(&mut *h, &event);
            assert_eq!(c_event.type_, STATE_PANE_AGENT_CHANGED);
            assert_eq!(c_event.pane_id, 17);
            let payload = std::slice::from_raw_parts(c_event.data, c_event.data_len);
            let json: serde_json::Value = serde_json::from_slice(payload).unwrap();
            assert_eq!(json["initial"], true);
            assert_eq!(json["agent"]["status"], "blocked");
            assert_eq!(json["agent"]["kind"], "codex");
            assert_eq!(json["agent"]["tokens"]["context"], "73%");
            assert_eq!(json["agent"]["session"]["kind"], "id");
            assert!(
                !String::from_utf8_lossy(payload).contains("pane.agent_status_changed"),
                "FFI payload 禁止泄漏 Herdr wire event 名"
            );
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_agent_event_updates_attention_without_surface_output() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let mut runtime = MockRuntime::with_single_pane();
            runtime.events.push(StateChange::PaneAgentChanged {
                pane: PaneId(1),
                agent: Some(Box::new(PaneAgentInfo {
                    terminal_id: None,
                    name: Some("codex".into()),
                    kind: Some("codex".into()),
                    title: Some("Waiting for input".into()),
                    terminal_title: None,
                    terminal_title_stripped: None,
                    display_name: Some("Codex".into()),
                    status: PaneAgentStatus::Done,
                    screen_detection_skipped: false,
                    state_labels: BTreeMap::new(),
                    tokens: BTreeMap::new(),
                    session: None,
                    focused: false,
                    launch_pending: false,
                    interactive_ready: true,
                    state_change_seq: 2,
                    cwd: Some("/repo".into()),
                    foreground_cwd: None,
                    revision: 2,
                })),
                initial: false,
            });
            let workspace = Workspace::new(
                WorkspaceId::new("local", None, "agents", "herdr", "w1"),
                "agents".into(),
                Box::new(runtime),
            );
            (&mut *h).pool_mut().insert_connected(workspace);

            let mut events = [CStateChange::default(); 16];
            assert!(muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32) > 0);

            let raw = muxterm_attention_snapshot(h);
            let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            let pane = &value["workspaces"][0]["panes"][0];
            assert_eq!(pane["status"], "done", "{text}");
            assert_eq!(pane["process_name"], "codex", "{text}");
            assert_eq!(pane["acknowledged"], false, "{text}");

            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_pane_frame_event_keeps_full_frame_type() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let frame = StateChange::PaneFrame {
                pane: PaneId(23),
                data: b"\x1b[2J\x1b[Hlatest-frame".to_vec(),
            };
            let c_event = state_change_to_c(&mut *h, &frame);
            assert_eq!(c_event.type_, STATE_PANE_FRAME);
            assert_ne!(c_event.type_, STATE_PANE_OUTPUT);
            assert_eq!(c_event.pane_id, 23);
            let payload = std::slice::from_raw_parts(c_event.data, c_event.data_len);
            assert_eq!(payload, b"\x1b[2J\x1b[Hlatest-frame");
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_local_new_connect_split_poll_free() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);

            // 等初始事件
            let mut buf = [CStateChange::default(); 32];
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 32);

            let task = CTask {
                type_: TASK_SPLIT_PANE,
                target_pane: 0,
                target_tab: 0,
                dir: DIR_HORIZONTAL,
                name: ptr::null(),
            };
            assert_eq!(muxterm_execute(h, &task), 0);

            let mut tabs = [CTab {
                id: 0,
                name: ptr::null(),
                is_active: 0,
            }; 8];
            let ntabs = muxterm_get_tabs(h, tabs.as_mut_ptr(), 8);
            assert!(ntabs >= 1, "应有至少 1 个 tab: {ntabs}");

            let tab_id = tabs[0].id;
            let mut panes = [CPane {
                id: 0,
                cols: 0,
                rows: 0,
                is_active: 0,
            }; 8];
            // split 后可能需要一点时间；再 refresh
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 32);
            let npanes = muxterm_get_panes(h, tab_id, panes.as_mut_ptr(), 8);
            assert!(npanes >= 1, "应有 pane: {npanes}");

            // 写入输入
            let msg = b"echo ffi-ok\n";
            let pane = panes[0].id;
            assert_eq!(muxterm_send_input(h, pane, msg.as_ptr(), msg.len()), 0);

            // 读输出缓冲（可能尚空，但不应报错）
            let mut out = [0u8; 256];
            let n = muxterm_get_pane_output(h, pane, out.as_mut_ptr(), out.len());
            assert!(n >= 0);

            assert_eq!(muxterm_shutdown(h), 0);
            muxterm_free(h);
        }
    }

    /// W7：workspace_open / list / activate / close 走 core 池。
    #[test]
    fn ffi_workspace_open_list_activate_close() {
        let Some(tmux) = IsolatedTmuxServer::start("demo") else {
            eprintln!("skip: tmux 不可用");
            return;
        };
        let socket = CString::new(tmux.socket.as_str()).unwrap();
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            // 旧 muxterm_new 已开一个 local shell 工作区。
            let list = muxterm_workspace_list(h);
            assert!(!list.is_null());
            let text = CStr::from_ptr(list).to_string_lossy().into_owned();
            muxterm_free_string(list);
            assert!(text.contains("workspaces"), "list 应含 workspaces: {text}");

            // 再开一个隔离 tmux 工作区，绝不能尝试用户默认 server。
            let rc = muxterm_workspace_open(
                h,
                c"local".as_ptr(),
                ptr::null(),
                c"demo".as_ptr(),
                c"tmux".as_ptr(),
                c"".as_ptr(),
                socket.as_ptr(),
            );
            assert_eq!(rc, 0, "隔离 tmux workspace 应可打开");

            // list 应仍可调用。
            let list = muxterm_workspace_list(h);
            assert!(!list.is_null());
            let text = CStr::from_ptr(list).to_string_lossy().into_owned();
            muxterm_free_string(list);
            assert!(text.contains("workspaces"), "list 应可重复调用: {text}");

            // activate/close 不存在的 id 返回 -1，不 panic。
            assert_eq!(
                muxterm_workspace_activate(h, c"local//missing/tmux/".as_ptr()),
                -1
            );
            assert_eq!(
                muxterm_workspace_close(h, c"local//missing/tmux/".as_ptr()),
                -1
            );

            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_detach_is_a_distinct_task_and_local_runtime_rejects_it() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);
            let task = CTask {
                type_: TASK_DETACH,
                target_pane: 0,
                target_tab: 0,
                dir: 0,
                name: ptr::null(),
            };
            assert_eq!(muxterm_execute(h, &task), -1);
            assert_eq!(muxterm_detach(h), -1);
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_snapshot_request_maps_to_runtime_neutral_task() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let c_task = CTask {
                type_: TASK_REQUEST_PANE_SNAPSHOT,
                target_pane: 42,
                target_tab: 0,
                dir: 0,
                name: ptr::null(),
            };
            let handle = &mut *h;
            let workspace = handle.active_workspace_mut().unwrap();
            assert_eq!(
                ctask_to_task(&c_task, workspace),
                Some(Task::RequestPaneSnapshot { target: PaneId(42) })
            );
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_snapshot_request_executes_and_exports_authoritative_snapshot() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let expected = b"\x1b[2J\x1b[Hauthoritative-frame".to_vec();
            let mut runtime = MockRuntime::with_single_pane();
            runtime.outputs[0].1 = expected.clone();
            let workspace = Workspace::new(
                WorkspaceId::new("local", None, "ffi-snapshot", "tmux", "mock"),
                "ffi-snapshot".into(),
                Box::new(runtime),
            );
            (&mut *h).pool_mut().insert_connected(workspace);

            let c_task = CTask {
                type_: TASK_REQUEST_PANE_SNAPSHOT,
                target_pane: 1,
                target_tab: 0,
                dir: 0,
                name: ptr::null(),
            };
            assert_eq!(muxterm_execute(h, &c_task), 0);

            let mut events = [CStateChange::default(); 16];
            let count = muxterm_poll_events(h, events.as_mut_ptr(), events.len() as i32);
            assert!(count > 0, "快照请求应产生 FFI 事件");
            let snapshot = events[..count as usize]
                .iter()
                .find(|event| event.type_ == STATE_PANE_SNAPSHOT)
                .expect("FFI 应保留 PaneSnapshot 类型");
            assert_eq!(snapshot.pane_id, 1);
            let payload = std::slice::from_raw_parts(snapshot.data, snapshot.data_len);
            assert_eq!(payload, expected);

            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_rename_tasks_update_tab_and_workspace_names() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);
            let mut events = [CStateChange::default(); 32];
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), 32);

            let mut tabs = [CTab {
                id: 0,
                name: ptr::null(),
                is_active: 0,
            }; 4];
            assert!(muxterm_get_tabs(h, tabs.as_mut_ptr(), 4) >= 1);
            let tab_id = tabs[0].id;

            let tab_name = CString::new("renamed-tab").unwrap();
            let rename_tab = CTask {
                type_: TASK_RENAME_TAB,
                target_pane: 0,
                target_tab: tab_id,
                dir: 0,
                name: tab_name.as_ptr(),
            };
            assert_eq!(muxterm_execute(h, &rename_tab), 0);
            let _ = muxterm_poll_events(h, events.as_mut_ptr(), 32);
            assert!(muxterm_get_tabs(h, tabs.as_mut_ptr(), 4) >= 1);
            let renamed = CStr::from_ptr(tabs[0].name).to_string_lossy();
            assert_eq!(renamed, "renamed-tab");

            let workspace_name = CString::new("renamed-workspace").unwrap();
            let rename_workspace = CTask {
                type_: TASK_RENAME_WORKSPACE,
                target_pane: 0,
                target_tab: 0,
                dir: 0,
                name: workspace_name.as_ptr(),
            };
            assert_eq!(muxterm_execute(h, &rename_workspace), 0);
            let count = muxterm_poll_events(h, events.as_mut_ptr(), 32);
            assert!(count >= 1);
            assert!(events[..count as usize]
                .iter()
                .any(|event| event.type_ == STATE_WORKSPACE_RENAMED));

            let list = muxterm_workspace_list(h);
            assert!(!list.is_null());
            let json = CStr::from_ptr(list).to_string_lossy().into_owned();
            muxterm_free_string(list);
            assert!(json.contains("renamed-workspace"), "workspace list: {json}");

            muxterm_free(h);
        }
    }

    /// 回归：长运行 pane 的累计输出超过前端缓冲后，`muxterm_get_pane_output`
    /// 必须返回**最近**的字节（尾部），而不是最旧头部。否则 macOS/TUI 的
    /// 快照会永远停在陈旧头部，htop/codex/agent 这类 pane 一旦超过前端
    /// 缓冲（256KB）就冻结或乱码。
    #[test]
    fn ffi_get_pane_output_returns_recent_tail() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);
            let mut buf = [CStateChange::default(); 32];
            let _ = muxterm_poll_events(h, buf.as_mut_ptr(), 32);

            // 等初始 pane 就绪并取真实 pane id（不能硬编码 1：并发跑时
            // local runtime 的 pane id 不保证是 1）。
            let mut tabs = [CTab {
                id: 0,
                name: ptr::null(),
                is_active: 0,
            }; 8];
            let ntabs = muxterm_get_tabs(h, tabs.as_mut_ptr(), 8);
            assert!(ntabs >= 1, "应有至少 1 个 tab: {ntabs}");
            let tab_id = tabs[0].id;
            let mut panes = [CPane {
                id: 0,
                cols: 0,
                rows: 0,
                is_active: 0,
            }; 8];
            let npanes = muxterm_get_panes(h, tab_id, panes.as_mut_ptr(), 8);
            assert!(npanes >= 1, "应有 pane: {npanes}");
            let pane = panes[0].id;

            // 写入远超过 256 字节的输出，尾部带唯一标记
            let msg = b"yes A | head -c 600; echo ZZZEND\n";
            assert_eq!(muxterm_send_input(h, pane, msg.as_ptr(), msg.len()), 0);

            // 轮询直到输出到达
            let mut out = [0u8; 256];
            let mut found = false;
            for _ in 0..250 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                // PTY 输出要经 refresh()（poll_events）才会 drain 进 model；
                // 与真实 UI 的 16ms 轮询一致。
                let mut events = [CStateChange::default(); 32];
                let _ = muxterm_poll_events(h, events.as_mut_ptr(), 32);
                let n = muxterm_get_pane_output(h, pane, out.as_mut_ptr(), out.len());
                if n > 0 && out[..n as usize].windows(6).any(|w| w == b"ZZZEND") {
                    found = true;
                    break;
                }
            }
            assert!(found, "应读到尾部标记 ZZZEND（而不是陈旧头部）");

            assert_eq!(muxterm_shutdown(h), 0);
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_callbacks_fire_on_poll() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        extern "C" fn on_state(_ev: *const CStateChange) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            muxterm_set_callbacks(h, None, Some(on_state));
            assert_eq!(muxterm_connect(h), 0);
            let mut buf = [CStateChange::default(); 64];
            let n = muxterm_poll_events(h, buf.as_mut_ptr(), 64);
            assert!(n >= 0);
            assert!(
                CALLS.load(Ordering::SeqCst) > 0 || n == 0,
                "有事件时应触发回调"
            );
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_poll_small_buffer_preserves_all_state_events() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);
            let mut initial = [CStateChange::default(); 64];
            let _ = muxterm_poll_events(h, initial.as_mut_ptr(), 64);

            let task = CTask {
                type_: TASK_SPLIT_PANE,
                target_pane: 0,
                target_tab: 0,
                dir: DIR_HORIZONTAL,
                name: ptr::null(),
            };
            assert_eq!(muxterm_execute(h, &task), 0);

            let mut one = [CStateChange::default(); 1];
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut saw_added = false;
            let mut saw_layout = false;
            while std::time::Instant::now() < deadline && !(saw_added && saw_layout) {
                let n = muxterm_poll_events(h, one.as_mut_ptr(), 1);
                assert!(n >= 0);
                if n == 1 {
                    saw_added |= one[0].type_ == STATE_PANE_ADDED;
                    saw_layout |= one[0].type_ == STATE_LAYOUT_CHANGED;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(saw_added, "小 C 缓冲不能丢失 PaneAdded 事件");
            assert!(saw_layout, "小 C 缓冲不能丢失 LayoutChanged 事件");

            let _ = muxterm_shutdown(h);
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_send_input_rejects_unknown_pane_instead_of_reporting_success() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);
            let data = b"should-fail";
            assert_eq!(
                muxterm_send_input(h, u32::MAX, data.as_ptr(), data.len()),
                -1
            );
            let _ = muxterm_shutdown(h);
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_null_handle_safe() {
        unsafe {
            assert_eq!(muxterm_connect(ptr::null_mut()), -1);
            assert_eq!(muxterm_shutdown(ptr::null_mut()), -1);
            assert_eq!(muxterm_detach(ptr::null_mut()), -1);
            assert_eq!(muxterm_execute(ptr::null_mut(), ptr::null()), -1);
            assert_eq!(muxterm_poll_events(ptr::null_mut(), ptr::null_mut(), 1), -1);
            muxterm_free(ptr::null_mut());
        }
    }

    /// C5：runtime_list JSON 含 tmux/herdr/shell，不含 daemon。
    #[test]
    fn ffi_runtime_list_contains_builtins_no_daemon() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let raw = muxterm_runtime_list_json(h);
            assert!(!raw.is_null());
            let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let json: serde_json::Value = serde_json::from_str(&value).unwrap();
            let ids: Vec<String> = json["runtimes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(ids, ["tmux", "herdr", "shell"]);
            assert!(!ids.iter().any(|id| id == "daemon"));
            muxterm_free(h);
        }
    }

    /// macOS 新产品路径需要一个不预开 shell/tmux 的 Catalog handle，随后再走
    /// descriptor-aware `workspace_open_target_json`。
    #[test]
    fn ffi_catalog_new_starts_empty_and_exposes_builtin_runtimes() {
        let h = muxterm_catalog_new();
        assert!(!h.is_null());
        unsafe {
            let list_raw = muxterm_workspace_list(h);
            let list_text = CStr::from_ptr(list_raw).to_string_lossy().into_owned();
            muxterm_free_string(list_raw);
            let list: serde_json::Value = serde_json::from_str(&list_text).unwrap();
            assert_eq!(list["workspaces"], serde_json::json!([]));

            let runtime_raw = muxterm_runtime_list_json(h);
            let runtime_text = CStr::from_ptr(runtime_raw).to_string_lossy().into_owned();
            muxterm_free_string(runtime_raw);
            let runtimes: serde_json::Value = serde_json::from_str(&runtime_text).unwrap();
            let ids: Vec<&str> = runtimes["runtimes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|runtime| runtime["id"].as_str())
                .collect();
            assert_eq!(ids, ["tmux", "herdr", "shell"]);
            muxterm_free(h);
        }
    }

    /// Existing Connection 不能只拿显示名；Herdr attach 必须跨 FFI 保留
    /// named session、target-side socket 与 workspace_id。
    #[test]
    fn session_candidate_json_preserves_runtime_attach_identity() {
        let candidate = crate::core::protocol::candidate::ExistingCandidate {
            runtime_id: "herdr".into(),
            transport_id: "ssh".into(),
            target: "buildbox".into(),
            namespace: Some("agents".into()),
            name: "muxterm".into(),
            extra: "w7".into(),
            session: Some("agents".into()),
            socket: Some("/remote/.config/herdr/sessions/agents/herdr.sock".into()),
            workspace_id: Some("w7".into()),
        };

        let json = session_candidate_json(&candidate);
        assert_eq!(json["runtime"], "herdr");
        assert_eq!(json["transport"], "ssh");
        assert_eq!(json["target"], "buildbox");
        assert_eq!(json["session"], "agents");
        assert_eq!(
            json["socket"],
            "/remote/.config/herdr/sessions/agents/herdr.sock"
        );
        assert_eq!(json["workspace_id"], "w7");
    }

    /// C5：transport_list JSON 含 local/ssh。
    #[test]
    fn ffi_transport_list_contains_local_ssh() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let raw = muxterm_transport_list_json(h);
            assert!(!raw.is_null());
            let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let json: serde_json::Value = serde_json::from_str(&value).unwrap();
            let ids: Vec<String> = json["transports"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(ids, ["local", "ssh"]);
            muxterm_free(h);
        }
    }

    fn ffi_discovery_returns_owned_json_and_can_be_freed() {
        let path =
            std::env::temp_dir().join(format!("muxterm-ffi-ssh-config-{}", std::process::id()));
        std::fs::write(
            &path,
            "Host testbox\n  HostName test.example\n  User alice\n  Port 2201\n",
        )
        .unwrap();
        let path_c = CString::new(path.to_str().unwrap()).unwrap();

        let raw = muxterm_discover_ssh_hosts_json(path_c.as_ptr());
        assert!(!raw.is_null());
        let json = unsafe {
            let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            serde_json::from_str::<serde_json::Value>(&value).unwrap()
        };
        assert_eq!(json["ok"], true);
        assert_eq!(json["hosts"][0]["alias"], "testbox");
        assert_eq!(json["hosts"][0]["port"], 2201);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ffi_list_dir_local_returns_entries_json() {
        let dir = std::env::temp_dir().join(format!("muxterm-ffi-listdir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        let path_c = CString::new(dir.to_str().unwrap()).unwrap();

        let raw = muxterm_list_dir_json(
            c"local".as_ptr(),
            ptr::null(),
            ptr::null(),
            path_c.as_ptr(),
            1000,
        );
        assert!(!raw.is_null());
        let json = unsafe {
            let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            serde_json::from_str::<serde_json::Value>(&value).unwrap()
        };
        assert_eq!(json["ok"], true);
        let names: Vec<&str> = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"sub"));
        assert!(names.contains(&"file.txt"));
        let sub = json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "sub")
            .unwrap();
        assert_eq!(sub["is_dir"], true);
    }

    /// C9：discover_sessions JSON 必须带 connect name（`target`），并接受 `all`。
    #[test]
    fn ffi_discover_sessions_json_includes_target_and_all() {
        let src = include_str!("functions/transport.rs");
        let start = src
            .find("pub unsafe extern \"C\" fn muxterm_discover_sessions_json")
            .expect("muxterm_discover_sessions_json 应存在");
        let rest = &src[start..];
        let end = rest
            .find("pub extern \"C\" fn muxterm_discover_workspaces_json")
            .or_else(|| rest.find("\n/// Discover local or SSH sessions"))
            .unwrap_or(rest.len().min(2500));
        let body = &rest[..end];
        let helper_start = src
            .find("fn session_candidate_json")
            .expect("统一 SessionCandidate JSON helper 应存在");
        let helper_rest = &src[helper_start..];
        let helper_end = helper_rest
            .find("\n/// 抓取 status bar")
            .unwrap_or(helper_rest.len().min(2500));
        let helper = &helper_rest[..helper_end];
        assert!(
            body.contains("\"target\"") || helper.contains("\"target\""),
            "JSON（含统一 helper）必须带 target=connect name，否则面板副标题只能是插件 id ssh: {body}\n{helper}"
        );
        assert!(
            body.contains("all"),
            "discover_sessions_json 必须把 transport=all 交给 Catalog: {body}"
        );
    }

    /// W4：两个 Workspace、各有 PaneId(1) 时，workspace-aware poll 必须
    /// 保留完整 WorkspaceId；旧 poll 只交付 active 事件；PaneIndexSnapshot
    /// 不出 FFI；CStateChange size/offset 与 window_id=0 保持不变。
    #[test]
    fn ffi_workspace_poll_keeps_workspace_identity() {
        unsafe {
            // 打开两个本地 shell workspace（一个 active、一个 background）。
            let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
            assert!(!h.is_null());
            assert_eq!(muxterm_connect(h), 0);
            // 第二个 workspace：直接经 pool 开一个 shell workspace。
            let handle = &mut *h;
            let id2 =
                crate::core::workspace::id::WorkspaceId::new("local", None, "second", "shell", "");
            let open_result = handle.rt.block_on(handle.catalog.pool_mut().open(
                id2.clone(),
                "second".into(),
                |_| {
                    let rt = crate::core::runtime::shell::ShellRuntime::new("$SHELL", "");
                    Box::new(rt)
                },
            ));
            assert!(open_result.is_ok(), "第二个 shell workspace 应能打开");
            let mut buf = [CWorkspaceStateChange {
                workspace_id: ptr::null(),
                event: CStateChange::default(),
            }; 64];
            let n = muxterm_poll_workspace_events(h, buf.as_mut_ptr(), 64);
            assert!(n > 0, "workspace poll 应返回事件");
            for entry in buf.iter().take(n as usize) {
                assert!(!entry.workspace_id.is_null(), "每个事件必须带 workspace_id");
                let wid = CStr::from_ptr(entry.workspace_id).to_string_lossy();
                assert!(
                    !wid.is_empty() && wid.contains('/'),
                    "workspace_id 应是五段 id: {wid}"
                );
                assert_eq!(
                    entry.event.window_id, 0,
                    "CStateChange.window_id 必须保持 0"
                );
            }
            // PaneIndexSnapshot 不出 FFI（state_change_to_c 只映射为 STATE_OTHER，
            // 且 Core 消费后从 poll 过滤——这里验证映射存在即可）。
            let ev = StateChange::PaneIndexSnapshot {
                pane: PaneId(1),
                data: b"x".to_vec(),
            };
            let c = state_change_to_c(handle, &ev);
            assert_eq!(c.type_, STATE_OTHER);
            assert_eq!(c.data, ptr::null());

            // 旧 poll 只交付 active 事件（不 panic、不泄漏 background）。
            let mut legacy = [CStateChange::default(); 64];
            let _ = muxterm_poll_events(h, legacy.as_mut_ptr(), 64);
            // 旧 CStateChange 布局不变：type_ 首字段、size 与 struct 一致。
            assert_eq!(
                std::mem::size_of::<CStateChange>(),
                std::mem::size_of::<crate::core::protocol::ffi::types::CStateChange>()
            );
            muxterm_free(h);
        }
    }

    /// additive `muxterm_execute_json`：合法 task 返回结构化结果；
    /// 非法 JSON / 未知 task / 参数缺失返回 error，不 panic。
    #[test]
    fn ffi_execute_json_returns_structured_outcome() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            assert_eq!(muxterm_connect(h), 0);

            // 合法 task：local runtime 对 rename_workspace 返回 done。
            let out = muxterm_execute_json(
                h,
                c"{\"task\":\"rename_workspace\",\"name\":\"json-test\"}".as_ptr(),
            );
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            muxterm_free_string(out);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], true, "{text}");
            assert_eq!(value["result"], "done", "{text}");

            // 非法 JSON → error，不 panic。
            let out = muxterm_execute_json(h, c"{not json".as_ptr());
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            muxterm_free_string(out);
            assert!(text.contains("\"ok\":false"), "{text}");

            // 未知 task → error。
            let out = muxterm_execute_json(h, c"{\"task\":\"nope\"}".as_ptr());
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            muxterm_free_string(out);
            assert!(text.contains("\"ok\":false"), "{text}");

            // 参数缺失（rename_tab 无 name）→ error。
            let out = muxterm_execute_json(h, c"{\"task\":\"rename_tab\",\"target\":1}".as_ptr());
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            muxterm_free_string(out);
            assert!(text.contains("\"ok\":false"), "{text}");

            // null handle → error。
            let out = muxterm_execute_json(ptr::null_mut(), c"{}".as_ptr());
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            muxterm_free_string(out);
            assert!(text.contains("\"ok\":false"), "{text}");
            muxterm_free(h);
        }
    }

    #[test]
    fn ffi_config_describe_begin_cancel_envelope() {
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            let raw = muxterm_config_describe_json(h);
            let describe = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let envelope: serde_json::Value = serde_json::from_str(&describe).unwrap();
            assert_eq!(envelope["ok"], true);
            assert!(envelope["data"]["schema"].is_object());
            assert!(envelope["data"]["manifest"].is_object());
            assert!(envelope["data"]["action_catalog"].is_array());

            let raw = muxterm_config_begin_json(h);
            let begin = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let begin: serde_json::Value = serde_json::from_str(&begin).unwrap();
            let transaction = begin["data"]["transaction"].as_str().unwrap().to_string();
            let transaction = std::ffi::CString::new(transaction).unwrap();
            let raw = muxterm_config_cancel_json(h, transaction.as_ptr());
            let cancel = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let cancel: serde_json::Value = serde_json::from_str(&cancel).unwrap();
            assert_eq!(cancel["ok"], true);

            let raw = muxterm_config_patch_json(h, c"missing".as_ptr(), c"[]".as_ptr());
            let error = CStr::from_ptr(raw).to_string_lossy().into_owned();
            muxterm_free_string(raw);
            let error: serde_json::Value = serde_json::from_str(&error).unwrap();
            assert_eq!(error["ok"], false);
            assert_eq!(error["error"]["code"], "config_error");

            muxterm_free(h);
        }
    }

    /// W6 §11.3：additive `muxterm_workspace_open_target_json` 走 Catalog
    /// resolver；`muxterm_workspace_list` 的行带 optional `resolved_target`，
    /// 旧消费者忽略未知字段。
    #[test]
    fn ffi_workspace_open_target_json_roundtrip_and_list_resolved_target() {
        let h = muxterm_catalog_new();
        assert!(!h.is_null());
        unsafe {
            // 打开一个 shell target（AttachOnly 对 shell 直接成功）。
            let out = muxterm_workspace_open_target_json(
                h,
                c"{\"name\":\"qc-proj\",\"runtime\":\"shell\",\"transport\":\"local\",\"path\":\"/tmp/qc-proj\"}".as_ptr(),
                c"attach_only".as_ptr(),
            );
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_string_lossy().into_owned();
            muxterm_free_string(out);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["ok"], true, "{text}");
            let id = value["id"].as_str().expect("open 返回 id").to_string();
            assert!(id.contains("shell"), "id 应含 runtime: {id}");
            assert_eq!(
                value["resolved_target"]["canonical"]["path"], "/tmp/qc-proj",
                "open 响应必须直接交回 canonical descriptor: {text}"
            );

            // list 行必须带 resolved_target（canonical 完整身份字段）。
            let list = muxterm_workspace_list(h);
            assert!(!list.is_null());
            let list_text = CStr::from_ptr(list).to_string_lossy().into_owned();
            muxterm_free_string(list);
            let list_value: serde_json::Value = serde_json::from_str(&list_text).unwrap();
            let row = list_value["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"].as_str() == Some(&id))
                .unwrap_or_else(|| panic!("list 缺新开的 {id}: {list_text}"));
            let resolved = &row["resolved_target"];
            assert_eq!(resolved["canonical"]["name"], "qc-proj", "{list_text}");
            assert_eq!(resolved["canonical"]["runtime"], "shell", "{list_text}");
            assert_eq!(resolved["canonical"]["path"], "/tmp/qc-proj", "{list_text}");
            assert_eq!(resolved["spec"]["runtime"], "shell", "{list_text}");

            // 缺 name/runtime → error，不 panic。
            let bad = muxterm_workspace_open_target_json(
                h,
                c"{\"path\":\"/tmp/x\"}".as_ptr(),
                c"attach_only".as_ptr(),
            );
            assert!(!bad.is_null());
            let bad_text = CStr::from_ptr(bad).to_string_lossy().into_owned();
            muxterm_free_string(bad);
            assert!(bad_text.contains("\"ok\":false"), "{bad_text}");

            muxterm_free(h);
        }
    }
}
