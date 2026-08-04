//! `#[no_mangle] extern "C"` 导出函数。
//!
//! 内部持有 [`TerminalModel`] + tokio runtime，对外全部同步。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use crate::core::model::layout::{LayoutNode, SplitDir};
use crate::core::model::state::StateChange;
use crate::core::model::task::Task;
use crate::core::model::terminal_model::TerminalModel;
use crate::core::runtime::{DaemonBackend, LocalBackend, TmuxBackend};
use crate::core::types::{PaneId, TabId, WindowId};
use crate::platform::cli::session::session_socket_path;

use super::callbacks::FfiCallbacks;
use super::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, DIR_VERTICAL, LAYOUT_LEAF, LAYOUT_SPLIT_H,
    LAYOUT_SPLIT_V, STATE_ACTIVE_PANE_CHANGED, STATE_ACTIVE_TAB_CHANGED, STATE_BACKEND_STATUS,
    STATE_LAYOUT_CHANGED, STATE_OTHER, STATE_PANE_ADDED, STATE_PANE_CLOSED, STATE_PANE_OUTPUT,
    STATE_PANE_RESIZED, STATE_TAB_ADDED, STATE_TAB_CLOSED, STATE_TAB_RENAMED, TASK_CLOSE_PANE,
    TASK_CLOSE_TAB, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE, TASK_SHUTDOWN, TASK_SPLIT_PANE,
    TASK_SWITCH_PANE, TASK_SWITCH_TAB,
};

/// FFI 句柄：TerminalModel + runtime + 供 C 侧借用的缓冲。
pub struct MuxtermHandle {
    pub(crate) model: TerminalModel,
    pub(crate) rt: tokio::runtime::Runtime,
    pub(crate) callbacks: FfiCallbacks,
    /// `poll_events` 产出的字节 / 字符串，保证指针在下次 poll 前有效。
    event_data: Vec<Vec<u8>>,
    event_names: Vec<CString>,
    /// `get_tabs` 名称缓冲。
    tab_names: Vec<CString>,
    /// `get_layout` 节点池（连续存储，指针稳定至下次 get_layout）。
    layout_nodes: Vec<CLayoutNode>,
}

impl MuxtermHandle {
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

/// 创建 handle。
///
/// `backend_type`：`"local"` / `"tmux"` / `"daemon"`（大小写不敏感）。
/// `socket` / `session`：tmux 模式可选；daemon 用 `session` 推导 socket 路径；local 忽略。
///
/// 失败返回 null。
#[no_mangle]
pub extern "C" fn muxterm_new(
    backend_type: *const c_char,
    socket: *const c_char,
    session: *const c_char,
) -> *mut MuxtermHandle {
    let kind = cstr_opt(backend_type)
        .unwrap_or_else(|| "local".into())
        .to_ascii_lowercase();
    let sock = cstr_opt(socket);
    let sess = cstr_opt(session);

    let backend: Box<dyn crate::core::model::Backend> = match kind.as_str() {
        "tmux" => {
            let sock_ref = sock.as_deref();
            if let Some(name) = sess.as_deref() {
                // 有 session：优先 attach；不存在时由 TmuxBackend 内部处理
                Box::new(TmuxBackend::new_with_attach(sock_ref, name))
            } else {
                Box::new(TmuxBackend::new(sock_ref))
            }
        }
        "ssh" => {
            let Some(alias) = sock.as_deref().filter(|value| !value.trim().is_empty()) else {
                return ptr::null_mut();
            };
            let Some(name) = sess.as_deref().filter(|value| !value.trim().is_empty()) else {
                return ptr::null_mut();
            };
            Box::new(TmuxBackend::new_with_ssh_attach(alias, name))
        }
        "daemon" => {
            // TUI × local：连已有 daemon（unix socket IPC）
            let name = sess.unwrap_or_else(|| "default".into());
            let path = sock
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| session_socket_path(&name));
            Box::new(DaemonBackend::new(path, name))
        }
        // 默认 local
        _ => Box::new(LocalBackend::new("$SHELL", "")),
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return ptr::null_mut(),
    };

    let handle = MuxtermHandle {
        model: TerminalModel::new(backend),
        rt,
        callbacks: FfiCallbacks::default(),
        event_data: Vec::new(),
        event_names: Vec::new(),
        tab_names: Vec::new(),
        layout_nodes: Vec::new(),
    };
    Box::into_raw(Box::new(handle))
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
    let mut handle = Box::from_raw(h);
    let _ = handle.rt.block_on(handle.model.shutdown());
}

