//! `#[no_mangle] extern "C"` 导出函数。
//!
//! 内部持有 [`TerminalModel`] + tokio runtime，对外全部同步。

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use crate::core::attention::clock::RealClock;
use crate::core::attention::engine::{AttentionEngine, AttentionNotificationKind};
use crate::core::attention::signal::AttentionSignal;
use crate::core::config::parse_hex;
use crate::core::logging::{init_logging, LoggingConfig};
use crate::core::model::layout::{LayoutNode, SplitDir};
use crate::core::model::state::StateChange;
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::model::terminal_model::TerminalModel;
use crate::core::protocol::terminal::emulate::DEFAULT_SCROLLBACK_LINES;
use crate::core::runtime::{DaemonRuntime, ShellRuntime, TmuxRuntime};
use crate::core::types::{PaneId, TabId};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::pool::{WorkspacePool, WorkspacePoolPolicy};
use crate::core::workspace::spec::WorkspaceSpec;
use crate::core::workspace::workspace::Workspace;

use super::callbacks::FfiCallbacks;
use super::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, BACKEND_STATUS_CONNECTED,
    BACKEND_STATUS_CONNECTING, BACKEND_STATUS_DISCONNECTED, BACKEND_STATUS_ERROR,
    BACKEND_STATUS_EXITED, DIR_HORIZONTAL, DIR_VERTICAL, LAYOUT_LEAF, LAYOUT_SPLIT_H,
    LAYOUT_SPLIT_V, STATE_ACTIVE_PANE_CHANGED, STATE_ACTIVE_TAB_CHANGED, STATE_BACKEND_STATUS,
    STATE_LAYOUT_CHANGED, STATE_MUTATION_SETTLED, STATE_OTHER, STATE_PANE_ADDED,
    STATE_PANE_AGENT_CHANGED, STATE_PANE_CLOSED, STATE_PANE_OUTPUT, STATE_PANE_RESIZED,
    STATE_PANE_SNAPSHOT, STATE_POOL_CHANGED, STATE_STATUS_SUBSCRIPTION, STATE_TAB_ADDED,
    STATE_TAB_CLOSED, STATE_TAB_RENAMED, STATE_WORKSPACE_RENAMED, TASK_BREAK_PANE, TASK_CLOSE_PANE,
    TASK_CLOSE_TAB, TASK_DETACH, TASK_MOVE_TAB, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE,
    TASK_REFRESH_TABS, TASK_RENAME_TAB, TASK_RENAME_WORKSPACE, TASK_SHUTDOWN, TASK_SPLIT_PANE,
    TASK_SWITCH_PANE, TASK_SWITCH_TAB, TASK_TOGGLE_PANE_FULLSCREEN,
};

/// FFI 句柄：WorkspacePool + runtime + 供 C 侧借用的缓冲。
///
/// W7：`muxterm_new()` 建空池；`muxterm_workspace_open` 开工作区。
/// 旧 `muxterm_new(backend, socket, session)` 是 deprecated 转发（macOS 暂用）。
pub struct MuxtermHandle {
    /// 进程内一份 backend 总状态（Driver/Transport/Connect/Inventory/Pool）。
    pub(crate) catalog: crate::core::catalog::Catalog,
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) callbacks: FfiCallbacks,
    /// 注意力引擎（跨工作区聚合；poll 时自动应用 PaneOutput 信号）。
    pub(crate) attention: AttentionEngine<RealClock>,
    /// `poll_events` 产出的字节 / 字符串，保证指针在下次 poll 前有效。
    event_data: Vec<Vec<u8>>,
    event_names: Vec<CString>,
    /// `get_tabs` 名称缓冲。
    tab_names: Vec<CString>,
    /// `get_layout` 节点池（连续存储，指针稳定至下次 get_layout）。
    layout_nodes: Vec<CLayoutNode>,
    /// `muxterm_poll_events` 的 C 缓冲可能小于一次 refresh 的事件数；
    /// 这里保留未返回的事件，避免 GUI 轮询 64 个事件时丢掉布局或输出。
    deferred_events: VecDeque<StateChange>,
}

impl MuxtermHandle {
    /// Catalog 里的池。
    pub(crate) fn pool(&self) -> &WorkspacePool {
        self.catalog.pool()
    }

    pub(crate) fn pool_mut(&mut self) -> &mut WorkspacePool {
        self.catalog.pool_mut()
    }

    /// 当前前台工作区。
    pub(crate) fn active_workspace(&self) -> Option<&Workspace> {
        self.pool().active()
    }

    /// 当前前台工作区（可变）。
    pub(crate) fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.pool_mut().active_mut()
    }

    fn clear_event_bufs(&mut self) {
        self.event_data.clear();
        self.event_names.clear();
    }

    fn push_name(&mut self, s: &str) -> *const c_char {
        match CString::new(s) {
            Ok(cs) => {
                self.event_names.push(cs);
                self.event_names.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        }
    }

    fn push_data(&mut self, data: &[u8]) -> (*const u8, usize) {
        self.event_data.push(data.to_vec());
        let last = self.event_data.last().unwrap();
        (last.as_ptr(), last.len())
    }

    /// 把一批事件里的 PaneOutput 信号应用到注意力引擎。
    fn apply_attention_for_events(&mut self, ws_id: &WorkspaceId, events: &[StateChange]) {
        let mut pending: Vec<(u32, Vec<AttentionSignal>, String, u64)> = Vec::new();
        {
            let Some(ws) = self.pool_mut().get_mut(ws_id) else {
                return;
            };
            for ev in events {
                if let StateChange::PaneOutput { pane, .. }
                | StateChange::PaneSnapshot { pane, .. } = ev
                {
                    let signals = ws.take_attention_signals(*pane);
                    let (last_line, seq) = ws.pane_last_line_seq(*pane);
                    pending.push((pane.0, signals, last_line, seq));
                }
            }
        }
        let ws_name = ws_id.replica_id();
        for (pane, signals, last_line, seq) in pending {
            self.attention
                .apply(&ws_name, pane, &signals, &last_line, seq);
        }
    }
}

