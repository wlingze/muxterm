//! AppWindow test and integration-test hooks.
//!
//! The runtime window orchestration stays in `window`; this module keeps
//! the stable production paths exposed to GTK/e2e tests in one boundary.

use super::window_actions::{adjust_font, handle_action, paste_active_pane};
use super::window_activity::{
    drain_attention_notifications, update_command_marks, update_jump_latest,
};
use super::window_connection::connect_target;
use super::window_discovery::{
    drain_existing_ssh, drain_local_existing, drain_ssh_probes, maybe_schedule_reconnect,
};
use super::window_event_pump::{
    activity_snapshot, drain_surface_input, enqueue_workspace_input, flush_command_queue,
    poll_event_store, sync_view_store,
};
use super::window_overlay::{activate_attention_workspace, open_pane_find, open_panel};
use super::window_render::{refresh_event_workspaces, sync_pane_outputs};
use super::window_resize::sync_window_size;
use super::window_scene::after_activate;
use super::window_sidebar::{maybe_warn_workspace_capacity, refresh_sidebar_if_open};
use super::window_status::{
    maybe_refresh_status, refresh_attention_chrome, refresh_connection_summary,
};
use super::*;

use crate::frontend::ffi_client::{ClientOpenIntent, ClientTarget};
use crate::frontend::linux::quickconnect::model::TargetConfig;

impl AppWindow {
    /// W21 测试钩子：向指定 pane 的生产滚轮路径发一次滚动。
    pub fn test_emit_scroll(&self, pane: u32, delta_y: f64) {
        let s = self._state.borrow();
        if let Some(view) = s.active_layout().pane(pane).cloned() {
            view.test_emit_scroll(delta_y);
        }
    }

    /// 测试用：把字节直接喂进 PaneView（含 reply_state / OSC 52）。
    pub fn test_feed_pane_view(&self, pane: u32, bytes: &[u8]) {
        let s = self._state.borrow();
        if let Some(view) = s.active_layout().pane(pane).cloned() {
            view.feed_output(bytes);
            view.flush_deferred_feed();
        }
    }

    /// 测试用：当前激活 pane 走生产粘贴路径。
    pub fn test_paste_active(&self) {
        let state = self._state.clone();
        let s = state.borrow();
        paste_active_pane(&s, &state);
    }

