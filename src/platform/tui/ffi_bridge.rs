//! TUI ↔ `protocol::ffi` C ABI 桥接。
//!
//! 不直接使用 TerminalModel / Runtime trait；所有状态经 muxterm_* 导出函数。
//! 与 Linux `ffi_bridge` 同构，但不依赖 glib（TUI 自己在事件循环里 poll）。

use std::collections::HashMap;
use std::ptr;

use crate::ffi::{
    CTask, DIR_HORIZONTAL, DIR_VERTICAL, STATE_PANE_FRAME, STATE_PANE_OUTPUT, STATE_PANE_SNAPSHOT,
    TASK_CLOSE_PANE, TASK_CLOSE_TAB, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE, TASK_SPLIT_PANE,
    TASK_SWITCH_TAB,
};
use crate::platform::ffi_client::{ClientEvent, ClientLayout, ClientPane, ClientTab, FfiClient};

pub type BridgeEvent = ClientEvent;
pub type BridgeLayout = ClientLayout;
pub type BridgeTab = ClientTab;
pub type BridgePane = ClientPane;

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
    client: FfiClient,
    /// 当前后端类型（local / tmux / tmux-ssh / daemon），供前端判断 resize 策略。
    runtime_type: String,
}

impl CoreBridge {
    /// 创建 handle 并 connect。
    pub fn new(
        runtime_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
    ) -> anyhow::Result<Self> {
        let client = FfiClient::new(runtime_type, socket, session)?;
        Ok(Self {
            client,
            runtime_type: runtime_type.to_string(),
        })
    }

    /// 用 `muxterm_new_connect` 一步建连（支持 SSH / attach / 起始目录）。
    pub fn new_connect(
        runtime_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
        ssh_alias: Option<&str>,
        start_directory: Option<&str>,
    ) -> anyhow::Result<Self> {
        let client =
            FfiClient::new_connect(runtime_type, socket, session, ssh_alias, start_directory)?;
        Ok(Self {
            client,
            runtime_type: runtime_type.to_string(),
        })
    }

    pub fn execute(&self, task: CTask) -> i32 {
        self.client.execute(&task)
    }

    /// 显式分离 tmux/daemon client；不终止 tmux session 或 local daemon。
    pub fn detach(&self) -> i32 {
        self.client.detach()
    }

    /// 当前后端类型。
    pub fn runtime(&self) -> &str {
        &self.runtime_type
    }

    pub fn poll_events(&mut self) -> Vec<BridgeEvent> {
        self.client.poll_events()
    }

    pub fn send_input(&self, pane_id: u32, data: &[u8]) -> i32 {
        self.client.send_input(pane_id, data)
    }

    /// 同步 tmux/daemon control client 的整体字符格尺寸。
    pub fn resize_client(&self, cols: u16, rows: u16) -> i32 {
        self.client.resize_client(cols, rows)
    }

    /// 同步本地 pane 的 pty 字符格尺寸。
    pub fn resize_pane(&self, pane_id: u32, cols: u16, rows: u16) -> i32 {
        self.client.resize_pane(pane_id, cols, rows)
    }

    pub fn get_tabs(&self) -> Vec<BridgeTab> {
        self.client.get_tabs()
    }

    pub fn get_panes(&self, tab_id: u32) -> Vec<BridgePane> {
        self.client.get_panes(tab_id)
    }

    pub fn get_layout(&self, tab_id: u32) -> Option<BridgeLayout> {
        self.client.get_layout(tab_id)
    }

    pub fn get_pane_output(&self, pane_id: u32) -> Vec<u8> {
        self.client.get_pane_output(pane_id)
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
            status: status_label(self.client.status_code()).to_string(),
            active_tab,
            active_pane,
        }
    }

    pub fn is_pane_output(ev: &BridgeEvent) -> bool {
        ev.type_ == STATE_PANE_OUTPUT
    }

    pub fn is_pane_frame(ev: &BridgeEvent) -> bool {
        ev.type_ == STATE_PANE_FRAME
    }

    pub fn is_pane_snapshot(ev: &BridgeEvent) -> bool {
        ev.type_ == STATE_PANE_SNAPSHOT
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