fn cstr_opt(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

fn json_string(value: serde_json::Value) -> *mut c_char {
    let text = value.to_string();
    CString::new(text)
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}

fn json_error(error: impl std::fmt::Display) -> *mut c_char {
    json_string(serde_json::json!({
        "ok": false,
        "error": error.to_string(),
    }))
}

fn discovery_timeout(timeout_ms: u32) -> std::time::Duration {
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

/// 发现用户现有 SSH 配置中的 Host alias。
///
/// 返回的 JSON 字符串由 [`muxterm_free_string`] 释放；函数本身不会把 SSH
/// 配置复制到 Muxterm，也不会触发连接或认证。
#[no_mangle]
pub extern "C" fn muxterm_discover_ssh_hosts_json(config_path: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let path = cstr_opt(config_path);
        match crate::core::discovery::list_ssh_hosts(path.as_deref().map(std::path::Path::new)) {
            Ok(hosts) => json_string(serde_json::json!({
                "ok": true,
                "hosts": hosts,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("SSH host discovery panic"))
}

/// 列出本地或远端目录条目（`name` + `is_dir`），供「选起始目录」UI 逐步浏览。
///
/// `transport_type` 为 `local` 或 `ssh`；SSH 模式下 `target` 是 `~/.ssh/config`
/// 中的 alias。返回 `{"ok":true,"entries":[...]}`，字符串由
/// [`muxterm_free_string`] 释放。`path` 为空时：本地取 HOME，SSH 取 `~`。
#[no_mangle]
pub extern "C" fn muxterm_list_dir_json(
    transport_type: *const c_char,
    target: *const c_char,
    config_path: *const c_char,
    path: *const c_char,
    timeout_ms: u32,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let transport = cstr_opt(transport_type)
            .unwrap_or_else(|| "local".into())
            .to_ascii_lowercase();
        let target = cstr_opt(target);
        let config_path = cstr_opt(config_path);
        let path = cstr_opt(path).unwrap_or_else(|| {
            if transport == "ssh" {
                "~".to_string()
            } else {
                ".".to_string()
            }
        });
        let result = match transport.as_str() {
            "local" => {
                let expanded = if path == "~" {
                    std::env::var("HOME").unwrap_or_else(|_| ".".into())
                } else {
                    path
                };
                Ok(crate::core::discovery::list_local_dir(
                    std::path::Path::new(&expanded),
                ))
            }
            "ssh" => {
                let Some(alias) = target.as_deref().filter(|value| !value.trim().is_empty()) else {
                    return json_error("SSH directory listing requires a host alias");
                };
                crate::core::discovery::list_remote_dir(
                    alias,
                    &path,
                    config_path.as_deref(),
                    discovery_timeout(timeout_ms),
                )
            }
            _ => return json_error(format!("unsupported directory transport: {transport}")),
        };
        match result {
            Ok(entries) => json_string(serde_json::json!({
                "ok": true,
                "entries": entries,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("directory listing panic"))
}

/// 新建项目卡数据源：Catalog::runtime_list（登记顺序，含 support / transports）。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_runtime_list_json(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let list = (*h).catalog.runtime_list();
        json_string(serde_json::json!({
            "ok": true,
            "runtimes": list.iter().map(|r| serde_json::json!({
                "id": r.id,
                "name": r.name,
                "support": r.support.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>(),
                "accepted_transports": r.accepted_transports,
            })).collect::<Vec<_>>(),
        }))
    }))
    .unwrap_or_else(|_| json_error("runtime list panic"))
}

/// Transport 插件表：Catalog::transport_list。
///
/// # Safety
/// `h` 有效且未 free。
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

/// 列出某个 Transport 的 target（Local 单例 / SSH hosts）。
///
/// # Safety
/// `h` 有效且未 free；`transport` NUL 结尾。
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

/// 扇出发现：该 target 上各 Driver 的可 attach 格子（tmux + herdr）。
///
/// # Safety
/// `h` 有效且未 free；`transport` / `target` NUL 结尾。
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
        // transport=all 时 Catalog 扇出全部 connect name（local + SSH alias）。
        match (*h).catalog.discover_sessions(&transport, &target) {
            Ok(rows) => json_string(serde_json::json!({
                "ok": true,
                "workspaces": rows.iter().map(|r| {
                    let target = if r.transport_id == "local" {
                        "local".to_string()
                    } else {
                        r.target.clone()
                    };
                    serde_json::json!({
                        "id": format!("{}/{}/{}/{}", r.transport_id, target, r.runtime_id, r.name),
                        "name": r.name,
                        "runtime": r.runtime_id,
                        "transport": r.transport_id,
                        "target": target,
                        "in_pool": false,
                    })
                }).collect::<Vec<_>>(),
            })),
            Err(e) => json_error(e),
        }
    }))
    .unwrap_or_else(|_| json_error("discover sessions panic"))
}

/// 通过 core 发现 local 或 SSH tmux session（W7：workspace 发现）。
///
/// `transport_type` 为 `local` 或 `ssh`；SSH 模式下 `target` 是 `~/.ssh/config`
/// 中的 alias。所有连接选项仍由系统 `ssh` 读取，`config_path` 仅供测试或显式
/// 配置使用。返回的 JSON 字符串由 [`muxterm_free_string`] 释放。
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
        // C5：走 Catalog::discover_sessions 扇出（tmux + herdr），保持 §6.2 形状。
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

/// deprecated 别名：`muxterm_discover_tmux_sessions_json`（W7 改名）。
#[no_mangle]
pub extern "C" fn muxterm_discover_tmux_sessions_json(
    transport_type: *const c_char,
    target: *const c_char,
    socket: *const c_char,
    config_path: *const c_char,
    timeout_ms: u32,
) -> *mut c_char {
    muxterm_discover_workspaces_json(transport_type, target, socket, config_path, timeout_ms)
}

