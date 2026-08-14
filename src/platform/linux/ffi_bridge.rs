//! FFI Bridge：GTK 前端 ↔ `protocol::ffi` C ABI。
//!
//! 所有核心交互经此模块，不直接使用 TerminalModel / Backend trait。
//! GTK 主线程轮询事件；`MuxtermHandle` 生命周期由本结构体管理。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use gtk4::glib;

use crate::core::protocol::ffi::api::{
    muxterm_connect, muxterm_create_tmux_session_json, muxterm_detach,
    muxterm_discover_ssh_hosts_json, muxterm_discover_tmux_sessions_json, muxterm_execute,
    muxterm_free, muxterm_free_string, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_list_dir_json, muxterm_new, muxterm_new_connect,
    muxterm_poll_events, muxterm_report_all_pane_colours, muxterm_report_pane_colours,
    muxterm_resize_client, muxterm_resize_pane, muxterm_send_input, muxterm_status_snapshot_json,
    MuxtermHandle,
};
use crate::core::protocol::ffi::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, LAYOUT_LEAF, LAYOUT_SPLIT_H, LAYOUT_SPLIT_V,
    STATE_PANE_OUTPUT,
};

/// 从 FFI 拷贝出的、可在 Rust 侧安全持有的事件。
#[derive(Debug, Clone)]
pub struct BridgeEvent {
    pub type_: u32,
    pub pane_id: u32,
    pub tab_id: u32,
    pub window_id: u32,
    pub data: Vec<u8>,
    pub name: String,
}

/// 布局树（owned，从 CLayoutNode 深拷贝）。
#[derive(Debug, Clone)]
pub enum BridgeLayout {
    Leaf {
        pane_id: u32,
    },
    Split {
        horizontal: bool,
        ratio: u32,
        first: Box<BridgeLayout>,
        second: Box<BridgeLayout>,
    },
}

/// Tab 快照（owned）。
#[derive(Debug, Clone)]
pub struct BridgeTab {
    pub id: u32,
    pub name: String,
    pub is_active: bool,
}

/// Pane 快照（owned）。
#[derive(Debug, Clone)]
pub struct BridgePane {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub is_active: bool,
}

/// 核心 FFI 桥。
///
/// **非线程安全**：仅在 GTK 主线程使用（内含 `*mut`，自动 !Send/!Sync）。
/// 事件轮询用 `glib::timeout_add_local` 挂在 GTK 主循环上（见 [`Self::start_polling`]）。
pub struct CoreBridge {
    handle: *mut MuxtermHandle,
    /// 当前连接的后端类型（local / tmux / tmux-ssh / daemon）。
    pub backend_type: String,
    /// tmux `-L` socket 名（可选）。
    pub socket: Option<String>,
    /// tmux session 名（可选）。
    pub session: Option<String>,
    /// SSH `~/.ssh/config` alias（可选；用于 status 快照的只读查询）。
    pub ssh_alias: Option<String>,
    /// 轮询定时器；`start_polling` 设置，Drop / `stop_polling` 清除。
    poll_source: Option<glib::SourceId>,
}

