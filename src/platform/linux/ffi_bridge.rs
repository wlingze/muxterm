//! FFI Bridge：GTK 前端 ↔ `protocol::ffi` C ABI。
//!
//! 所有核心交互经此模块，不直接使用 TerminalModel / Runtime trait。
//! GTK 主线程轮询事件；`MuxtermHandle` 生命周期由本结构体管理。

use std::ptr;

use gtk4::glib;

use crate::ffi::{
    CTask, DIR_HORIZONTAL, DIR_VERTICAL, STATE_PANE_FRAME, STATE_PANE_OUTPUT, STATE_PANE_SNAPSHOT,
    TASK_CLOSE_PANE, TASK_CLOSE_TAB, TASK_DETACH, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE,
    TASK_SPLIT_PANE, TASK_SWITCH_PANE, TASK_SWITCH_TAB, TASK_TOGGLE_PANE_FULLSCREEN,
};
use crate::platform::ffi_client::{ClientEvent, ClientLayout, ClientPane, ClientTab, FfiClient};

pub type BridgeEvent = ClientEvent;
pub type BridgeLayout = ClientLayout;
pub type BridgeTab = ClientTab;
pub type BridgePane = ClientPane;
pub use crate::platform::ffi_client::{FsEntry, SshHostEntry, WorkspaceCandidate};

