//! 主窗口：FFI 驱动的 GTK4 前端。
//!
//! - 启动 `CoreBridge`（muxterm_new/connect）
//! - 16ms 轮询 `poll_events`，分发到 tab / pane
//! - 快捷键 → `execute(CTask)`
//! - 退出 → `shutdown()` 或 Drop（`muxterm_free`）

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, ApplicationWindow, Box, CssProvider, EventControllerKey, Label, Orientation, Window,
};
use vte4::prelude::*;

use crate::core::config::{Action, Config, Theme};
use crate::platform::linux::ffi_bridge::{tasks, BridgeEvent, CoreBridge};
use crate::platform::linux::keymap::KeyMap;
use crate::platform::linux::layout_host::LayoutHost;
use crate::platform::linux::tab_bar::TabBar;

/// 主窗口。
pub struct AppWindow {
    pub window: Window,
    /// 保持 UI 状态与 CoreBridge 存活（轮询闭包只用 Weak，避免循环引用）。
    _state: Rc<RefCell<UiState>>,
}

struct UiState {
    bridge: CoreBridge,
    /// 当前窗口是否持有 tmux/SSH control client；local shell 不支持 detach。
    uses_tmux: bool,
    tabs: TabBar,
    layout: LayoutHost,
    status: Label,
    keymap: KeyMap,
    active_tab: u32,
    active_pane: u32,
    /// 最近一次同步给后端/PTY 的字符格尺寸，避免 16ms 轮询重复 resize。
    last_client_size: Option<(u16, u16)>,
}

impl AppWindow {
    /// 有序关闭：停轮询 → 摘掉子树 → destroy 窗口，避免与 PaneView 持有的 VTE 交叉销毁。
    pub fn shutdown(self) {
        {
            let mut s = self._state.borrow_mut();
            s.bridge.stop_polling();
            while let Some(child) = s.layout.root_box.first_child() {
                s.layout.root_box.remove(&child);
            }
            s.layout.panes_mut().clear();
        }
        self.window.set_child(None::<&gtk4::Widget>);
        self.window.destroy();
        while glib::MainContext::default().iteration(false) {}
    }