/// C9：SessionCandidate → §6.2 JSON（target = connect name；id 含 connect name）。
fn session_candidate_json(r: &crate::core::catalog::driver::SessionCandidate) -> serde_json::Value {
    let target = if r.transport_id == "local" {
        "local".to_string()
    } else {
        r.target.clone()
    };
    serde_json::json!({
        "id": format!("{}/{}/{}/{}", r.transport_id, target, r.runtime_id, r.name),
        "name": r.name,
        "runtime": r.runtime_id,
        "transport": r.transport_id,
        "target": target,
        "in_pool": false,
    })
}
/// 抓取 status bar 快照（tmux 兼容：`show -g` / `show -w -g` + `display-message`）。
///
/// `transport_type` 为 `local` 或 `ssh`；SSH 模式下 `target` 是
/// `~/.ssh/config` 的 alias。返回 `{"ok":true,"status":{...}}` JSON 字符串，
/// 由 [`muxterm_free_string`] 释放。只读命令，不干扰控制客户端。
#[no_mangle]
pub extern "C" fn muxterm_status_snapshot_json(
    transport_type: *const c_char,
    target: *const c_char,
    socket: *const c_char,
    session: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let transport = cstr_opt(transport_type)
            .unwrap_or_else(|| "local".into())
            .to_ascii_lowercase();
        let ssh_alias = if transport == "ssh" {
            cstr_opt(target)
        } else {
            None
        };
        let session = match cstr_opt(session) {
            Some(s) if !s.trim().is_empty() => s,
            _ => {
                return json_error("session 为空");
            }
        };
        let cfg = crate::core::runtime::tmux::status::StatusQueryConfig {
            socket: cstr_opt(socket),
            ssh_alias,
            session,
        };
        match crate::core::runtime::tmux::status::fetch_snapshot(&cfg) {
            Ok(status) => json_string(serde_json::json!({
                "ok": true,
                "status": status,
            })),
            Err(error) => json_error(error),
        }
    }))
    .unwrap_or_else(|_| json_error("status snapshot panic"))
}

/// 当前 tmux 后端是否已启用 status bar 订阅（`refresh-client -B`）。
///
/// 返回 1 = 已启用（前端关闭轮询定时器，由 `%subscription-changed` 推送）；
/// 0 = 未启用（tmux < 3.2 / 非 tmux 后端 / 发送失败，前端回退轮询）。
///
/// # Safety
/// `handle` 必须是 [`muxterm_create`] 返回且尚未释放的指针。
#[no_mangle]
pub unsafe extern "C" fn muxterm_status_subscription_active(handle: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return 0;
        }
        let h = unsafe { &*handle };
        i32::from(
            h.active_workspace()
                .map(|w| w.runtime().status_subscriptions_active())
                .unwrap_or(false),
        )
    }))
    .unwrap_or(0)
}

/// 当前连接累计下行字节（SSH transport 读端计数；非 SSH 为 0）。
///
/// # Safety
/// `handle` 必须是 [`muxterm_create`] 返回且尚未释放的指针。
#[no_mangle]
pub unsafe extern "C" fn muxterm_traffic_down(handle: *mut MuxtermHandle) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return 0;
        }
        let h = unsafe { &*handle };
        h.active_workspace()
            .map(|w| w.runtime().traffic_bytes().0)
            .unwrap_or(0)
    }))
    .unwrap_or(0)
}

/// 当前连接累计上行字节（SSH PtyWriter 计数；非 SSH 为 0）。
///
/// # Safety
/// `handle` 必须是 [`muxterm_create`] 返回且尚未释放的指针。
#[no_mangle]
pub unsafe extern "C" fn muxterm_traffic_up(handle: *mut MuxtermHandle) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return 0;
        }
        let h = unsafe { &*handle };
        h.active_workspace()
            .map(|w| w.runtime().traffic_bytes().1)
            .unwrap_or(0)
    }))
    .unwrap_or(0)
}

/// 通过 core 创建 detached tmux session（W7：workspace create）。
/// 随后由调用方使用同一 alias/session 进入控制模式。返回
/// `{"ok":true,"session":"..."}`，字符串由 [`muxterm_free_string`] 释放。
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
            "local" => crate::core::discovery::create_local_tmux_session(
                socket.as_deref(),
                &session,
                &directory,
            ),
            "ssh" => {
                let Some(alias) = target.as_deref().filter(|value| !value.trim().is_empty()) else {
                    return json_error("SSH session creation requires a host alias");
                };
                crate::core::discovery::create_ssh_tmux_session(
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

/// deprecated 别名：`muxterm_create_tmux_session_json`（W7 改名）。
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

fn task_result_code(result: anyhow::Result<TaskOutcome>) -> i32 {
    match result {
        Ok(TaskOutcome::Done) => 0,
        Ok(TaskOutcome::Rejected { .. }) | Err(_) => -1,
    }
}

/// 创建 handle（deprecated 转发：建空池并打开一个工作区）。
///
/// W7 起新代码用 [`muxterm_workspace_open`]；本函数保留给 macOS 暂用。
/// `runtime_type`：`"local"` / `"tmux"` / `"daemon"`（大小写不敏感）。
/// `socket` / `session`：tmux 模式可选；daemon 用 `session` 推导 socket 路径；local 忽略。
///
/// 失败返回 null。
#[no_mangle]
pub extern "C" fn muxterm_new(
    runtime_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
) -> *mut MuxtermHandle {
    let kind = cstr_opt(runtime_type)
        .unwrap_or_else(|| "local".into())
        .to_ascii_lowercase();
    let sock = cstr_opt(socket);
    let sess = cstr_opt(session);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return ptr::null_mut(),
    };
    let mut catalog = crate::core::catalog::Catalog::with_builtins();

    let (id, name, runtime, scrollback_lines) =
        match legacy_runtime_spec(&kind, sock, sess, None, None) {
            Some(spec) => spec,
            None => return ptr::null_mut(),
        };
    let fut =
        catalog
            .pool_mut()
            .open_with_scrollback(id.clone(), name, scrollback_lines, move |_| runtime);
    if rt.block_on(fut).is_err() {
        return ptr::null_mut();
    }

    let attention_config = crate::core::config::Config::load()
        .map(|c| c.attention)
        .unwrap_or_default();
    Box::into_raw(Box::new(MuxtermHandle {
        catalog,
        rt,
        callbacks: FfiCallbacks::default(),
        attention: AttentionEngine::new(attention_config, RealClock),
        event_data: Vec::new(),
        event_names: Vec::new(),
        tab_names: Vec::new(),
        layout_nodes: Vec::new(),
        deferred_events: VecDeque::new(),
    }))
}

/// 创建 handle 并直接连接（deprecated 转发：开一个工作区）。
///
/// 相比 [`muxterm_new`]（+ [`muxterm_connect`] 两步），此函数一步完成建连。
/// W7 起新代码用 [`muxterm_workspace_open`]。
///
/// - `runtime_type`：`"local"` / `"tmux"` / `"daemon"` / `"tmux-ssh"`
/// - `socket`：tmux 的 `-L` socket 名（本地 tmux），SSH 模式为远端 socket（可选）
/// - `session`：attach 的目标 session 名（非空 → attach 模式；空 → new-session）
/// - `ssh_alias`：SSH 模式下的 `~/.ssh/config` Host 名（仅 `tmux-ssh` 用）
/// - `start_directory`：new-session 的起始工作目录（可选）
///
/// 失败返回 null。
#[no_mangle]
pub extern "C" fn muxterm_new_connect(
    runtime_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
    ssh_alias: *const c_char,
    start_directory: *const c_char,
) -> *mut MuxtermHandle {
    let kind = cstr_opt(runtime_type)
        .unwrap_or_else(|| "local".into())
        .to_ascii_lowercase();
    let sock = cstr_opt(socket);
    let sess = cstr_opt(session);
    let alias = cstr_opt(ssh_alias);
    let start_dir = cstr_opt(start_directory);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return ptr::null_mut(),
    };
    let mut catalog = crate::core::catalog::Catalog::with_builtins();

    let (id, name, runtime, scrollback_lines) =
        match legacy_runtime_spec(&kind, sock, sess, alias, start_dir) {
            Some(spec) => spec,
            None => return ptr::null_mut(),
        };
    let fut =
        catalog
            .pool_mut()
            .open_with_scrollback(id.clone(), name, scrollback_lines, move |_| runtime);
    if rt.block_on(fut).is_err() {
        return ptr::null_mut();
    }

    let attention_config = crate::core::config::Config::load()
        .map(|c| c.attention)
        .unwrap_or_default();
    Box::into_raw(Box::new(MuxtermHandle {
        catalog,
        rt,
        callbacks: FfiCallbacks::default(),
        attention: AttentionEngine::new(attention_config, RealClock),
        event_data: Vec::new(),
        event_names: Vec::new(),
        tab_names: Vec::new(),
        layout_nodes: Vec::new(),
        deferred_events: VecDeque::new(),
    }))
}