/// 核心 FFI 桥。
///
/// **非线程安全**：仅在 GTK 主线程使用（内含 `*mut`，自动 !Send/!Sync）。
/// 事件轮询用 `glib::timeout_add_local` 挂在 GTK 主循环上（见 [`Self::start_polling`]）。
pub struct CoreBridge {
    client: FfiClient,
    /// 当前连接的后端类型（local / tmux / tmux-ssh / daemon）。
    pub runtime_type: String,
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
        runtime_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
    ) -> anyhow::Result<Self> {
        let client = FfiClient::new(runtime_type, socket, session).map_err(|error| {
            anyhow::anyhow!(
                "{}: {error}",
                crate::platform::i18n::tr(crate::platform::i18n::Key::ErrorBridgeConnect)
            )
        })?;
        let runtime_type = runtime_type.to_ascii_lowercase();
        Ok(Self {
            client,
            runtime_type,
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
        runtime_type: &str,
        socket: Option<&str>,
        session: Option<&str>,
        ssh_alias: Option<&str>,
        start_directory: Option<&str>,
    ) -> anyhow::Result<Self> {
        let client =
            FfiClient::new_connect(runtime_type, socket, session, ssh_alias, start_directory)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "{}: {error}",
                        crate::platform::i18n::tr(crate::platform::i18n::Key::ErrorBridgeConnect)
                    )
                })?;
        let normalized = runtime_type.to_ascii_lowercase();
        // QuickConnect 面板统一用 runtime_type="tmux-ssh" + alias 建连，
        // 但旧代码/配置也可能传 "ssh"：这里归一化记录。
        let (runtime_type, ssh_alias) = if normalized == "ssh" {
            ("tmux-ssh".to_string(), socket.map(|s| s.to_string()))
        } else {
            (normalized, ssh_alias.map(|s| s.to_string()))
        };
        Ok(Self {
            client,
            runtime_type,
            socket: socket.map(|s| s.to_string()),
            session: session.map(|s| s.to_string()),
            ssh_alias,
            poll_source: None,
        })
    }

    /// 当前连接是否由 tmux/SSH 控制 client 管理尺寸与状态栏。
    pub fn uses_tmux(&self) -> bool {
        matches!(self.runtime_type.as_str(), "tmux" | "ssh" | "tmux-ssh")
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

    /// tmux status bar 订阅是否已生效（`%subscription-changed` 推送，无需轮询）。
    pub fn status_subscription_active(&self) -> bool {
        self.client.status_subscription_active()
    }

    /// 当前连接累计读写字节 `(down, up)`（SSH 才有非零值）。
    pub fn traffic_bytes(&self) -> (u64, u64) {
        self.client.traffic_bytes()
    }

    /// 停止事件轮询定时器。
    pub fn stop_polling(&mut self) {
        if let Some(id) = self.poll_source.take() {
            id.remove();
        }
    }

    pub fn execute(&self, task: CTask) -> i32 {
        self.client.execute(&task)
    }

    /// 显式分离 tmux/SSH control client；session 由 tmux server 保留。
    pub fn detach(&self) -> i32 {
        self.client.detach()
    }

    /// 轮询事件；立刻拷贝 data/name，避免下次 poll 失效。
    pub fn poll_events(&self) -> Vec<BridgeEvent> {
        self.client.poll_events()
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

    pub fn send_input(&self, pane_id: u32, data: &[u8]) -> i32 {
        self.client.send_input(pane_id, data)
    }

    /// 同步 tmux/SSH control client 的整体字符格尺寸。
    pub fn resize_client(&self, cols: u16, rows: u16) -> i32 {
        self.client.resize_client(cols, rows)
    }

    /// 同步本地 pane 的 pty 字符格尺寸。
    pub fn resize_pane(&self, pane_id: u32, cols: u16, rows: u16) -> i32 {
        self.client.resize_pane(pane_id, cols, rows)
    }

    /// 上报单个 pane 的前景/背景色（`refresh-client -r`），供 tmux 代答 OSC 10/11。
    pub fn report_pane_colours(&self, pane_id: u32, fg_hex: &str, bg_hex: &str) -> i32 {
        self.client.report_pane_colours(pane_id, fg_hex, bg_hex)
    }

    /// 上报所有 pane 的颜色（主题切换后必须整段对齐）。
    pub fn report_all_pane_colours(&self, fg_hex: &str, bg_hex: &str) -> i32 {
        self.client.report_all_pane_colours(fg_hex, bg_hex)
    }

    /// 抓取 status bar 快照（只读查询，tmux 兼容），返回解析后的快照。
    pub fn status_snapshot(
        &self,
    ) -> Option<crate::platform::linux::quickconnect::status_style::StatusBarSnapshot> {
        if !self.uses_tmux() {
            return None;
        }
        // SSH：runtime 归一化为 tmux-ssh，FFI 期望 runtime_type="ssh"
        let ffi_runtime = if self.runtime_type == "tmux-ssh" {
            "ssh"
        } else {
            self.runtime_type.as_str()
        };
        let session = self.session.as_deref()?;
        let value = FfiClient::status_snapshot_json(
            ffi_runtime,
            self.ssh_alias.as_deref(),
            self.socket.as_deref(),
            session,
        )
        .ok()?;
        value
            .get("status")
            .cloned()
            .and_then(|status| serde_json::from_value(status).ok())
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

impl Drop for CoreBridge {
    fn drop(&mut self) {
        self.stop_polling();
    }
}

// ============================================================================
// 无状态 discovery（本地 / SSH 目录、tmux session、SSH host）
// ============================================================================

impl CoreBridge {
    /// 发现用户 SSH 配置中的 Host alias。
    pub fn discover_ssh_hosts() -> anyhow::Result<Vec<SshHostEntry>> {
        FfiClient::discover_ssh_hosts()
    }

    /// 列出本地或远端目录条目。
    pub fn list_dir(
        runtime_type: &str,
        target: Option<&str>,
        path: &str,
    ) -> anyhow::Result<Vec<FsEntry>> {
        FfiClient::list_dir(runtime_type, target, path)
    }

    /// 发现本地或 SSH 工作区候选。
    pub fn discover_workspaces(
        runtime_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
    ) -> anyhow::Result<Vec<WorkspaceCandidate>> {
        FfiClient::discover_workspaces(runtime_type, target, socket)
    }

    /// 创建 detached 工作区（Project attach→create fallback 用）。
    pub fn create_workspace(
        runtime_type: &str,
        target: Option<&str>,
        socket: Option<&str>,
        session: &str,
        directory: &str,
    ) -> anyhow::Result<String> {
        FfiClient::create_workspace(runtime_type, target, socket, session, directory)
    }
}

/// 构造常用 CTask 的便捷函数。
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
    use crate::ffi::{
        DIR_HORIZONTAL, DIR_VERTICAL, STATE_PANE_FRAME, STATE_PANE_OUTPUT, TASK_CLOSE_PANE,
        TASK_CLOSE_TAB, TASK_DETACH, TASK_NEW_TAB, TASK_NEXT_PANE, TASK_PREV_PANE, TASK_SPLIT_PANE,
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

    #[test]
    fn is_pane_frame_matches_only_full_frame_type() {
        let ev = BridgeEvent {
            type_: STATE_PANE_FRAME,
            pane_id: 1,
            tab_id: 0,
            window_id: 0,
            data: b"frame".to_vec(),
            name: String::new(),
        };
        assert!(CoreBridge::is_pane_frame(&ev));
        assert!(!CoreBridge::is_pane_output(&ev));
    }
}