    pub fn new(cfg: Config, theme: Theme) -> Self {
        let window = ApplicationWindow::builder()
            .title("muxterm")
            .default_width(960)
            .default_height(640)
            .build();
        let window: Window = window.upcast();

        apply_css();

        let backend = if cfg.tmux.socket.trim().is_empty() {
            "local"
        } else {
            "tmux"
        };
        let socket = {
            let s = cfg.tmux.socket.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };
        let session = {
            let s = cfg.tmux.default_session.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };

        let requested_tmux = socket.is_some();
        let (bridge, uses_tmux) =
            match CoreBridge::new(backend, socket.as_deref(), session.as_deref()) {
                Ok(b) => (b, requested_tmux),
                Err(e) => {
                    tracing::error!(target = "muxterm::linux", "启动核心失败: {e}");
                    // 回退 local；回退后不能展示 tmux detach。
                    (
                        CoreBridge::new("local", None, None).expect("local backend 必须可用"),
                        false,
                    )
                }
            };

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        root.add_css_class("muxterm-root");

        let tabs = TabBar::new(cfg.ui.tab_bar_height);
        let layout = LayoutHost::new(theme);
        let status = Label::builder()
            .label(crate::platform::i18n::tr(
                crate::platform::i18n::Key::StatusConnected,
            ))
            .halign(Align::Start)
            .hexpand(true)
            .build();
        status.add_css_class("status-bar");

        if cfg.ui.tab_bar_at_bottom() {
            root.append(&layout.root_box);
            root.append(&status);
            root.append(&tabs.container);
        } else {
            root.append(&tabs.container);
            root.append(&layout.root_box);
            root.append(&status);
        }
        window.set_child(Some(&root));

        let keymap = KeyMap::from_bindings(&cfg.keybindings);
        let state = Rc::new(RefCell::new(UiState {
            bridge,
            uses_tmux,
            tabs,
            layout,
            status,
            keymap,
            active_tab: 0,
            active_pane: 0,
            last_client_size: None,
        }));

        // tab 点击
        {
            let st = state.clone();
            state.borrow().tabs.connect_activate(move |tab_id| {
                let mut s = st.borrow_mut();
                s.bridge.execute(tasks::switch_tab(tab_id));
                s.active_tab = tab_id;
                refresh_ui(&mut s);
            });
        }

        // 快捷键
        {
            let st = state.clone();
            let controller = EventControllerKey::new();
            controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let window_for_palette = window.clone();
            controller.connect_key_pressed(move |_c, keyval, _keycode, mods| {
                let mut s = st.borrow_mut();
                if let Some(action) = s.keymap.lookup(keyval, mods) {
                    handle_action(&mut s, action, &window_for_palette, &st);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            window.add_controller(controller);
        }

        // 关闭窗口清理
        {
            let st = state.clone();
            window.connect_close_request(move |_| {
                // Drop bridge via take
                let _ = st.borrow_mut();
                glib::Propagation::Proceed
            });
        }

        // 首次刷新 + 由 CoreBridge 托管的 16ms 轮询（GTK 主线程）
        {
            let mut s = state.borrow_mut();
            let _ = s.bridge.poll_events();
            refresh_ui(&mut s);
        }

        {
            let st_weak = Rc::downgrade(&state);
            let win_weak = window.downgrade();
            let mut s = state.borrow_mut();
            s.bridge.start_polling(16, move || {
                if win_weak.upgrade().is_none() {
                    return false;
                }
                let Some(st) = st_weak.upgrade() else {
                    return false;
                };
                let mut s = st.borrow_mut();
                let events = s.bridge.poll_events();
                for ev in events {
                    dispatch_event(&mut s, &ev);
                }
                sync_pane_outputs(&mut s);
                sync_window_size(&mut s);
                true
            });
        }

        Self {
            window,
            _state: state,
        }
    }

    /// 测试用：向当前激活 pane 发送原始输入（如 `echo hi\n` / `\x04` Ctrl+D）。
    pub fn test_send_input(&self, data: &[u8]) {
        let s = self._state.borrow();
        let _ = s.bridge.send_input(s.active_pane, data);
    }

    /// 测试用：当前激活 pane 的核心输出快照。
    pub fn test_active_pane_output(&self) -> Vec<u8> {
        let s = self._state.borrow();
        s.bridge.get_pane_output(s.active_pane)
    }

    /// 测试用：当前激活 pane 的 VTE 可见文本（比核心缓冲更能发现黑屏）。
    pub fn test_active_pane_vte_text(&self) -> String {
        let s = self._state.borrow();
        s.layout
            .pane(s.active_pane)
            .map(|v| v.visible_text())
            .unwrap_or_default()
    }

    /// 测试用：tab / 当前 tab 的 pane 数量。
    pub fn test_tab_and_pane_counts(&self) -> (usize, usize) {
        let s = self._state.borrow();
        let n_tabs = s.bridge.get_tabs().len();
        let n_panes = s.bridge.get_panes(s.active_tab).len();
        (n_tabs, n_panes)
    }

    /// 测试用：状态栏文案。
    pub fn test_status_text(&self) -> String {
        self._state.borrow().status.label().to_string()
    }

    /// 测试用：手动轮询一次核心事件并刷新输出（不等待 16ms 定时器）。
    pub fn test_poll_once(&self) {
        let mut s = self._state.borrow_mut();
        let events = s.bridge.poll_events();
        for ev in events {
            dispatch_event(&mut s, &ev);
        }
        sync_pane_outputs(&mut s);
    }
}

fn handle_action(s: &mut UiState, action: Action, window: &Window, state: &Rc<RefCell<UiState>>) {
    match action {
        Action::NewTab | Action::NewWindow => {
            s.bridge.execute(tasks::new_tab());
        }
        Action::NewPane => {
            s.bridge.execute(tasks::split_h(s.active_pane));
        }
        Action::NewPaneVertical => {
            s.bridge.execute(tasks::split_v(s.active_pane));
        }
        Action::SwitchTab1 => switch_tab_n(s, 1),
        Action::SwitchTab2 => switch_tab_n(s, 2),
        Action::SwitchTab3 => switch_tab_n(s, 3),
        Action::SwitchTab4 => switch_tab_n(s, 4),
        Action::SwitchTab5 => switch_tab_n(s, 5),
        Action::SwitchTab6 => switch_tab_n(s, 6),
        Action::SwitchTab7 => switch_tab_n(s, 7),
        Action::SwitchTab8 => switch_tab_n(s, 8),
        Action::SwitchTab9 => switch_tab_n(s, 9),
        Action::SwitchTabLast => {
            let tabs = s.bridge.get_tabs();
            if let Some(t) = tabs.last() {
                s.bridge.execute(tasks::switch_tab(t.id));
                s.active_tab = t.id;
            }
        }
        Action::SwitchPaneNext => {
            s.bridge.execute(tasks::next_pane());
        }
        Action::SwitchPanePrev => {
            s.bridge.execute(tasks::prev_pane());
        }
        Action::CommandPalette => {
            // 命令面板回调在 GTK 主线程执行；语言选择会更新当前进程 catalog，
            // 后续所有新建控件都会直接使用新语言。
            let parent = window.clone();
            let callback_parent = parent.clone();
            let callback_window = parent.clone();
            let callback_state = state.clone();
            let status = s.status.clone();
            let pane_count = s.bridge.get_panes(s.active_tab).len();
            let uses_tmux = s.uses_tmux;
            crate::platform::linux::command_palette::show_for_backend(
                &parent,
                uses_tmux,
                move |id| {
                    if id == "language" {
                        let language_parent = callback_parent.clone();
                        let status = status.clone();
                        crate::platform::linux::command_palette::show_language(
                            &language_parent,
                            move |language| {
                                crate::platform::i18n::set_language(language);
                                set_status_summary(&status, pane_count);
                            },
                        );
                    } else if id == crate::platform::linux::command_palette::TMUX_DETACH_COMMAND
                        && uses_tmux
                    {
                        let rc = callback_state.borrow().bridge.detach();
                        if rc == 0 {
                            // Task::Detach 已经由 core 发送 detach-client；关闭
                            // GTK client 只负责回收窗口和 bridge handle。
                            callback_window.close();
                        } else {
                            status.set_label(&format!(
                                "{}: {}",
                                crate::platform::i18n::tr(crate::platform::i18n::Key::StatusError),
                                crate::platform::i18n::tr(
                                    crate::platform::i18n::Key::ErrorCommandFailed
                                )
                            ));
                        }
                    }
                },
            );
            return;
        }
        Action::Search | Action::Unknown => {}
        // 以下动作由后续 QuickConnect/主题/字体/全屏接线处理；先保持编译通过。
        Action::QuickConnect | Action::Quit | Action::IncreaseFontSize
        | Action::DecreaseFontSize | Action::ResetFontSize
        | Action::TogglePaneFullscreen => {
            return;
        }
    }
    refresh_ui(s);
}

fn switch_tab_n(s: &mut UiState, n: usize) {
    let tabs = s.bridge.get_tabs();
    if let Some(t) = tabs.get(n.saturating_sub(1)) {
        s.bridge.execute(tasks::switch_tab(t.id));
        s.active_tab = t.id;
    }
}

fn dispatch_event(s: &mut UiState, ev: &BridgeEvent) {
    use crate::core::protocol::ffi::types::*;
    match ev.type_ {
        STATE_PANE_OUTPUT => {
            if let Some(view) = s.layout.pane(ev.pane_id) {
                view.feed_output(&ev.data);
                let replies = view.take_replies();
                if !replies.is_empty() {
                    let _ = s.bridge.send_input(ev.pane_id, &replies);
                }
            }
        }
        STATE_ACTIVE_TAB_CHANGED => {
            s.active_tab = ev.tab_id;
            refresh_ui(s);
        }
        STATE_ACTIVE_PANE_CHANGED => {
            s.active_pane = ev.pane_id;
        }
        STATE_TAB_ADDED | STATE_TAB_CLOSED | STATE_LAYOUT_CHANGED | STATE_PANE_ADDED
        | STATE_PANE_CLOSED => {
            refresh_ui(s);
        }
        STATE_BACKEND_STATUS => {
            let msg = match ev.pane_id {
                2 => crate::platform::i18n::tr(crate::platform::i18n::Key::StatusConnected),
                3 => crate::platform::i18n::tr(crate::platform::i18n::Key::StatusError),
                4 => crate::platform::i18n::tr(crate::platform::i18n::Key::StatusExited),
                1 => crate::platform::i18n::tr(crate::platform::i18n::Key::StatusConnecting),
                _ => crate::platform::i18n::tr(crate::platform::i18n::Key::StatusDisconnected),
            };
            s.status.set_label(&msg);
        }
        _ => {}
    }
}

fn refresh_ui(s: &mut UiState) {
    let tabs = s.bridge.get_tabs();
    s.tabs.set_tabs(&tabs);
    if let Some(active) = tabs.iter().find(|t| t.is_active) {
        s.active_tab = active.id;
    } else if let Some(first) = tabs.first() {
        s.active_tab = first.id;
    }

    // 重建布局
    if let Some(layout) = s.bridge.get_layout(s.active_tab) {
        let bridge_ptr = &s.bridge as *const CoreBridge;
        let input_cb = move |pane_id: u32, data: &[u8]| {
            // Safety: GTK 主线程，bridge 与窗口同寿
            let bridge = unsafe { &*bridge_ptr };
            let _ = bridge.send_input(pane_id, data);
        };
        s.layout.apply_layout(&layout, &input_cb);

        for pane in s.bridge.get_panes(s.active_tab) {
            if let Some(view) = s.layout.pane(pane.id) {
                let out = s.bridge.get_pane_output(pane.id);
                view.sync_full_output(&out);
                let replies = view.take_replies();
                if !replies.is_empty() {
                    let _ = s.bridge.send_input(pane.id, &replies);
                }
                if pane.is_active {
                    s.active_pane = pane.id;
                    view.grab_focus();
                }
            }
        }
    }

    let npanes = s.bridge.get_panes(s.active_tab).len();
    set_status_summary(&s.status, npanes);
}

fn set_status_summary(status: &Label, npanes: usize) {
    let connected = crate::platform::i18n::tr(crate::platform::i18n::Key::StatusConnected);
    let panes = crate::platform::i18n::tr(crate::platform::i18n::Key::Panes);
    let close_hint = crate::platform::i18n::tr(crate::platform::i18n::Key::WindowCloseHint);
    status.set_label(&format!("{connected} | {panes}: {npanes} | {close_hint}"));
}

fn sync_pane_outputs(s: &mut UiState) {
    for pane in s.bridge.get_panes(s.active_tab) {
        if let Some(view) = s.layout.pane(pane.id) {
            let out = s.bridge.get_pane_output(pane.id);
            view.sync_full_output(&out);
            let replies = view.take_replies();
            if !replies.is_empty() {
                let _ = s.bridge.send_input(pane.id, &replies);
            }
        }
    }
}

/// 把窗口内容区的新字符格尺寸同步给后端。
///
/// tmux/SSH 模式只发一次 client resize（`refresh-client -C`），避免逐个
/// pane 触发布局反馈；local 模式 resize 当前激活 pane 的 pty。
fn sync_window_size(s: &mut UiState) {
    let Some(view) = s.layout.pane(s.active_pane) else {
        return;
    };
    let term = view.terminal();
    let cw = term.char_width();
    let ch = term.char_height();
    if cw <= 0 || ch <= 0 {
        return;
    }
    let root_w = s.layout.root_box.width().max(0) as u64;
    let root_h = s.layout.root_box.height().max(0) as u64;
    if root_w == 0 || root_h == 0 {
        return;
    }
    let cols = (root_w / cw as u64).clamp(2, u16::MAX as u64) as u16;
    let rows = (root_h / ch as u64).clamp(1, u16::MAX as u64) as u16;
    if s.last_client_size == Some((cols, rows)) {
        return;
    }
    s.last_client_size = Some((cols, rows));
    if s.uses_tmux {
        let _ = s.bridge.resize_client(cols, rows);
    } else {
        let _ = s.bridge.resize_pane(s.active_pane, cols, rows);
    }
}

fn apply_css() {
    let css = CssProvider::new();
    css.load_from_data(
        "
        .muxterm-root { background: #1e1e2e; }
        .tab-bar { background: #181825; }
        .tab-button { padding: 4px 12px; border-radius: 0; }
        .tab-button.active { background: #313244; }
        .status-bar { color: #a6adc8; padding: 2px 8px; font-size: 11px; }
        ",
    );
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