/// 旧 `muxterm_new` / `muxterm_new_connect` 的 runtime 规格（deprecated 转发）。
fn legacy_runtime_spec(
    kind: &str,
    sock: Option<String>,
    sess: Option<String>,
    alias: Option<String>,
    start_dir: Option<String>,
) -> Option<(
    WorkspaceId,
    String,
    std::boxed::Box<dyn crate::core::model::Runtime>,
    usize,
)> {
    let scrollback_lines = configured_scrollback_lines();
    let runtime: std::boxed::Box<dyn crate::core::model::Runtime> = match kind {
        "tmux" => {
            let sock_ref = sock.as_deref();
            let mut tmux = if let Some(name) = sess.as_deref() {
                TmuxRuntime::new_with_attach(sock_ref, name)
            } else if let Some(dir) = start_dir.as_deref() {
                TmuxRuntime::new_with_cwd(sock_ref, Some(dir))
            } else {
                TmuxRuntime::new(sock_ref)
            };
            tmux.set_scrollback_lines(scrollback_lines as u32);
            std::boxed::Box::new(tmux)
        }
        "ssh" | "tmux-ssh" => {
            let (alias_name, sock_owned) =
                TmuxRuntime::ssh_alias_and_tmux_socket(sock.as_deref(), alias.as_deref())?;
            let sock_ref = sock_owned.as_deref();
            let mut tmux = if let Some(name) = sess.as_deref() {
                TmuxRuntime::new_ssh_attach(&alias_name, sock_ref, name)
            } else {
                TmuxRuntime::new_ssh(&alias_name, sock_ref)
            };
            tmux.set_scrollback_lines(scrollback_lines as u32);
            std::boxed::Box::new(tmux)
        }
        "daemon" => {
            let name = sess.clone().unwrap_or_else(|| "default".into());
            let path = sock
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| DaemonRuntime::default_socket_path(&name));
            std::boxed::Box::new(DaemonRuntime::new(path, name))
        }
        _ => std::boxed::Box::new(ShellRuntime::new(
            "$SHELL",
            start_dir.as_deref().unwrap_or(""),
        )),
    };
    let transport = if matches!(kind, "ssh" | "tmux-ssh") {
        "ssh"
    } else {
        "local"
    };
    let runtime_kind = if matches!(kind, "daemon") {
        "daemon"
    } else if matches!(kind, "ssh" | "tmux-ssh") || kind == "tmux" {
        "tmux"
    } else {
        "shell"
    };
    let session = sess.unwrap_or_default();
    let id = WorkspaceId::new(transport, alias.as_deref(), &session, runtime_kind, "");
    let name = if session.is_empty() {
        "muxterm".to_string()
    } else {
        session.clone()
    };
    Some((id, name, runtime, scrollback_lines))
}

/// FFI legacy/workspace-open 没有单独的 scrollback 参数时，读取用户配置。
/// 配置不可读时回退到 core 默认值，不能让 attach 直接失去历史。
fn configured_scrollback_lines() -> usize {
    crate::core::config::Config::load()
        .map(|config| config.scrollback.lines.max(1) as usize)
        .unwrap_or(DEFAULT_SCROLLBACK_LINES)
}

