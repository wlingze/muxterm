//! TUI ↔ `protocol::ffi` C ABI 桥接。
//!
//! 不直接使用 TerminalModel / Backend trait；所有状态经 muxterm_* 导出函数。
//! 与 Linux `ffi_bridge` 同构，但不依赖 glib（TUI 自己在事件循环里 poll）。

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use crate::core::protocol::ffi::api::{
    muxterm_connect, muxterm_execute, muxterm_free, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_new, muxterm_new_connect, muxterm_poll_events,
    muxterm_resize_client, muxterm_resize_pane, muxterm_send_input, muxterm_shutdown,
    MuxtermHandle,
};
use crate::core::protocol::ffi::types::{
    CLayoutNode, CPane, CStateChange, CTab, CTask, LAYOUT_LEAF, LAYOUT_SPLIT_H, LAYOUT_SPLIT_V,
    STATE_BACKEND_STATUS, STATE_PANE_OUTPUT,
};

/// 从 FFI 拷贝出的事件。
#[derive(Debug, Clone)]
pub struct BridgeEvent {
    pub type_: u32,
    pub pane_id: u32,
    pub tab_id: u32,
    pub window_id: u32,
    pub data: Vec<u8>,
    pub name: String,
}

/// 布局树（owned）。
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

/// Tab 快照。
#[derive(Debug, Clone)]
pub struct BridgeTab {
    pub id: u32,
    pub name: String,
    pub is_active: bool,
}

/// Pane 快照。
#[derive(Debug, Clone)]
pub struct BridgePane {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub is_active: bool,
    pub title: String,
}

/// 一帧渲染所需的全部快照（纯数据，无 FFI 指针）。
#[derive(Debug, Clone, Default)]
pub struct FrameSnapshot {
    pub tabs: Vec<BridgeTab>,
    pub panes: Vec<BridgePane>,
    pub layout: Option<BridgeLayout>,
    /// pane_id → 累计输出
    pub outputs: HashMap<u32, Vec<u8>>,
    /// 状态栏文案（connected / error / …）
    pub status: String,
    pub active_tab: u32,
    pub active_pane: u32,
}

/// 核心 FFI 桥。
pub struct CoreBridge {
    handle: *mut MuxtermHandle,
    /// 最近一次 BackendStatus（pane_id 字段复用状态码）。
    last_status: u32,
    /// 当前后端类型（local / tmux / tmux-ssh / daemon），供前端判断 resize 策略。
    backend_type: String,
}

impl CoreBridge {
    /// 创建 handle 并 connect。
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
            anyhow::bail!("muxterm_new 失败");
        }
        let rc = unsafe { muxterm_connect(handle) };
        if rc != 0 {
            unsafe { muxterm_free(handle) };
            anyhow::bail!("muxterm_connect 失败: {rc}");
        }
        Ok(Self {
            handle,
            last_status: 2, // Connected
            backend_type: backend_type.to_string(),
        })
    }

    /// 用 `muxterm_new_connect` 一步建连（支持 SSH / attach / 起始目录）。
    pub fn new_connect(
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
            anyhow::bail!("muxterm_new_connect 失败");
        }
        Ok(Self {
            handle,
            last_status: 2, // Connected
            backend_type: backend_type.to_string(),
        })
    }

    pub fn execute(&self, task: CTask) -> i32 {
        unsafe { muxterm_execute(self.handle, &task) }
    }

    /// 当前后端类型。
    pub fn backend(&self) -> &str {
        &self.backend_type
    }

    pub fn poll_events(&mut self) -> Vec<BridgeEvent> {
        let mut buf = [CStateChange::default(); 64];
        let n = unsafe { muxterm_poll_events(self.handle, buf.as_mut_ptr(), 64) };
        if n <= 0 {
            return Vec::new();
        }
        buf[..n as usize]
            .iter()
            .map(|c| {
                if c.type_ == STATE_BACKEND_STATUS {
                    self.last_status = c.pane_id;
                }
                let data = if c.data.is_null() || c.data_len == 0 {
                    Vec::new()
                } else {
                    unsafe { std::slice::from_raw_parts(c.data, c.data_len).to_vec() }
                };
                BridgeEvent {
                    type_: c.type_,
                    pane_id: c.pane_id,
                    tab_id: c.tab_id,
                    window_id: c.window_id,
                    data,
                    name: cstr_to_string(c.name),
                }
            })
            .collect()
    }

    pub fn send_input(&self, pane_id: u32, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        unsafe { muxterm_send_input(self.handle, pane_id, data.as_ptr(), data.len()) }
    }

    /// 同步 tmux/daemon control client 的整体字符格尺寸。
    pub fn resize_client(&self, cols: u16, rows: u16) -> i32 {
        unsafe { muxterm_resize_client(self.handle, cols, rows) }
    }

    /// 同步本地 pane 的 pty 字符格尺寸。
    pub fn resize_pane(&self, pane_id: u32, cols: u16, rows: u16) -> i32 {
        unsafe { muxterm_resize_pane(self.handle, pane_id, cols, rows) }
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
                title: String::new(),
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

    /// 拉取完整渲染快照。
    pub fn snapshot(&self) -> FrameSnapshot {
        let tabs = self.get_tabs();
        let active_tab = tabs
            .iter()
            .find(|t| t.is_active)
            .map(|t| t.id)
            .or_else(|| tabs.first().map(|t| t.id))
            .unwrap_or(0);
        let panes = self.get_panes(active_tab);
        let active_pane = panes
            .iter()
            .find(|p| p.is_active)
            .map(|p| p.id)
            .or_else(|| panes.first().map(|p| p.id))
            .unwrap_or(0);
        let layout = self.get_layout(active_tab);
        let mut outputs = HashMap::new();
        for p in &panes {
            outputs.insert(p.id, self.get_pane_output(p.id));
        }
        // 也覆盖 layout 里可能有的 pane
        if let Some(ref lay) = layout {
            collect_layout_panes(lay, &mut |id| {
                outputs
                    .entry(id)
                    .or_insert_with(|| self.get_pane_output(id));
            });
        }
        FrameSnapshot {
            tabs,
            panes,
            layout,
            outputs,
            status: status_label(self.last_status).to_string(),
            active_tab,
            active_pane,
        }
    }

    pub fn is_pane_output(ev: &BridgeEvent) -> bool {
        ev.type_ == STATE_PANE_OUTPUT
    }
}

impl Drop for CoreBridge {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = muxterm_shutdown(self.handle);
                muxterm_free(self.handle);
            }
            self.handle = ptr::null_mut();
        }
    }
}

fn status_label(code: u32) -> &'static str {
    match code {
        0 => "disconnected",
        1 => "connecting",
        2 => "connected",
        3 => "error",
        4 => "exited",
        _ => "unknown",
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

fn collect_layout_panes(layout: &BridgeLayout, f: &mut dyn FnMut(u32)) {
    match layout {
        BridgeLayout::Leaf { pane_id } => f(*pane_id),
        BridgeLayout::Split { first, second, .. } => {
            collect_layout_panes(first, f);
            collect_layout_panes(second, f);
        }
    }
}

/// 构造常用 CTask。
pub mod tasks {
    use super::*;
    use crate::core::protocol::ffi::types::{
        DIR_HORIZONTAL, DIR_VERTICAL, TASK_CLOSE_PANE, TASK_CLOSE_TAB, TASK_NEW_TAB,
        TASK_NEXT_PANE, TASK_PREV_PANE, TASK_SPLIT_PANE, TASK_SWITCH_TAB,
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
}
