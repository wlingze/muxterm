//! FFI Bridge：GTK 前端 ↔ `protocol::ffi` C ABI。
//!
//! 所有核心交互经此模块，不直接使用 TerminalModel / Backend trait。
//! GTK 主线程轮询事件；`MuxtermHandle` 生命周期由本结构体管理。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use gtk4::glib;

use crate::protocol::ffi::api::{
    muxterm_connect, muxterm_execute, muxterm_free, muxterm_get_layout, muxterm_get_pane_output,
    muxterm_get_panes, muxterm_get_tabs, muxterm_new, muxterm_poll_events, muxterm_send_input,
    MuxtermHandle,
};
use crate::protocol::ffi::types::{
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
            anyhow::bail!("muxterm_new 失败");
        }
        let rc = unsafe { muxterm_connect(handle) };
        if rc != 0 {
            unsafe { muxterm_free(handle) };
            anyhow::bail!("muxterm_connect 失败: {rc}");
        }
        Ok(Self {
            handle,
            poll_source: None,
        })
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

/// 构造常用 CTask 的便捷函数。
pub mod tasks {
    use super::*;
    use crate::protocol::ffi::types::{
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