/// 打开一个工作区并设为前台。0=ok，-1=err。
///
/// `transport`：`"local"` / `"ssh"`；`runtime`：`"tmux"` / `"shell"` / `"daemon"`。
/// `alias`：SSH 的 `~/.ssh/config` Host 名；`session`：tmux session 名；
/// `path`：shell 工作目录 / daemon socket 路径；`socket`：tmux `-L` socket 名。
/// tmux 的 capture 与 Workspace/PaneBuf 历史上限统一读取 `[scrollback].lines`。
///
/// # Safety
/// `h` 有效且未 free；字符串参数 NUL 结尾。
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
        // Catalog 路径：未知 runtime / 不接受 transport → -1，不悄悄变 shell。
        let spec = WorkspaceSpec {
            transport: transport.clone(),
            alias: alias.clone(),
            session: session.clone(),
            runtime: runtime.clone(),
            path: path.clone(),
            socket: socket.clone(),
            create: false,
            scrollback_lines: configured_scrollback_lines() as u32,
        };
        let fut = handle.catalog.open(&spec);
        match handle.rt.block_on(fut) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }))
    .unwrap_or(-1)
}

/// 列出池里全部工作区，返回 JSON 字符串（`muxterm_free_string` 释放）。
///
/// # Safety
/// `h` 有效且未 free。
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
                serde_json::json!({
                    "id": w.id().as_str(),
                    "name": w.name(),
                    "runtime": w.state().workspace_runtime(),
                    "active": handle.pool().active_id() == Some(w.id()),
                })
            })
            .collect();
        json_string(serde_json::json!({ "ok": true, "workspaces": workspaces }))
    }))
    .unwrap_or_else(|_| json_error("workspace list panic"))
}

/// 激活池里一个工作区。0=ok，-1=err。
///
/// `id` 是 `muxterm_workspace_list` 返回的 `id` 字符串。
///
/// # Safety
/// `h` 有效且未 free；`id` NUL 结尾。
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

/// 关闭池里一个工作区（tmux Detach / shell Shutdown）。0=ok，-1=err。
///
/// # Safety
/// `h` 有效且未 free；`id` NUL 结尾。
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

/// 从 `transport/alias/session/runtime/path` 字符串解析 WorkspaceId。
fn parse_workspace_id(id: &str) -> WorkspaceId {
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

/// 释放 handle。
///
/// # Safety
/// `h` 必须来自 `muxterm_new`，且只 free 一次。
#[no_mangle]
pub unsafe extern "C" fn muxterm_free(h: *mut MuxtermHandle) {
    if h.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut handle = Box::from_raw(h);
        handle.pool_mut().shutdown_all();
    }));
}

/// 连接后端。0=ok，-1=err。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_connect(h: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let MuxtermHandle { catalog, rt, .. } = handle;
        let Some(ws) = catalog.pool_mut().active_mut() else {
            return -1;
        };
        match rt.block_on(ws.connect()) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    }))
    .unwrap_or(-1)
}

/// 关闭后端。0=ok，-1=err。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_shutdown(h: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        handle.pool_mut().shutdown_all();
        0
    }))
    .unwrap_or(-1)
}

/// 分离当前 control client，但保留 tmux session / daemon。
///
/// 这是一个独立于 `muxterm_shutdown` 的前端动作；调用方随后仍应释放
/// handle。所有异常都转成 -1，不能让 FFI 边界 panic 到 GUI 进程。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_detach(h: *mut MuxtermHandle) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        match handle.active_workspace_mut() {
            Some(ws) => task_result_code(ws.execute(Task::Detach)),
            None => -1,
        }
    }))
    .unwrap_or(-1)
}

fn ctask_to_task(task: &CTask, ws: &Workspace) -> Option<Task> {
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
            before: task.dir == super::types::TAB_MOVE_BEFORE,
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
        _ => None,
    }
}

/// CTask 的 pane id 需要兼容 tmux 合法的 PaneId(0)。
///
/// macOS 会把可见 pane 的真实 id（包括 0）传进来；若当前后端没有
/// PaneId(0)，才把 0 解释为“当前 active pane”的旧兼容语义。
fn resolve_c_task_pane(raw: u32, ws: &Workspace) -> PaneId {
    if raw == 0 && ws.state().pane(&PaneId(0)).is_none() {
        ws.state().active_pane().map(|p| p.id).unwrap_or(PaneId(0))
    } else {
        PaneId(raw)
    }
}

/// 执行一个 Task。0=ok，-1=err。
///
/// # Safety
/// `h` / `task` 有效。
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

