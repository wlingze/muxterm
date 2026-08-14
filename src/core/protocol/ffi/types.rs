//! C 友好类型（`#[repr(C)]`）。
//!
//! 与 `docs/ARCHITECTURE-PLAN.md` §1.5 对齐。

use std::os::raw::c_char;

// ── StateChange.type_ ──────────────────────────────────────
pub const STATE_PANE_OUTPUT: u32 = 0;
pub const STATE_TAB_ADDED: u32 = 1;
pub const STATE_TAB_CLOSED: u32 = 2;
pub const STATE_LAYOUT_CHANGED: u32 = 3;
pub const STATE_PANE_ADDED: u32 = 4;
pub const STATE_PANE_CLOSED: u32 = 5;
pub const STATE_ACTIVE_TAB_CHANGED: u32 = 6;
pub const STATE_ACTIVE_PANE_CHANGED: u32 = 7;
pub const STATE_TAB_RENAMED: u32 = 8;
pub const STATE_PANE_RESIZED: u32 = 9;
pub const STATE_BACKEND_STATUS: u32 = 10;
pub const STATE_STATUS_SUBSCRIPTION: u32 = 11;
pub const STATE_OTHER: u32 = 99;

// ── BackendStatus 编码到 CStateChange.pane_id ──────────────
pub const BACKEND_STATUS_DISCONNECTED: u32 = 0;
pub const BACKEND_STATUS_CONNECTING: u32 = 1;
pub const BACKEND_STATUS_CONNECTED: u32 = 2;
pub const BACKEND_STATUS_ERROR: u32 = 3;
pub const BACKEND_STATUS_EXITED: u32 = 4;

// ── Task.type_ ─────────────────────────────────────────────
pub const TASK_SPLIT_PANE: u32 = 0;
pub const TASK_NEW_TAB: u32 = 1;
pub const TASK_SWITCH_TAB: u32 = 2;
pub const TASK_CLOSE_PANE: u32 = 3;
pub const TASK_CLOSE_TAB: u32 = 4;
pub const TASK_NEXT_PANE: u32 = 5;
pub const TASK_PREV_PANE: u32 = 6;
pub const TASK_SHUTDOWN: u32 = 7;
pub const TASK_SWITCH_PANE: u32 = 8;
pub const TASK_DETACH: u32 = 9;
pub const TASK_TOGGLE_PANE_FULLSCREEN: u32 = 10;

// ── Split dir / layout node ────────────────────────────────
pub const DIR_HORIZONTAL: u32 = 0;
pub const DIR_VERTICAL: u32 = 1;

pub const LAYOUT_LEAF: u32 = 0;
pub const LAYOUT_SPLIT_H: u32 = 1;
pub const LAYOUT_SPLIT_V: u32 = 2;

/// 状态变更事件（核心 → 平台）。
///
/// `data` / `name` 指向由 [`crate::core::protocol::ffi::api::MuxtermHandle`] 持有的缓冲，
/// 在下一次 `muxterm_poll_events` / `muxterm_free` 前有效。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CStateChange {
    pub type_: u32,
    pub pane_id: u32,
    pub tab_id: u32,
    pub window_id: u32,
    pub data: *const u8,
    pub data_len: usize,
    pub name: *const c_char,
}

/// 平台 → 核心的任务描述。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CTask {
    pub type_: u32,
    pub target_pane: u32,
    pub target_tab: u32,
    pub dir: u32,
    pub name: *const c_char,
}

/// Tab 快照。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CTab {
    pub id: u32,
    pub name: *const c_char,
    pub is_active: u8,
}

/// Pane 快照。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CPane {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub is_active: u8,
}

/// 布局树节点（二叉）。
///
/// `first` / `second` 指向由 handle 持有的节点缓冲。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CLayoutNode {
    pub type_: u32,
    pub pane_id: u32,
    pub ratio: u32,
    pub first: *const CLayoutNode,
    pub second: *const CLayoutNode,
}

impl Default for CStateChange {
    fn default() -> Self {
        Self {
            type_: STATE_OTHER,
            pane_id: 0,
            tab_id: 0,
            window_id: 0,
            data: std::ptr::null(),
            data_len: 0,
            name: std::ptr::null(),
        }
    }
}