impl CoreBridge {
    /// 创建并 `connect`。失败返回 Err。
    pub fn new(
        backend_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
    ) -> anyhow::Result<Self> {
        let bt = CString::new(backend_type).unwrap_or_default();
        let sock_c = socket.and_then(|s| CString::new(s).ok());
        let sess_c = session.and_then(|s| CString::new(s).ok());
        let sock_ptr = sock_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null());
        let sess_ptr = sess_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null());

        let handle = muxterm_new(bt.as_ptr(), sock_ptr, sess_ptr);
        if handle.is_null() {
            anyhow::bail!(crate::platform::i18n::tr(
                crate::platform::i18n::Key::ErrorBridgeCreate
            ));
        }
        let rc = unsafe { muxterm_connect(handle) };
        if rc != 0 {
            unsafe { muxterm_free(handle) };
            anyhow::bail!(crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::ErrorBridgeConnect,
                &[("code", &rc.to_string())],
            ));
        }
        let backend_type = backend_type.to_ascii_lowercase();
        Ok(Self {
            handle,
            backend_type,
            socket: socket.map(|s| s.to_string()),
            session: session.map(|s| s.to_string()),
            ssh_alias: None,
            poll_source: None,
        })
    }

    /// 一步建连：支持 SSH（`tmux-ssh` + alias）、attach 与指定起始目录。
    ///
    /// 与 macOS `CoreBridge.connect` 语义一致：`muxterm_new_connect` 内部
    /// 已完成 `connect`；这里再调一次 `muxterm_connect`（幂等）以便统一
    /// 错误路径。
    pub fn connect(
        backend_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
        ssh_alias: Option<&str>,
        start_directory: Option<&str>,
    ) -> anyhow::Result<Self> {
        let bt = CString::new(backend_type).unwrap_or_default();
        let sock_c = socket.and_then(|s| CString::new(s).ok());
        let sess_c = session.and_then(|s| CString::new(s).ok());
        let alias_c = ssh_alias.and_then(|s| CString::new(s).ok());
        let dir_c = start_directory.and_then(|s| CString::new(s).ok());
        let sock_ptr = sock_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null());
        let sess_ptr = sess_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null());
        let alias_ptr = alias_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null());
        let dir_ptr = dir_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null());

        let handle = muxterm_new_connect(bt.as_ptr(), sock_ptr, sess_ptr, alias_ptr, dir_ptr);
        if handle.is_null() {
            anyhow::bail!(crate::platform::i18n::tr(
                crate::platform::i18n::Key::ErrorBridgeCreate
            ));
        }
        let rc = unsafe { muxterm_connect(handle) };
        if rc != 0 {
            unsafe { muxterm_free(handle) };
            anyhow::bail!(crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::ErrorBridgeConnect,
                &[("code", &rc.to_string())],
            ));
        }
        let normalized = backend_type.to_ascii_lowercase();
        // QuickConnect 面板统一用 backend_type="tmux-ssh" + alias 建连，
        // 但旧代码/配置也可能传 "ssh"：这里归一化记录。
        let (backend_type, ssh_alias) = if normalized == "ssh" {
            ("tmux-ssh".to_string(), socket.map(|s| s.to_string()))
        } else {
            (normalized, ssh_alias.map(|s| s.to_string()))
        };
        Ok(Self {
            handle,
            backend_type,
            socket: socket.map(|s| s.to_string()),
            session: session.map(|s| s.to_string()),
            ssh_alias,
            poll_source: None,
        })
    }

    /// 当前连接是否由 tmux/SSH 控制 client 管理尺寸与状态栏。
    pub fn uses_tmux(&self) -> bool {
        matches!(self.backend_type.as_str(), "tmux" | "ssh" | "tmux-ssh")
    }

    /// 在 GTK 主循环上启动周期轮询（默认由 window 以 16ms 调用）。
    ///
    /// `on_tick` 返回 `false` 时停止定时器。已有定时器会先被替换。
    pub fn start_polling<F>(&mut self, interval_ms: u64, mut on_tick: F)
    where
        F: FnMut() -> bool + 'static,
    {
        self.stop_polling();
        let id =
            glib::timeout_add_local(std::time::Duration::from_millis(interval_ms), move || {
                if on_tick() {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        self.poll_source = Some(id);
    }

    /// 停止事件轮询定时器。
    pub fn stop_polling(&mut self) {
        if let Some(id) = self.poll_source.take() {
            id.remove();
        }
    }

    pub fn execute(&self, task: CTask) -> i32 {
        unsafe { muxterm_execute(self.handle, &task) }
    }

    /// 显式分离 tmux/SSH control client；session 由 tmux server 保留。
    pub fn detach(&self) -> i32 {
        unsafe { muxterm_detach(self.handle) }
    }

    /// 轮询事件；立刻拷贝 data/name，避免下次 poll 失效。
    pub fn poll_events(&self) -> Vec<BridgeEvent> {
        let mut buf = [CStateChange::default(); 64];
        let n = unsafe { muxterm_poll_events(self.handle, buf.as_mut_ptr(), 64) };
        if n <= 0 {
            return Vec::new();
        }
        buf[..n as usize]
            .iter()
            .map(|c| {
                let data = if c.data.is_null() || c.data_len == 0 {
                    Vec::new()
                } else {
                    unsafe { std::slice::from_raw_parts(c.data, c.data_len).to_vec() }
                };
                let name = cstr_to_string(c.name);
                BridgeEvent {
                    type_: c.type_,
                    pane_id: c.pane_id,
                    tab_id: c.tab_id,
                    window_id: c.window_id,
                    data,
                    name,
                }
            })
            .collect()
    }

    pub fn get_tabs(&self) -> Vec<BridgeTab> {
        let mut buf = [CTab {
            id: 0,
            name: ptr::null(),
            is_active: 0,
        }; 32];
        let n = unsafe { muxterm_get_tabs(self.handle, buf.as_mut_ptr(), 32) };
        if n <= 0 {
            return Vec::new();
        }
        buf[..n as usize]
            .iter()
            .map(|t| BridgeTab {
                id: t.id,
                name: cstr_to_string(t.name),
                is_active: t.is_active != 0,
            })
            .collect()
    }

    pub fn get_panes(&self, tab_id: u32) -> Vec<BridgePane> {
        let mut buf = [CPane {
            id: 0,
            cols: 0,
            rows: 0,
            is_active: 0,
        }; 64];
        let n = unsafe { muxterm_get_panes(self.handle, tab_id, buf.as_mut_ptr(), 64) };
        if n <= 0 {
            return Vec::new();
        }
        buf[..n as usize]
            .iter()
            .map(|p| BridgePane {
                id: p.id,
                cols: p.cols,
                rows: p.rows,
                is_active: p.is_active != 0,
            })
            .collect()
    }

    pub fn get_layout(&self, tab_id: u32) -> Option<BridgeLayout> {
        let mut root = CLayoutNode {
            type_: LAYOUT_LEAF,
            pane_id: 0,
            ratio: 0,
            first: ptr::null(),
            second: ptr::null(),
        };
        let rc = unsafe { muxterm_get_layout(self.handle, tab_id, &mut root) };
        if rc != 0 {
            return None;
        }
        Some(unsafe { clone_layout(&root) })
    }

    pub fn get_pane_output(&self, pane_id: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 256 * 1024];
        let n =
            unsafe { muxterm_get_pane_output(self.handle, pane_id, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize);
        buf
    }

    pub fn send_input(&self, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        unsafe { muxterm_send_input(self.handle, pane_id, data.as_ptr(), data.len()) }
    }

    /// 同步 tmux/SSH control client 的整体字符格尺寸。
    pub fn resize_client(&self, cols: u16, rows: u16) -> i32 {
        unsafe { muxterm_resize_client(self.handle, cols, rows) }
    }

    /// 同步本地 pane 的 pty 字符格尺寸。
    pub fn resize_pane(&self, pane_id: u32, cols: u16, rows: u16) -> i32 {
        unsafe { muxterm_resize_pane(self.handle, pane_id, cols, rows) }
    }

    /// 上报单个 pane 的前景/背景色（`refresh-client -r`），供 tmux 代答 OSC 10/11。
    pub fn report_pane_colours(&self, pane_id: u32, fg_hex: &str, bg_hex: &str) -> i32 {
        let fg_c = CString::new(fg_hex).unwrap_or_default();
        let bg_c = CString::new(bg_hex).unwrap_or_default();
        unsafe { muxterm_report_pane_colours(self.handle, pane_id, fg_c.as_ptr(), bg_c.as_ptr()) }
    }

    /// 上报所有 pane 的颜色（主题切换后必须整段对齐）。
    pub fn report_all_pane_colours(&self, fg_hex: &str, bg_hex: &str) -> i32 {
        let fg_c = CString::new(fg_hex).unwrap_or_default();
        let bg_c = CString::new(bg_hex).unwrap_or_default();
        unsafe { muxterm_report_all_pane_colours(self.handle, fg_c.as_ptr(), bg_c.as_ptr()) }
    }

    /// 抓取 status bar 快照（只读查询，tmux 兼容），返回解析后的快照。
    pub fn status_snapshot(
        &self,
    ) -> Option<crate::platform::linux::quickconnect::status_style::StatusBarSnapshot> {
        if !self.uses_tmux() {
            return None;
        }
        // SSH：backend 归一化为 tmux-ssh，FFI 期望 backend_type="ssh"
        let ffi_backend = if self.backend_type == "tmux-ssh" {
            "ssh"
        } else {
            self.backend_type.as_str()
        };
        let bt_c = CString::new(ffi_backend).unwrap_or_default();
        let alias_c = self
            .ssh_alias
            .as_ref()
            .and_then(|s| CString::new(s.clone()).ok());
        let sock_c = self
            .socket
            .as_ref()
            .and_then(|s| CString::new(s.clone()).ok());
        let sess_c = self
            .session
            .as_ref()
            .and_then(|s| CString::new(s.clone()).ok());
        let Some(sess) = &sess_c else {
            return None;
        };
        let raw = muxterm_status_snapshot_json(
            bt_c.as_ptr(),
            alias_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
            sock_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
            sess.as_ptr(),
        );
        if raw.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe { muxterm_free_string(raw) };
        #[derive(serde::Deserialize)]
        struct Response {
            ok: bool,
            status: Option<crate::platform::linux::quickconnect::status_style::StatusBarSnapshot>,
        }
        let response: Response = serde_json::from_str(&text).ok()?;
        if !response.ok {
            return None;
        }
        response.status
    }

    pub fn is_pane_output(ev: &BridgeEvent) -> bool {
        ev.type_ == STATE_PANE_OUTPUT
    }
}

impl Drop for CoreBridge {
    fn drop(&mut self) {
        self.stop_polling();
        if !self.handle.is_null() {
            // `muxterm_free` 内部会再调一次 shutdown；勿先 shutdown 再 free，
            // 避免后端二次清理导致堆损坏（集成测试里表现为后续 VTE 分配 double free）。
            unsafe {
                muxterm_free(self.handle);
            }
            self.handle = ptr::null_mut();
        }
    }
}

fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe fn clone_layout(node: &CLayoutNode) -> BridgeLayout {
    match node.type_ {
        LAYOUT_SPLIT_H | LAYOUT_SPLIT_V => {
            let first = if node.first.is_null() {
                BridgeLayout::Leaf { pane_id: 0 }
            } else {
                clone_layout(&*node.first)
            };
            let second = if node.second.is_null() {
                BridgeLayout::Leaf { pane_id: 0 }
            } else {
                clone_layout(&*node.second)
            };
            BridgeLayout::Split {
                horizontal: node.type_ == LAYOUT_SPLIT_H,
                ratio: node.ratio,
                first: Box::new(first),
                second: Box::new(second),
            }
        }
        _ => BridgeLayout::Leaf {
            pane_id: node.pane_id,
        },
    }
}

// ============================================================================
// 无状态 discovery（本地 / SSH 目录、tmux session、SSH host）
// ============================================================================

/// core SSH host discovery 返回的条目。
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct SshHostEntry {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
}

/// core tmux discovery 返回的 session 摘要。
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct TmuxSessionEntry {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
    pub created: u64,
}

/// core 目录列表返回的条目。
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
}

/// 运行一次返回 JSON 的 discovery FFI，并解析为 serde 值。
///
/// 调用方负责在非 GTK 线程执行（SSH 查询可能阻塞数秒）。
fn discovery_json<F>(call: F) -> anyhow::Result<serde_json::Value>
where
    F: FnOnce() -> *mut c_char,
{
    let raw = call();
    if raw.is_null() {
        anyhow::bail!(crate::platform::i18n::tr(
            crate::platform::i18n::Key::ErrorCoreDiscoveryNoResponse
        ));
    }
    let text = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    unsafe { muxterm_free_string(raw) };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        anyhow::anyhow!(crate::platform::i18n::tr_args(
            crate::platform::i18n::Key::ErrorCoreDiscoveryInvalidJson,
            &[("error", &e.to_string())],
        ))
    })?;
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let error = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        anyhow::bail!(error.to_string());
    }
    Ok(value)
}