    /// 测试用：向当前激活 pane 发送原始输入（如 `echo hi\n` / `\x04` Ctrl+D）。
    pub fn test_send_input(&self, data: &[u8]) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        let pane = s.active_pane;
        let workspace_key = active_workspace_key(&s);
        enqueue_workspace_input(&s, &workspace_key, pane, data, false);
        s.compatibility_activity.on_user_input(&ws, pane);
    }

    /// 测试用：向当前 VTE 发出生产 `commit` 信号。与 `test_send_input` 不同，
    /// 这里必须经过 PaneView.connect_input → 当前 Runtime 的完整输入路径。
    pub fn test_emit_active_pane_commit(&self, text: &str) -> bool {
        let view = {
            let s = self._state.borrow();
            s.active_layout().pane(s.active_pane).cloned()
        };
        let Some(view) = view else {
            return false;
        };
        view.test_emit_commit(text);
        true
    }

    /// 测试用：调用生产快捷键动作分发，禁止集成测试绕过 `handle_action`
    /// 直接构造 `Task`。
    pub fn test_handle_action(&self, action: Action) {
        let mut s = self._state.borrow_mut();
        handle_action(&mut s, action, &self.window, &self._state);
    }

    /// 测试用：当前工作区的稳定 replica id，供 `WorkspacePool` 切换断言。
    pub fn test_active_workspace_replica_id(&self) -> String {
        active_workspace_id(&self._state.borrow())
    }

    /// 测试用：当前工作区 Runtime id。
    pub fn test_active_workspace_runtime(&self) -> String {
        self._state
            .borrow()
            .active_workspace_runtime()
            .unwrap_or_default()
            .to_string()
    }

    /// 测试用：能力判断必须走 Runtime 契约，不能按 runtime 名字分支。
    pub fn test_active_runtime_supports(&self, capability: ClientRuntimeCapability) -> bool {
        let s = self._state.borrow();
        s.active_supports(capability)
    }

    /// 测试用：对当前 Runtime 执行真实 detach，并保留精确 outcome。
    pub fn test_detach_active_workspace_outcome(&self) -> anyhow::Result<TaskOutcome> {
        let s = self._state.borrow();
        if !s.active_supports(ClientRuntimeCapability::PersistDetach) {
            return Ok(TaskOutcome::Rejected {
                reason: "active Runtime does not support PersistDetach".into(),
            });
        }
        match s.execute_active_task(ClientTask::Detach) {
            Ok(()) => {
                let result = flush_command_queue(&s)
                    .into_iter()
                    .find(|code| *code != 0)
                    .map(|code| TaskOutcome::Rejected {
                        reason: format!("Core FFI task dispatch failed: code={code}"),
                    })
                    .unwrap_or(TaskOutcome::Done);
                Ok(result)
            }
            Err(error) => Ok(TaskOutcome::Rejected {
                reason: error.to_string(),
            }),
        }
    }

    /// 测试用：走生产 `adjust_font(+1)`（Ctrl+= 热路径）。
    pub fn test_increase_font(&self) {
        let mut s = self._state.borrow_mut();
        adjust_font(&mut s, &self._state, 1);
    }

    /// 测试用：走生产 `adjust_font(-1)`（Ctrl+- 热路径）。
    pub fn test_decrease_font(&self) {
        let mut s = self._state.borrow_mut();
        adjust_font(&mut s, &self._state, -1);
    }

    /// 测试用：当前 UiState 字号（缩放热路径断言）。
    pub fn test_font_size(&self) -> f32 {
        self._state.borrow().font.size
    }

    /// 测试用：通过当前 EventPump 的 Core 配置服务提交一个字段修改。
    /// 提交只产生 `ConfigChanged`；调用方必须再走 `test_poll_once` 验证热应用。
    pub fn test_commit_config_path(
        &self,
        dotted: &str,
        value: serde_json::Value,
    ) -> anyhow::Result<()> {
        self._state
            .borrow()
            .event_pump
            .client()
            .config_apply_path(dotted, value)
            .map(|_| ())
    }

    /// 测试用：读取当前前端已应用的主题名，而不是重新查询 Core。
    pub fn test_theme_name(&self) -> String {
        self._state.borrow().theme_name.clone()
    }

    /// 测试用：当前激活 pane 的核心输出快照。
    pub fn test_active_pane_output(&self) -> Vec<u8> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.event_pump
            .client()
            .get_workspace_pane_output(&workspace_id, s.active_pane)
    }

    /// 测试用：当前激活 pane 的 VTE 可见文本（比核心缓冲更能发现黑屏）。
    pub fn test_active_pane_vte_text(&self) -> String {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .map(|v| v.visible_text())
            .unwrap_or_default()
    }

    /// 测试用：把 pane 的 VTE 滚动到顶部（mock-codex 末帧头在 row 1，
    /// 视口默认在底部时 text_format 只返回可见区，看不到 TOKEN_HEADER）。
    pub fn test_scroll_pane_to_top(&self, pane_id: u32) {
        let s = self._state.borrow();
        if let Some(view) = s.active_layout().pane(pane_id) {
            if let Some(adj) = view.terminal().vadjustment() {
                adj.set_value(adj.lower());
            }
        }
    }

    /// 测试用：指定 pane 的 VTE 文本。
    pub fn test_pane_vte_text(&self, pane_id: u32) -> String {
        let s = self._state.borrow();
        let t = s
            .active_layout()
            .pane(pane_id)
            .map(|v| v.visible_text())
            .unwrap_or_default();
        t
    }

    /// 测试用：指定 pane 的 VTE scrollback + 当前屏完整文本。
    pub fn test_pane_vte_buffer_text(&self, pane_id: u32) -> String {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|view| view.buffer_text())
            .unwrap_or_default()
    }

    /// 测试用：指定 pane 的 VTE **当前屏幕**文本（不含 scrollback）。
    ///
    /// Ctrl-L 后旧内容可能留在 VTE scrollback；“当前屏不可见 BEFORE”
    /// 断言必须只看屏幕，不能把 scrollback 算进去。
    pub fn test_pane_screen_text(&self, pane_id: u32) -> String {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| v.screen_text())
            .unwrap_or_default()
    }

    /// 测试用：指定 pane 的 VTE 光标行（0 起；最后一行 = rows-1）。
    pub fn test_pane_cursor_row(&self, pane_id: u32) -> i64 {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| v.cursor_row())
            .unwrap_or(-1)
    }

    /// 测试用：指定 pane 的 VTE 屏幕行数。
    pub fn test_pane_screen_rows(&self, pane_id: u32) -> i64 {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| v.screen_rows())
            .unwrap_or(0)
    }

    /// 测试用：当前 tab 布局 leaf pane id。
    pub fn test_layout_leaf_ids(&self) -> Vec<u32> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        let Some(view) = s.view_store.workspace(&workspace_id) else {
            return Vec::new();
        };
        let active_tab = s.visible_tab_id(&workspace_id).unwrap_or(s.active_tab);
        let Some(layout) = view.layouts.get(&active_tab) else {
            return Vec::new();
        };
        fn leaves(layout: &crate::frontend::ffi_client::ClientLayout, out: &mut Vec<u32>) {
            match layout {
                crate::frontend::ffi_client::ClientLayout::Leaf { pane_id } => out.push(*pane_id),
                crate::frontend::ffi_client::ClientLayout::Split { first, second, .. } => {
                    leaves(first, out);
                    leaves(second, out);
                }
            }
        }
        let mut ids = Vec::new();
        leaves(layout, &mut ids);
        ids
    }

    /// 测试用：按先序返回当前 GTK 布局里每个 GtkPaned 的真实方向。
    ///
    /// 不能只断言 core LayoutNode；这里要保证 Herdr 的上下分割最终确实
    /// 变成 GTK Vertical，而不是在 platform 边界再次被翻成左右分割。
    pub fn test_gtk_paned_orientations(&self) -> Vec<gtk4::Orientation> {
        fn collect(widget: &gtk4::Widget, out: &mut Vec<gtk4::Orientation>) {
            let Ok(paned) = widget.clone().downcast::<gtk4::Paned>() else {
                return;
            };
            out.push(paned.orientation());
            if let Some(child) = paned.start_child() {
                collect(&child, out);
            }
            if let Some(child) = paned.end_child() {
                collect(&child, out);
            }
        }

        let s = self._state.borrow();
        let mut orientations = Vec::new();
        if let Some(root) = s.active_layout().active_root_widget() {
            collect(&root, &mut orientations);
        }
        orientations
    }

    /// 测试用：真实 GTK 子树签名。`H(L,V(L,L))` 表示左侧单 pane，右侧
    /// 再上下分割；可抓住把 Herdr Vertical 错画成 Horizontal 的回归。
    pub fn test_gtk_layout_signature(&self) -> String {
        fn signature(widget: &gtk4::Widget) -> String {
            let Ok(paned) = widget.clone().downcast::<gtk4::Paned>() else {
                return "L".to_string();
            };
            let direction = match paned.orientation() {
                gtk4::Orientation::Horizontal => "H",
                gtk4::Orientation::Vertical => "V",
                _ => "?",
            };
            let start = paned
                .start_child()
                .map(|child| signature(&child))
                .unwrap_or_else(|| "_".to_string());
            let end = paned
                .end_child()
                .map(|child| signature(&child))
                .unwrap_or_else(|| "_".to_string());
            format!("{direction}({start},{end})")
        }

        let s = self._state.borrow();
        s.active_layout()
            .active_root_widget()
            .map(|root| signature(&root))
            .unwrap_or_default()
    }

    /// 测试用：pane 控件分配尺寸（0×0 = 白屏）。
    pub fn test_pane_allocation(&self, pane_id: u32) -> (i32, i32) {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| {
                let w = v.widget();
                (w.width(), w.height())
            })
            .unwrap_or((0, 0))
    }

    /// 测试用：flush 全部 VTE 合并缓冲后再读文本。
    pub fn test_flush_feeds(&self) {
        self._state.borrow().active_layout().flush_all_feeds();
    }

    /// 测试用：轮询一次并返回本批 `PaneOutput` 条数（1820 CPU）。
    pub fn test_poll_output_event_count(&self) -> usize {
        drain_ssh_probes(&self._state);
        drain_existing_ssh(&self._state);
        drain_local_existing(&self._state);
        maybe_schedule_reconnect(&self._state);
        maybe_warn_workspace_capacity(&self._state, &self.window);
        let (n, pending_close) = {
            let mut s = self._state.borrow_mut();
            let events = poll_event_store(&mut s);
            refresh_event_workspaces(&mut s, &events);
            let n = events
                .iter()
                .filter(|event| {
                    matches!(
                        event.event.kind(),
                        ClientEventKind::PaneOutput | ClientEventKind::PaneFrame
                    )
                })
                .count();
            drain_attention_notifications(&mut s);
            sync_pane_outputs(&mut s);
            maybe_refresh_status(&mut s, true);
            refresh_connection_summary(&mut s);
            update_command_marks(&s);
            update_jump_latest(&s);
            refresh_attention_chrome(&s, &self.window);
            let close = s.pending_close;
            if close {
                s.pending_close = false;
            }
            (n, close)
        };
        if pending_close {
            self._state.borrow_mut().quit_requested = true;
            self.window.close();
        }
        n
    }

    /// 测试用：当前激活 pane 的 VTE reset 次数（F1 Surface 契约）。
    pub fn test_active_pane_resets(&self) -> u32 {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .map(|v| v.render_trace().resets)
            .unwrap_or(0)
    }

    /// 测试用：当前激活 pane 是否已经完成首屏 Surface seed。
    ///
    /// 新 tab/远端 SSH 的 pane topology 可能先收敛，随后才在 GTK
    /// allocation 后播种快照；输入契约必须从 seed 完成后开始计数，不能
    /// 把正常的首屏 reset 误报成命令期间的 reset。
    pub fn test_active_pane_seeded(&self) -> bool {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .is_some_and(|view| view.is_seeded())
    }

    /// 测试用：指定 pane 的渲染痕迹（seeds/feeds/bytes）。
    pub fn test_pane_render_trace(&self, pane_id: u32) -> (u32, u32, usize) {
        let s = self._state.borrow();
        s.active_layout()
            .pane(pane_id)
            .map(|v| {
                let t = v.render_trace();
                (t.seeds, t.feeds, t.bytes_fed)
            })
            .unwrap_or((0, 0, 0))
    }

    /// 测试用：清空当前激活 pane 的渲染痕迹（切 tab 前归零）。
    pub fn test_clear_active_pane_render_trace(&self) {
        let s = self._state.borrow();
        if let Some(v) = s.active_layout().pane(s.active_pane) {
            v.clear_render_trace();
        }
    }

    /// 测试用：tab / 当前 tab 的 pane 数量。
    pub fn test_tab_and_pane_counts(&self) -> (usize, usize) {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        let Some(view) = s.view_store.workspace(&workspace_id) else {
            return (0, 0);
        };
        let n_tabs = view.tabs.len();
        let active_tab = s.visible_tab_id(&workspace_id).unwrap_or(s.active_tab);
        let n_panes = view.panes.get(&active_tab).map_or(0, Vec::len);
        (n_tabs, n_panes)
    }

    /// 测试用：状态栏文案。
    pub fn test_status_text(&self) -> String {
        self._state.borrow().status.plain_text()
    }

    /// 测试用：手动轮询一次核心事件并刷新输出（不等待 16ms 定时器）。
    pub fn test_poll_once(&self) {
        drain_ssh_probes(&self._state);
        drain_existing_ssh(&self._state);
        drain_local_existing(&self._state);
        maybe_schedule_reconnect(&self._state);
        maybe_warn_workspace_capacity(&self._state, &self.window);
        let pending_close = {
            let mut s = self._state.borrow_mut();
            let events = poll_event_store(&mut s);
            refresh_event_workspaces(&mut s, &events);
            drain_attention_notifications(&mut s);
            sync_pane_outputs(&mut s);
            sync_window_size(&mut s);
            drain_surface_input(&mut s);
            flush_command_queue(&s);
            maybe_refresh_status(&mut s, true);
            refresh_connection_summary(&mut s);
            update_command_marks(&s);
            update_jump_latest(&s);
            refresh_attention_chrome(&s, &self.window);
            let close = s.pending_close;
            if close {
                s.pending_close = false;
            }
            close
        };
        if pending_close {
            self._state.borrow_mut().quit_requested = true;
            self.window.close();
        }
    }

    /// W21 测试钩子：最近一次经 PaneView input_cb 的原始输入。
    pub fn test_last_raw_input(&self) -> Vec<u8> {
        self._state.borrow().last_raw_input.clone()
    }

    /// W21 测试钩子：指定 pane 的 reply_state 是否在 alt-screen。
    pub fn test_pane_mouse_reporting(&self, pane: u32) -> bool {
        self._state
            .borrow()
            .active_layout()
            .pane(pane)
            .map(|v| v.test_mouse_reporting())
            .unwrap_or(false)
    }

    pub fn test_pane_alternate_screen(&self, pane: u32) -> bool {
        self._state
            .borrow()
            .active_layout()
            .pane(pane)
            .map(|v| v.test_alternate_screen())
            .unwrap_or(false)
    }

    /// W19e 测试钩子：注入一次 fault（report + 弹窗），进程必须继续。
    pub fn test_inject_fault(&self, token: &str) {
        crate::frontend::linux::fault_gtk::inject_fault(token);
    }

    /// 测试用：主窗口本身（供 widget 树断言）。
    pub fn test_window(&self) -> gtk4::Window {
        self.window.clone()
    }

    /// 测试用：QuickConnect 面板是否打开。
    pub fn test_panel_open(&self) -> bool {
        self._state.borrow().panel_open.is_some()
    }

    /// 测试用：当前 pane 的 VTE 是否持有键盘焦点。
    pub fn test_active_terminal_has_focus(&self) -> bool {
        let s = self._state.borrow();
        s.active_layout()
            .pane(s.active_pane)
            .is_some_and(|view| view.terminal().has_focus())
    }

    /// 测试用：当前面板 tab（0=workspaces / 1=attention / 2=search）。
    pub fn test_active_panel_tab(&self) -> u32 {
        self._state
            .borrow()
            .panel_open
            .map(|t| t as u32)
            .unwrap_or(0)
    }

    /// 测试用：全部 tab id（core 顺序）。
    pub fn test_tab_ids(&self) -> Vec<u32> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.view_store
            .workspace(&workspace_id)
            .map(|view| view.tabs.iter().map(|tab| tab.id).collect())
            .unwrap_or_default()
    }

    /// 测试用：全部 tab 名（core 顺序；W7 new_tab_shortcut 断言非空/raw label）。
    pub fn test_tab_names(&self) -> Vec<String> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.view_store
            .workspace(&workspace_id)
            .map(|view| view.tabs.iter().map(|tab| tab.name.clone()).collect())
            .unwrap_or_default()
    }

    /// 测试用：指定 replica 的 herdr 运行时 stream 探针（takeover_watchdog 用）。
    /// 返回 (stream_starts, control_takeover_starts, takeover_suppressed, actual_mode)。
    pub fn test_herdr_probe(&self, replica: &str, pane: u32) -> Option<(u64, u64, bool, String)> {
        let s = self._state.borrow();
        let workspace_key = s
            .view_store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .find(|workspace| {
                parse_workspace_id(&workspace.id).is_some_and(|id| id.replica_id() == replica)
            })
            .map(|workspace| workspace.id.clone())?;
        s.event_pump
            .client()
            .herdr_probe(&workspace_key, pane)
            .map(|probe| {
                (
                    probe.stream_starts,
                    probe.control_takeover_starts,
                    probe.takeover_suppressed,
                    probe.actual_mode,
                )
            })
    }

    /// 测试用：当前激活 tab id。
    pub fn test_active_tab_id(&self) -> u32 {
        self._state.borrow().active_tab
    }

    /// 测试用：已有的连接探测线程是否已收完（local-first 流式结束后 idle）。
    pub fn test_existing_probe_idle(&self) -> bool {
        let s = self._state.borrow();
        s.pending_local_probe.is_empty() && !s.existing.borrow().probe_inflight
    }

    /// 测试用：以指定 tab 打开面板（0=workspaces / 1=attention / 2=search）。
    pub fn test_open_panel(&self, tab: u32) {
        let state = self._state.clone();
        let tab = match tab {
            0 => PanelTab::Workspaces,
            1 => PanelTab::Attention,
            _ => PanelTab::Search,
        };
        open_panel(&state, &self.window, tab);
    }

    /// 测试用：当前 blocked 工作区数（红点 N）。
    pub fn test_attention_blocked_workspaces(&self) -> usize {
        activity_snapshot(&self._state.borrow()).blocked_count
    }

    /// 测试用：读取当前 Core activity 快照，诊断跨 workspace 的注意力状态。
    pub fn test_attention_snapshot(&self) -> crate::frontend::ffi_client::ClientActivitySnapshot {
        activity_snapshot(&self._state.borrow())
    }

    /// 测试用：窗口标题（M3.4 接红点前缀，当前返回原始标题）。
    pub fn test_window_title(&self) -> String {
        self.window
            .title()
            .map(|t| t.to_string())
            .unwrap_or_default()
    }

    /// 测试用：工作区 PaneBuf 中某 pane 的最近 n 行。
    pub fn test_replica_last_n(&self, pane_id: u32, n: usize) -> Vec<String> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_key(&s);
        s.event_pump
            .client()
            .workspace_pane_last_n_lines(&workspace_id, pane_id, n as u32)
            .unwrap_or_default()
    }

    /// 测试用：绕过 tmux 直接向 Surface/前端 activity 兼容层注入字节。
    pub fn test_feed_replica(&self, pane_id: u32, bytes: &[u8]) {
        let mut s = self._state.borrow_mut();
        if let Some(view) = s.active_layout().pane(pane_id).cloned() {
            view.feed_output(bytes);
            view.flush_deferred_feed();
        }
        let workspace = active_workspace_id(&s);
        let visible = pane_id == s.active_pane;
        s.compatibility_activity
            .apply_output(&workspace, pane_id, bytes, visible);
        refresh_sidebar_if_open(&mut s);
    }

    /// 测试用：本轮进入 blocked 的 workspace 通知记录。
    pub fn test_notifications_recorded(&self) -> Vec<String> {
        self._state.borrow().notification_log.clone()
    }

    /// 测试用：所有工作区 Done pane 数之和（任务完成，不是 blocked）。
    pub fn test_attention_done_count(&self) -> usize {
        let snapshot = activity_snapshot(&self._state.borrow());
        snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.done)
            .sum()
    }

    /// 测试用：通过通用 Attention 权威状态模拟任意 Runtime 的 agent。
    pub fn test_set_agent_attention(
        &self,
        pane: u32,
        process_name: &str,
        status: ClientAttentionStatus,
    ) {
        let mut s = self._state.borrow_mut();
        let ws = active_workspace_id(&s);
        s.compatibility_activity
            .set_agent_attention(&ws, pane, process_name, status);
        refresh_sidebar_if_open(&mut s);
    }

    /// 测试用：当前激活 pane id。
    pub fn test_active_pane_id(&self) -> u32 {
        self._state.borrow().active_pane
    }

    /// 测试用：SwitchPane（后台完成通知必须打在非前台 pane）。
    pub fn test_switch_pane(&self, pane_id: u32) {
        let _ = self
            ._state
            .borrow()
            .execute_active_task(ClientTask::SwitchPane { pane_id });
    }

    /// 测试用：生产搜索路径 `WorkspacePool::search_all`（不是 Mock PaneBuf）。
    pub fn test_search_all(&self, query: &str) -> Vec<(String, u32, String)> {
        self._state
            .borrow()
            .event_pump
            .client()
            .search_all(query)
            .unwrap_or_default()
            .into_iter()
            .map(|h| (h.workspace_id, h.pane_id, h.line))
            .collect()
    }

    /// 测试用：连接一个 QuickConnect 目标（走生产 connect_target 路径）。
    pub fn test_connect_target(&self, config: TargetConfig) {
        connect_target(&self._state.clone(), config);
    }

    /// 测试用：通过 FFI semantic target attach（SSH loopback 必须带远端 `-L`）。
    ///
    /// 等连接完成并激活后再返回：测试随后 `wait_ready` / 取 leaf 时看到的是
    /// 新工作区，而不是启动时的本地 shell（W18b 的 pane id 才不会串）。
    pub fn test_open_target(
        &self,
        target: ClientTarget,
        workspace_replica_id: String,
        socket: Option<String>,
    ) {
        let result = {
            let s = self._state.borrow();
            s.event_pump
                .client()
                .open_target(&target, ClientOpenIntent::AttachOnly)
        };
        match result {
            Ok(opened) => {
                let mut s = self._state.borrow_mut();
                if let Some(opened_id) = parse_workspace_id(&opened.id) {
                    s.workspace_sockets.insert(opened_id, socket);
                }
                if sync_view_store(&mut s).is_ok() {
                    after_activate(&mut s);
                }
            }
            Err(error) => {
                self._state
                    .borrow_mut()
                    .notification_log
                    .push(format!("{workspace_replica_id}: connect failed: {error}"));
            }
        }
    }

    /// 测试用：当前池里各工作区 replica id（`name@transport`）。
    pub fn test_workspace_replica_ids(&self) -> Vec<String> {
        self._state
            .borrow()
            .view_store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .filter_map(|workspace| parse_workspace_id(&workspace.id))
            .map(|id| id.replica_id())
            .collect()
    }

    /// 测试用：池里各工作区的 runtime 种类（断言没误开成本地 tmux）。
    pub fn test_workspace_runtimes(&self) -> Vec<String> {
        self._state
            .borrow()
            .view_store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .map(|workspace| workspace.runtime.clone())
            .collect()
    }

    /// 测试用：按 replica id 激活工作区（上次看到这里 / 跨工作区搜索）。
    pub fn test_activate_workspace(&self, replica: &str) {
        let mut s = self._state.borrow_mut();
        activate_attention_workspace(&mut s, replica);
    }

    /// 测试用：只搜当前工作区当前 pane。
    pub fn test_search_pane(&self, pane: u32, query: &str) -> Vec<(String, u32, String)> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_id(&s);
        s.event_pump
            .client()
            .search_all(query)
            .unwrap_or_default()
            .into_iter()
            .filter(|hit| hit.workspace_id == workspace_id && hit.pane_id == pane)
            .map(|h| (h.workspace_id, h.pane_id, h.line))
            .collect()
    }

    /// 测试用：只搜当前工作区全部 pane。
    pub fn test_search_workspace(&self, query: &str) -> Vec<(String, u32, String)> {
        let s = self._state.borrow();
        let workspace_id = active_workspace_id(&s);
        s.event_pump
            .client()
            .search_all(query)
            .unwrap_or_default()
            .into_iter()
            .filter(|hit| hit.workspace_id == workspace_id)
            .map(|h| (h.workspace_id, h.pane_id, h.line))
            .collect()
    }

    /// 测试用：打开当前 pane 内查找条（与 Ctrl+F 同一条生产路径）。
    pub fn test_open_pane_find(&self) {
        open_pane_find(&self._state.clone());
    }
}