fn state_change_to_c(handle: &mut MuxtermHandle, ev: &StateChange) -> CStateChange {
    let mut out = CStateChange::default();
    match ev {
        StateChange::PaneOutput { pane, data } | StateChange::PaneFrame { pane, data } => {
            out.type_ = STATE_PANE_OUTPUT;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::PaneSnapshot { pane, data } => {
            out.type_ = STATE_PANE_SNAPSHOT;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::TabAdded { tab } => {
            out.type_ = STATE_TAB_ADDED;
            out.tab_id = tab.0;
        }
        StateChange::TabClosed { tab } => {
            out.type_ = STATE_TAB_CLOSED;
            out.tab_id = tab.0;
        }
        StateChange::LayoutChanged { tab, .. } => {
            out.type_ = STATE_LAYOUT_CHANGED;
            out.tab_id = tab.0;
        }
        StateChange::PaneAdded { pane, tab } => {
            out.type_ = STATE_PANE_ADDED;
            out.pane_id = pane.0;
            out.tab_id = tab.0;
        }
        StateChange::PaneClosed { pane } => {
            out.type_ = STATE_PANE_CLOSED;
            out.pane_id = pane.0;
        }
        StateChange::ActiveTabChanged { tab } => {
            out.type_ = STATE_ACTIVE_TAB_CHANGED;
            out.tab_id = tab.0;
        }
        StateChange::ActivePaneChanged { tab, pane } => {
            out.type_ = STATE_ACTIVE_PANE_CHANGED;
            out.tab_id = tab.0;
            out.pane_id = pane.0;
        }
        StateChange::TabRenamed { tab, name } => {
            out.type_ = STATE_TAB_RENAMED;
            out.tab_id = tab.0;
            out.name = handle.push_name(name);
        }
        StateChange::PaneResized { pane, cols, rows } => {
            out.type_ = STATE_PANE_RESIZED;
            out.pane_id = pane.0;
            // 复用 window_id / tab_id 传尺寸不合适；放在 data 里
            let bytes = [
                *cols as u8,
                (*cols >> 8) as u8,
                *rows as u8,
                (*rows >> 8) as u8,
            ];
            let (p, n) = handle.push_data(&bytes);
            out.data = p;
            out.data_len = n;
        }
        StateChange::PaneAgentChanged {
            pane,
            agent,
            initial,
        } => {
            out.type_ = STATE_PANE_AGENT_CHANGED;
            out.pane_id = pane.0;
            // FFI 只暴露 Runtime-neutral 产品模型；Herdr event 名/public id
            // 已在 Runtime 内归一化，macOS/TUI 不需要识别来源。
            let payload = serde_json::to_vec(&serde_json::json!({
                "initial": initial,
                "agent": agent,
            }))
            .unwrap_or_else(|_| b"{\"initial\":false,\"agent\":null}".to_vec());
            let (ptr, len) = handle.push_data(&payload);
            out.data = ptr;
            out.data_len = len;
        }
        StateChange::StatusBarSubscription { name, value, pane } => {
            out.type_ = STATE_STATUS_SUBSCRIPTION;
            out.pane_id = pane.map(|p| p.0).unwrap_or(0);
            out.name = handle.push_name(name);
            let (ptr, len) = handle.push_data(value.as_bytes());
            out.data = ptr;
            out.data_len = len;
        }
        StateChange::WorkspaceRenamed { name } => {
            out.type_ = STATE_WORKSPACE_RENAMED;
            out.name = handle.push_name(name);
        }
        StateChange::PoolChanged => {
            out.type_ = STATE_POOL_CHANGED;
        }
        StateChange::BackendStatusChanged(status) => {
            out.type_ = STATE_BACKEND_STATUS;
            out.pane_id = match status {
                crate::core::model::state::BackendStatus::Disconnected => {
                    BACKEND_STATUS_DISCONNECTED
                }
                crate::core::model::state::BackendStatus::Connecting => BACKEND_STATUS_CONNECTING,
                crate::core::model::state::BackendStatus::Connected => BACKEND_STATUS_CONNECTED,
                crate::core::model::state::BackendStatus::Error => BACKEND_STATUS_ERROR,
                crate::core::model::state::BackendStatus::Exited => BACKEND_STATUS_EXITED,
            };
        }
        StateChange::PaneTitleChanged { pane, title } => {
            out.type_ = STATE_OTHER;
            out.pane_id = pane.0;
            out.name = handle.push_name(title);
        }
        // Index 专属快照：Core 消费后从 FFI 输出过滤（见 poll 路径）。
        StateChange::PaneIndexSnapshot { .. } => {
            out.type_ = STATE_OTHER;
        }
        StateChange::MutationSettled {
            operation_id,
            kind,
            result,
        } => {
            out.type_ = STATE_MUTATION_SETTLED;
            // 既有 data buffer 携带完整 JSON；不扩 struct、不吞异步失败。
            let payload = serde_json::to_vec(&serde_json::json!({
                "operation_id": operation_id,
                "kind": kind,
                "result": result,
            }))
            .unwrap_or_else(|_| {
                b"{\"operation_id\":0,\"kind\":\"new_tab\",\"result\":{\"stage\":\"queue\"}}"
                    .to_vec()
            });
            let (ptr, len) = handle.push_data(&payload);
            out.data = ptr;
            out.data_len = len;
        }
    }
    out
}

/// 非阻塞拉取事件，写入 `out[0..]`，返回写入数量（或 -1）。
///
/// 会先 `refresh()` 拉取 runtime 增量（含 pty 输出）。
///
/// # Safety
/// `out` 至少 `max_count` 个元素；返回的指针在下次 poll/free 前有效。
#[no_mangle]
pub unsafe extern "C" fn muxterm_poll_events(
    h: *mut MuxtermHandle,
    out: *mut CStateChange,
    max_count: i32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() || out.is_null() || max_count <= 0 {
            return -1;
        }
        let handle = &mut *h;
        handle.clear_event_bufs();
        // 后台工作区事件也拉取（W7：池在 core）。
        let mut fresh: Vec<StateChange> = Vec::new();
        for (ws_id, events) in handle.pool_mut().poll_background() {
            handle.apply_attention_for_events(&ws_id, &events);
            fresh.extend(events);
        }
        if let Some(ws) = handle.active_workspace_mut() {
            let events = ws.refresh();
            let ws_id = handle.pool().active_id().cloned();
            if let Some(ws_id) = ws_id {
                handle.apply_attention_for_events(&ws_id, &events);
            }
            fresh.extend(events);
        }
        handle.deferred_events.extend(fresh);
        let n = handle.deferred_events.len().min(max_count as usize);
        let slice = std::slice::from_raw_parts_mut(out, n);
        let ready: Vec<StateChange> = handle.deferred_events.drain(..n).collect();
        for (i, ev) in ready.iter().enumerate() {
            let c = state_change_to_c(handle, ev);
            // 回调
            if let StateChange::PaneOutput { pane, data } | StateChange::PaneFrame { pane, data } =
                ev
            {
                if let Some(cb) = handle.callbacks.on_output {
                    cb(pane.0, data.as_ptr(), data.len());
                }
            }
            if let Some(cb) = handle.callbacks.on_state_change {
                cb(&c);
            }
            slice[i] = c;
        }
        n as i32
    }))
    .unwrap_or(-1)
}

/// 向 pane 写入原始字节。0=ok，-1=err。
///
/// # Safety
/// `data` 至少 `len` 字节。
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

/// 向 pane 写入原始字节，但**不**触发注意力 `on_user_input`（W19-E：
/// 注意力 reply overlay 的快速回复不应把 Blocked 清成 Idle）。
///
/// # Safety
/// `data` 至少 `len` 字节。
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

/// 向 tmux 上报 pane 的前景/背景色（`refresh-client -r`），供 OSC 10/11
/// 查询代答。颜色为 `#rrggbb` / `rrggbb`。0=ok，-1=err。
///
/// # Safety
/// `fg_hex` / `bg_hex` 必须是 NUL 结尾字符串。
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