fn cstr_pair(value: Option<&str>) -> Option<CString> {
    value.and_then(|s| CString::new(s).ok())
}

impl CoreBridge {
    /// 发现用户 SSH 配置中的 Host alias。
    pub fn discover_ssh_hosts() -> anyhow::Result<Vec<SshHostEntry>> {
        let value = discovery_json(|| muxterm_discover_ssh_hosts_json(ptr::null()))?;
        let hosts: Vec<SshHostEntry> = serde_json::from_value(value["hosts"].clone())?;
        Ok(hosts)
    }

    /// 列出本地或远端目录条目。
    pub fn list_dir(
        backend_type: &str,
        target: Option<&str>,
        path: &str,
    ) -> anyhow::Result<Vec<FsEntry>> {
        let bt_c = CString::new(backend_type).unwrap_or_default();
        let target_c = cstr_pair(target);
        let path_c = CString::new(path).unwrap_or_default();
        let value = discovery_json(|| {
            muxterm_list_dir_json(
                bt_c.as_ptr(),
                target_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
                ptr::null(),
                path_c.as_ptr(),
                10_000,
            )
        })?;
        let entries: Vec<FsEntry> = serde_json::from_value(value["entries"].clone())?;
        Ok(entries)
    }

    /// 发现本地或 SSH tmux session。
    pub fn discover_tmux_sessions(
        backend_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
    ) -> anyhow::Result<Vec<TmuxSessionEntry>> {
        let bt_c = CString::new(backend_type).unwrap_or_default();
        let target_c = cstr_pair(target);
        let sock_c = cstr_pair(socket);
        let value = discovery_json(|| {
            muxterm_discover_tmux_sessions_json(
                bt_c.as_ptr(),
                target_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
                sock_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
                ptr::null(),
                10_000,
            )
        })?;
        let sessions: Vec<TmuxSessionEntry> = serde_json::from_value(value["sessions"].clone())?;
        Ok(sessions)
    }