/// 连接后端。0=ok，-1=err。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_connect(h: *mut MuxtermHandle) -> i32 {
    if h.is_null() {
        return -1;
    }
    let handle = &mut *h;
    match handle.rt.block_on(handle.model.connect()) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// 关闭后端。0=ok，-1=err。
///
/// # Safety
/// `h` 有效且未 free。
#[no_mangle]
pub unsafe extern "C" fn muxterm_shutdown(h: *mut MuxtermHandle) -> i32 {
    if h.is_null() {
        return -1;
    }
    let handle = &mut *h;
    match handle.rt.block_on(handle.model.shutdown()) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn ctask_to_task(task: &CTask, model: &TerminalModel) -> Option<Task> {
    let name = cstr_opt(task.name);
    match task.type_ {
        TASK_SPLIT_PANE => {
            let dir = if task.dir == DIR_VERTICAL {
                SplitDir::Vertical
            } else {
                SplitDir::Horizontal
            };
            let target = if task.target_pane == 0 {
                None
            } else {
                Some(PaneId(task.target_pane))
            };
            Some(Task::SplitPane {
                target,
                dir,
                command: None,
                workdir: None,
            })
        }
        TASK_NEW_TAB => {
            let window = model
                .state()
                .active_window()
                .map(|w| w.id)
                .unwrap_or(WindowId(0));
            Some(Task::NewTab {
                window,
                name,
                command: None,
                workdir: None,
            })
        }
        TASK_SWITCH_TAB => Some(Task::SwitchTab {
            target: TabId(task.target_tab),
        }),
        TASK_CLOSE_PANE => {
            let pane = if task.target_pane == 0 {
                model.active_pane_id()?
            } else {
                PaneId(task.target_pane)
            };
            Some(Task::ClosePane { target: pane })
        }
        TASK_CLOSE_TAB => Some(Task::CloseTab {
            target: TabId(task.target_tab),
        }),
        TASK_NEXT_PANE => Some(Task::NextPane),
        TASK_PREV_PANE => Some(Task::PrevPane),
        TASK_SWITCH_PANE => {
            let pane = if task.target_pane == 0 {
                model.active_pane_id()?
            } else {
                PaneId(task.target_pane)
            };
            Some(Task::SwitchPane { target: pane })
        }
        TASK_SHUTDOWN => Some(Task::Shutdown),
        _ => None,
    }
}

/// 执行一个 Task。0=ok，-1=err。
///
/// # Safety
/// `h` / `task` 有效。
#[no_mangle]
pub unsafe extern "C" fn muxterm_execute(h: *mut MuxtermHandle, task: *const CTask) -> i32 {
    if h.is_null() || task.is_null() {
        return -1;
    }
    let handle = &mut *h;
    let ctask = &*task;
    let Some(rust_task) = ctask_to_task(ctask, &handle.model) else {
        return -1;
    };
    match handle.model.execute(rust_task) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

fn state_change_to_c(handle: &mut MuxtermHandle, ev: &StateChange) -> CStateChange {
    let mut out = CStateChange::default();
    match ev {
        StateChange::PaneOutput { pane, data } => {
            out.type_ = STATE_PANE_OUTPUT;
            out.pane_id = pane.0;
            let (p, n) = handle.push_data(data);
            out.data = p;
            out.data_len = n;
        }
        StateChange::TabAdded { tab, window } => {
            out.type_ = STATE_TAB_ADDED;
            out.tab_id = tab.0;
            out.window_id = window.0;
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
        StateChange::ActiveTabChanged { window, tab } => {
            out.type_ = STATE_ACTIVE_TAB_CHANGED;
            out.window_id = window.0;
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
        StateChange::BackendStatusChanged(status) => {
            out.type_ = STATE_BACKEND_STATUS;
            out.pane_id = match status {
                crate::core::model::state::BackendStatus::Disconnected => 0,
                crate::core::model::state::BackendStatus::Connecting => 1,
                crate::core::model::state::BackendStatus::Connected => 2,
                crate::core::model::state::BackendStatus::Error => 3,
                crate::core::model::state::BackendStatus::Exited => 4,
            };
        }
        StateChange::PaneTitleChanged { pane, title } => {
            out.type_ = STATE_OTHER;
            out.pane_id = pane.0;
            out.name = handle.push_name(title);
        }
        _ => {
            out.type_ = STATE_OTHER;
        }
    }
    out
}

/// 非阻塞拉取事件，写入 `out[0..]`，返回写入数量（或 -1）。
///
/// 会先 `refresh()` 拉取 backend 增量（含 pty 输出）。
///
/// # Safety
/// `out` 至少 `max_count` 个元素；返回的指针在下次 poll/free 前有效。
#[no_mangle]
pub unsafe extern "C" fn muxterm_poll_events(
    h: *mut MuxtermHandle,
    out: *mut CStateChange,
    max_count: i32,
) -> i32 {
    if h.is_null() || out.is_null() || max_count <= 0 {
        return -1;
    }
    let handle = &mut *h;
    handle.clear_event_bufs();
    let events = handle.model.refresh();
    let n = events.len().min(max_count as usize);
    let slice = std::slice::from_raw_parts_mut(out, n);
    for (i, ev) in events.iter().take(n).enumerate() {
        let c = state_change_to_c(handle, ev);
        // 回调
        if let StateChange::PaneOutput { pane, data } = ev {
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
    if h.is_null() || data.is_null() {
        return -1;
    }
    let handle = &mut *h;
    let bytes = std::slice::from_raw_parts(data, len).to_vec();
    let pane = if pane_id == 0 {
        match handle.model.active_pane_id() {
            Some(p) => p,
            None => return -1,
        }
    } else {
        PaneId(pane_id)
    };
    match handle.model.execute(Task::WriteRaw {
        target: pane,
        data: bytes,
    }) {
        Ok(_) => 0,
        Err(_) => -1,
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
    if h.is_null() || cols == 0 || rows == 0 {
        return -1;
    }
    let handle = &mut *h;
    let pane = if pane_id == 0 {
        match handle.model.active_pane_id() {
            Some(p) => p,
            None => return -1,
        }
    } else {
        PaneId(pane_id)
    };
    match handle.model.execute(Task::ResizePane {
        target: pane,
        cols,
        rows,
    }) {
        Ok(_) => 0,
        Err(_) => -1,
    }
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
    let window = match handle.model.state().active_window() {
        Some(w) => w.id,
        None => return 0,
    };
    let tabs = handle.model.state().tabs(&window);
    let n = tabs.len().min(max_count as usize);
    let slice = std::slice::from_raw_parts_mut(out, n);
    for (i, t) in tabs.iter().take(n).enumerate() {
        let name_ptr = match CString::new(t.name.as_str()) {
            Ok(cs) => {
                handle.tab_names.push(cs);
                handle.tab_names.last().unwrap().as_ptr()
            }
            Err(_) => ptr::null(),
        };
        slice[i] = CTab {
            id: t.id.0,
            name: name_ptr,
            is_active: u8::from(t.active),
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
    let tid = if tab_id == 0 {
        match handle.model.state().active_tab() {
            Some(t) => t.id,
            None => return 0,
        }
    } else {
        TabId(tab_id)
    };
    let panes = handle.model.state().panes(&tid);
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
/// # Safety
/// `buf` 至少 `buf_len` 字节。
#[no_mangle]
pub unsafe extern "C" fn muxterm_get_pane_output(
    h: *mut MuxtermHandle,
    pane_id: u32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if h.is_null() || buf.is_null() {
        return -1;
    }
    let handle = &*h;
    let pane = if pane_id == 0 {
        match handle.model.active_pane_id() {
            Some(p) => p,
            None => return -1,
        }
    } else {
        PaneId(pane_id)
    };
    let Some(out) = handle.model.state().pane_output(&pane) else {
        return 0;
    };
    let n = out.len().min(buf_len);
    std::ptr::copy_nonoverlapping(out.as_ptr(), buf, n);
    n as i32
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
    let tid = if tab_id == 0 {
        match handle.model.state().active_tab() {
            Some(t) => t.id,
            None => return -1,
        }
    } else {
        TabId(tab_id)
    };
    let Some(tl) = handle.model.state().layout(&tid) else {
        return -1;
    };
    let root_idx = push_layout_node(&mut handle.layout_nodes, &tl.tree);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::ffi::muxterm_set_callbacks;
    use crate::core::protocol::ffi::types::DIR_HORIZONTAL;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn ffi_null_handle_safe() {
        unsafe {
            assert_eq!(muxterm_connect(ptr::null_mut()), -1);
            assert_eq!(muxterm_shutdown(ptr::null_mut()), -1);
            assert_eq!(muxterm_execute(ptr::null_mut(), ptr::null()), -1);
            assert_eq!(muxterm_poll_events(ptr::null_mut(), ptr::null_mut(), 1), -1);
            muxterm_free(ptr::null_mut());
        }
    }
}