/// 向 tmux 上报**所有** pane 的前景/背景色（`refresh-client -r`）。
///
/// 主题切换后必须整段对齐，否则后台 tab 的 codex/agent 输入框会沿用旧
/// 主题的颜色代答（白/黑输入框与当前主题相反时看不清）。0=ok，-1=err。
///
/// # Safety
/// `fg_hex` / `bg_hex` 必须是 NUL 结尾字符串。
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

/// C ABI 中 `0` 既是历史上的 active-pane 哨兵，也可能是真实的 tmux pane id。
/// 只有当前状态不存在 PaneId(0) 时才使用旧哨兵语义。
fn resolve_c_io_pane(raw: u32, ws: &Workspace) -> Option<PaneId> {
    if raw == 0 && ws.state().pane(&PaneId(0)).is_none() {
        ws.state().active_pane().map(|p| p.id)
    } else {
        Some(PaneId(raw))
    }
}

/// 调整 pane 的 pty 行列。0=ok，-1=err。
///
/// # Safety
/// `h` 有效。
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

/// 调整 tmux 控制 client 的字符格尺寸。0=ok，-1=err。
///
/// # Safety
/// `h` 有效。
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

/// 调整分割条相邻 pane 的单一轴尺寸。0=ok，-1=err。
///
/// `axis` 使用 `DIR_HORIZONTAL`（宽度）或 `DIR_VERTICAL`（高度）。
/// # Safety
/// `h` 有效。
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

/// 列出 tabs，返回写入数量。
///
/// # Safety
/// `out` 至少 `max_count` 个；`name` 指针在下次 get_tabs/free 前有效。
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

/// 列出某 tab 下 panes，返回写入数量。
///
/// # Safety
/// `out` 至少 `max_count` 个。
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
    // tmux window index 也是 0 基的，tab_id==0 是真实 tab，不能当作“active”哨兵。
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

/// 读取 pane 累计输出到 `buf`，返回写入字节数（截断到 buf_len），-1=err。
///
/// 当累计输出超过 `buf_len` 时，拷贝**最近**的 `buf_len` 字节（尾部），而不是
/// 最旧的头部：前端拿到的快照必须代表「当前屏幕附近」的内容，否则持续输出的
/// pane（htop/codex/agent）超过前端缓冲后，增量对账会永远对着一段陈旧头部，
/// 导致渲染冻结或乱码。
///
/// # Safety
/// `buf` 至少 `buf_len` 字节。
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

/// 导出 tab 布局树到 `out`（根节点写到 *out，子节点在 handle 内部池）。
///
/// 返回 0=ok，-1=err。
///
/// # Safety
/// `out` 非空；子节点指针在下次 get_layout/free 前有效。
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
    // tmux window index 0 基，tab_id==0 是真实 tab。
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
    // 重新修正指针（push 时用 index，最后一遍 fixup）
    fixup_layout_pointers(&mut handle.layout_nodes);
    *out = handle.layout_nodes[root_idx];
    0
}

/// 用 index 占位的临时节点；fixup 后 first/second 成真实指针。
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
            // 先占位
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
            // 暂存 index 到指针低位（仅内部，随后 fixup）
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

/// 跨全部工作区搜索 pane 文本，返回 JSON 命中列表。
///
/// 返回 `{"ok": true, "hits": [{"workspace_id", "tab_id", "pane_id", "seq", "line"}]}`。
///
/// # Safety
/// `h` 有效且未 free；`query` NUL 结尾。
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

/// 注意力引擎快照，返回 JSON。
///
/// 返回 `{"ok": true, "blocked_count": N, "workspaces": [...]}`。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_snapshot(h: *mut MuxtermHandle) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &*h;
        let workspaces: Vec<serde_json::Value> = handle
            .attention
            .snapshot()
            .into_iter()
            .map(|ws| {
                // 从池里找工作区路径（W19 注意力行标题需要 path）。
                let path = handle
                    .pool()
                    .list()
                    .iter()
                    .find(|w| w.id().replica_id() == ws.workspace_id)
                    .map(|w| w.id().path.as_str())
                    .filter(|p| !p.trim().is_empty())
                    .unwrap_or("~");
                serde_json::json!({
                    "workspace_id": ws.workspace_id,
                    "path": path,
                    "blocked": ws.blocked,
                    "done": ws.done,
                    "working": ws.working,
                    "panes": ws.panes.iter().map(|p| {
                        serde_json::json!({
                            "pane_id": p.pane_id,
                            "status": format!("{:?}", p.status).to_lowercase(),
                            "last_line": p.last_line,
                            "seq": p.seq,
                            "process_name": p.process_name,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        json_string(serde_json::json!({
            "ok": true,
            "blocked_count": handle.attention.blocked_workspace_count(),
            "workspaces": workspaces,
        }))
    }))
    .unwrap_or_else(|_| json_error("attention snapshot panic"))
}

/// 取走本轮新进入 blocked / done 的工作区通知，返回 JSON。
///
/// 返回 `{"ok": true, "notifications": [...], "blocked": [...], "done": [...]}`。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_take_notifications(
    h: *mut MuxtermHandle,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return json_error("handle 为空");
        }
        let handle = &mut *h;
        let notifications = handle.attention.take_notifications();
        let blocked = notifications
            .iter()
            .filter(|n| n.kind == AttentionNotificationKind::Blocked)
            .map(|n| n.workspace_id.clone())
            .collect::<Vec<_>>();
        let done = notifications
            .iter()
            .filter(|n| n.kind == AttentionNotificationKind::Done)
            .map(|n| n.workspace_id.clone())
            .collect::<Vec<_>>();
        let records = notifications
            .into_iter()
            .map(|n| {
                serde_json::json!({
                    "workspace_id": n.workspace_id,
                    "pane_id": n.pane_id,
                    "kind": match n.kind {
                        AttentionNotificationKind::Blocked => "blocked",
                        AttentionNotificationKind::Done => "done",
                    },
                    "process_name": n.process_name,
                    "last_line": n.last_line,
                    "seq": n.seq,
                })
            })
            .collect::<Vec<_>>();
        json_string(serde_json::json!({
            "ok": true,
            "notifications": records,
            "blocked": blocked,
            "done": done,
        }))
    }))
    .unwrap_or_else(|_| json_error("attention notifications panic"))
}