    /// 创建 detached tmux session（Project attach→create fallback 用）。
    pub fn create_tmux_session(
        backend_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
        session: &str,
        directory: &str,
    ) -> anyhow::Result<String> {
        let bt_c = CString::new(backend_type).unwrap_or_default();
        let target_c = cstr_pair(target);
        let sock_c = cstr_pair(socket);
        let sess_c = CString::new(session).unwrap_or_default();
        let dir_c = CString::new(directory).unwrap_or_default();
        let value = discovery_json(|| {
            muxterm_create_tmux_session_json(
                bt_c.as_ptr(),
                target_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
                sock_c.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null()),
                ptr::null(),
                sess_c.as_ptr(),
                dir_c.as_ptr(),
                10_000,
            )
        })?;
        Ok(value["session"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| session.to_string()))
    }
}

/// 构造常用 CTask 的便捷函数。
pub mod tasks {
    use super::*;
    use crate::core::protocol::ffi::types::{
        DIR_HORIZONTAL, DIR_VERTICAL, TASK_CLOSE_PANE, TASK_CLOSE_TAB, TASK_DETACH, TASK_NEW_TAB,
        TASK_NEXT_PANE, TASK_PREV_PANE, TASK_SPLIT_PANE, TASK_SWITCH_PANE, TASK_SWITCH_TAB,
        TASK_TOGGLE_PANE_FULLSCREEN,
    };