/// 标记某 pane 成为前台可见（Done → Idle；Blocked 保持）。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_on_became_visible(
    h: *mut MuxtermHandle,
    pane_id: u32,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle
            .attention
            .on_became_visible(&ws_id.replica_id(), pane_id);
        0
    }))
    .unwrap_or(-1)
}

/// 显式确认某 pane 的通知已读（Blocked/Done → Idle）。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_acknowledge(h: *mut MuxtermHandle, pane_id: u32) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle.attention.acknowledge(&ws_id.replica_id(), pane_id);
        0
    }))
    .unwrap_or(-1)
}

/// 更新某 pane 的进程名（注意力列表展示用）。
///
/// # Safety
/// `h` 有效且未 free；`name` NUL 结尾（可为 NULL）。
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_set_process_name(
    h: *mut MuxtermHandle,
    pane_id: u32,
    name: *const c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle
            .attention
            .set_process_name(&ws_id.replica_id(), pane_id, cstr_opt(name));
        0
    }))
    .unwrap_or(-1)
}

/// 静音某 pane 一段时间（秒），不进红点、不通知。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_attention_mute(
    h: *mut MuxtermHandle,
    pane_id: u32,
    seconds: u64,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if h.is_null() {
            return -1;
        }
        let handle = &mut *h;
        let Some(ws_id) = handle.pool().active_id() else {
            return -1;
        };
        handle.attention.mute_for(
            &ws_id.replica_id(),
            pane_id,
            std::time::Duration::from_secs(seconds),
        );
        0
    }))
    .unwrap_or(-1)
}

/// 读取某 pane 的滚动窗口 ANSI 字节（历史查看用）。
///
/// 返回写入字节数（截断到 buf_len），-1=err。
///
/// # Safety
/// `h` 有效且未 free；`buf` 至少 `buf_len` 字节。
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

/// 读取某 pane 内置 VT 的可见网格 ANSI（首屏播种：只给当前屏，不重放 history）。
///
/// 返回写入字节数（截断到 buf_len），-1=err。
///
/// # Safety
/// `h` 有效且未 free；`buf` 至少 `buf_len` 字节。
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

/// 读取某 pane 的一次性 Surface seed。
///
/// `buf` 为空或 `buf_len == 0` 时只返回所需长度；否则复制最多 `buf_len`
/// 字节并始终返回完整所需长度，调用方应先查询长度再分配。该 seed 只允许
/// 在新建 Surface 时使用一次，不能作为 live 输出重播。
///
/// # Safety
/// `h` 有效且未 free；非空 `buf` 至少有 `buf_len` 字节。
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

/// 读取某 pane 的 viewport 滚动偏移（0 = 底部/最新）。
///
/// # Safety
/// `h` 有效且未 free。
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

/// 设置某 pane 的 viewport 滚动偏移（跳转历史后恢复）。
///
/// # Safety
/// `h` 有效且未 free。
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

/// 某 pane 还能往历史上滚的最大 offset（0 = 没有离屏历史；-1 = err）。
///
/// # Safety
/// `h` 有效且未 free。
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

/// 读取某 pane 的 OSC 133 命令刻度 JSON。
///
/// 返回 `{"ok":true,"marks":[{"seq":...,"command":"...",
/// "exit_code":...,"history_offset":...}]}`。
///
/// # Safety
/// `h` 有效且未 free；返回字符串由 `muxterm_free_string` 释放。
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

/// 读取某 pane 最新稳定行 ID（供“上次看到这里”和未读计数使用）。
///
/// # Safety
/// `h` 有效且未 free。
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

/// 搜索命中 seq 对应的 viewport 偏移（0 = 已在可见屏；-1 = seq 已淘汰或 err）。
///
/// # Safety
/// `h` 有效且未 free。
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

/// 读取某 pane 最近 n 行文本，返回 JSON 数组。
///
/// 返回 `{"ok": true, "lines": [...]}`。
///
/// # Safety
/// `h` 有效且未 free。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::state::{
        PaneAgentInfo, PaneAgentSession, PaneAgentSessionKind, PaneAgentStatus,
    };
    use crate::core::protocol::ffi::muxterm_set_callbacks;
    use crate::core::protocol::ffi::types::DIR_HORIZONTAL;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        let h = muxterm_new(c"local".as_ptr(), ptr::null(), ptr::null());
        assert!(!h.is_null());
        unsafe {
            // 旧 muxterm_new 已开一个 local shell 工作区。
            let list = muxterm_workspace_list(h);
            assert!(!list.is_null());
            let text = CStr::from_ptr(list).to_string_lossy().into_owned();
            muxterm_free_string(list);
            assert!(text.contains("workspaces"), "list 应含 workspaces: {text}");

            // 再开一个 tmux 工作区（无真实 tmux 时 open 失败，但不应 panic）。
            let rc = muxterm_workspace_open(
                h,
                c"local".as_ptr(),
                ptr::null(),
                c"demo".as_ptr(),
                c"tmux".as_ptr(),
                c"".as_ptr(),
                ptr::null(),
            );
            // 有 tmux 时 rc==0；无 tmux 时 rc==-1，两种情况都接受。
            let _ = rc;

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

        let _ = std::fs::remove_dir_all(dir);
    }

    /// C9：discover_sessions JSON 必须带 connect name（`target`），并接受 `all`。
    #[test]
    fn ffi_discover_sessions_json_includes_target_and_all() {
        let src = include_str!("api.rs");
        let start = src
            .find("pub unsafe extern \"C\" fn muxterm_discover_sessions_json")
            .expect("muxterm_discover_sessions_json 应存在");
        let rest = &src[start..];
        let end = rest
            .find("pub unsafe extern \"C\" fn muxterm_discover_workspaces_json")
            .or_else(|| rest.find("\n/// 通过 core 发现"))
            .unwrap_or(rest.len().min(2500));
        let body = &rest[..end];
        assert!(
            body.contains("\"target\""),
            "JSON 必须带 target=connect name，否则面板副标题只能是插件 id ssh: {body}"
        );
        assert!(
            body.contains("all"),
            "discover_sessions_json 必须把 transport=all 交给 Catalog: {body}"
        );
    }
}