    pub fn split_h(target_pane: u32) -> CTask {
        CTask {
            type_: TASK_SPLIT_PANE,
            target_pane,
            target_tab: 0,
            dir: DIR_HORIZONTAL,
            name: ptr::null(),
        }
    }

    pub fn split_v(target_pane: u32) -> CTask {
        CTask {
            type_: TASK_SPLIT_PANE,
            target_pane,
            target_tab: 0,
            dir: DIR_VERTICAL,
            name: ptr::null(),
        }
    }

    pub fn new_tab() -> CTask {
        CTask {
            type_: TASK_NEW_TAB,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn switch_tab(tab_id: u32) -> CTask {
        CTask {
            type_: TASK_SWITCH_TAB,
            target_pane: 0,
            target_tab: tab_id,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn close_pane(pane_id: u32) -> CTask {
        CTask {
            type_: TASK_CLOSE_PANE,
            target_pane: pane_id,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn close_tab(tab_id: u32) -> CTask {
        CTask {
            type_: TASK_CLOSE_TAB,
            target_pane: 0,
            target_tab: tab_id,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn next_pane() -> CTask {
        CTask {
            type_: TASK_NEXT_PANE,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn prev_pane() -> CTask {
        CTask {
            type_: TASK_PREV_PANE,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn switch_pane(pane_id: u32) -> CTask {
        CTask {
            type_: TASK_SWITCH_PANE,
            target_pane: pane_id,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn detach() -> CTask {
        CTask {
            type_: TASK_DETACH,
            target_pane: 0,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }

    pub fn toggle_pane_fullscreen(pane_id: u32) -> CTask {
        CTask {
            type_: TASK_TOGGLE_PANE_FULLSCREEN,
            target_pane: pane_id,
            target_tab: 0,
            dir: 0,
            name: ptr::null(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::ffi::types::{
        DIR_HORIZONTAL, DIR_VERTICAL, STATE_PANE_OUTPUT, TASK_CLOSE_PANE, TASK_CLOSE_TAB,
        TASK_DETACH, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE, TASK_SPLIT_PANE,
        TASK_SWITCH_PANE, TASK_SWITCH_TAB, TASK_TOGGLE_PANE_FULLSCREEN,
    };

    #[test]
    fn task_builders_set_expected_type_and_targets() {
        let split_h = tasks::split_h(7);
        assert_eq!(split_h.type_, TASK_SPLIT_PANE);
        assert_eq!(split_h.target_pane, 7);
        assert_eq!(split_h.dir, DIR_HORIZONTAL);

        let split_v = tasks::split_v(8);
        assert_eq!(split_v.type_, TASK_SPLIT_PANE);
        assert_eq!(split_v.target_pane, 8);
        assert_eq!(split_v.dir, DIR_VERTICAL);

        let new_tab = tasks::new_tab();
        assert_eq!(new_tab.type_, TASK_NEW_TAB);

        let switch_tab = tasks::switch_tab(3);
        assert_eq!(switch_tab.type_, TASK_SWITCH_TAB);
        assert_eq!(switch_tab.target_tab, 3);

        let close_pane = tasks::close_pane(9);
        assert_eq!(close_pane.type_, TASK_CLOSE_PANE);
        assert_eq!(close_pane.target_pane, 9);

        let close_tab = tasks::close_tab(4);
        assert_eq!(close_tab.type_, TASK_CLOSE_TAB);
        assert_eq!(close_tab.target_tab, 4);

        assert_eq!(tasks::next_pane().type_, TASK_NEXT_PANE);
        assert_eq!(tasks::prev_pane().type_, TASK_PREV_PANE);

        let switch_pane = tasks::switch_pane(11);
        assert_eq!(switch_pane.type_, TASK_SWITCH_PANE);
        assert_eq!(switch_pane.target_pane, 11);

        assert_eq!(tasks::detach().type_, TASK_DETACH);

        let fullscreen = tasks::toggle_pane_fullscreen(12);
        assert_eq!(fullscreen.type_, TASK_TOGGLE_PANE_FULLSCREEN);
        assert_eq!(fullscreen.target_pane, 12);
    }

    #[test]
    fn is_pane_output_matches_state_type() {
        let ev = BridgeEvent {
            type_: STATE_PANE_OUTPUT,
            pane_id: 1,
            tab_id: 0,
            window_id: 0,
            data: b"x".to_vec(),
            name: String::new(),
        };
        assert!(CoreBridge::is_pane_output(&ev));
        let other = BridgeEvent {
            type_: STATE_PANE_OUTPUT + 1,
            ..ev
        };
        assert!(!CoreBridge::is_pane_output(&other));
    }
}
