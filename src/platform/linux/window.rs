//! 主窗口：FFI 驱动的 GTK4 前端。
//!
//! - 启动 `CoreBridge`（muxterm_new/connect）
//! - 16ms 轮询 `poll_events`，分发到 tab / pane
//! - 快捷键 → `execute(CTask)`
//! - 退出 → `shutdown()` 或 Drop（`muxterm_free`）

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{ApplicationWindow, Box, CssProvider, EventControllerKey, Orientation, Window};
use vte4::prelude::*;

use crate::core::config::{Action, Config, Theme};
use crate::platform::i18n::{self, Key};
use crate::platform::linux::connection_slot::{
    connection_key, startup_connection_key, WarmConnectionSlot,
};
use crate::platform::linux::ffi_bridge::{tasks, BridgeEvent, CoreBridge};
use crate::platform::linux::keymap::KeyMap;
use crate::platform::linux::layout_host::LayoutHost;
use crate::platform::linux::pane_view::rgb_hex;
use crate::platform::linux::quickconnect::event_policy::StateEventPolicy;
use crate::platform::linux::quickconnect::font::{FontSettings, Preferences};
use crate::platform::linux::quickconnect::model::{TargetConfig, TargetRuntime};
use crate::platform::linux::quickconnect::pool::{ConnectionPool, ConnectionPoolPolicy};
use crate::platform::linux::quickconnect::project_flow::{ProjectConnectFlow, ProjectConnectState};
use crate::platform::linux::quickconnect::status_style::{StatusBarMode, StatusBarSnapshot};
use crate::platform::linux::quickconnect::store::{user_quickconnect_path, QuickConnectStore};
use crate::platform::linux::quickconnect::tab_gate::TabSwitchGate;
use crate::platform::linux::quickconnect_panel::QuickConnectCallbacks;
use crate::platform::linux::status_bar::StatusBar;
use crate::platform::linux::tab_bar::TabBar;

/// 主窗口。
pub struct AppWindow {
    pub window: Window,
    /// 保持 UI 状态与 CoreBridge 存活（轮询闭包只用 Weak，避免循环引用）。
    _state: Rc<RefCell<UiState>>,
}

struct UiState {
    pool: ConnectionPool<WarmConnectionSlot>,
    qc_store: QuickConnectStore,
    poll_source: Option<glib::SourceId>,
    /// 当前窗口是否持有 tmux/SSH control client；local shell 不支持 detach。
    uses_tmux: bool,
    /// 当前终端字体（config + 运行期偏好）。
    font: FontSettings,
    /// config.toml 的字号，Reset 回到这里。
    config_font_size: f32,
    theme: Theme,
    theme_name: String,
    tabs: TabBar,
    layout: LayoutHost,
    status: StatusBar,
    status_mode: StatusBarMode,
    last_status_at: Instant,
    status_interval: Duration,
    keymap: KeyMap,
    active_tab: u32,
    active_pane: u32,
    /// 最近一次同步给后端/PTY 的字符格尺寸，避免 16ms 轮询重复 resize。
    last_client_size: Option<(u16, u16)>,
    tab_gate: TabSwitchGate,
    preferences: Preferences,
}

impl UiState {
    fn bridge(&self) -> &CoreBridge {
        &self.pool.active_slot().expect("必须有前台连接").bridge
    }

    fn bridge_mut(&mut self) -> &mut CoreBridge {
        &mut self.pool.active_slot_mut().expect("必须有前台连接").bridge
    }
}

impl AppWindow {
    /// 有序关闭：停轮询 → 摘掉子树 → destroy 窗口，避免与 PaneView 持有的 VTE 交叉销毁。
    pub fn shutdown(self) {
        {
            let mut s = self._state.borrow_mut();
            s.bridge_mut().stop_polling();
            if let Some(id) = s.poll_source.take() {
                id.remove();
            }
            s.pool.shutdown_all();
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

        let preferences = Preferences::load();
        let theme_name = preferences
            .theme
            .clone()
            .unwrap_or_else(|| cfg.theme.name.clone());
        let theme = Theme::load(&theme_name).unwrap_or(theme);
        let config_font_size = cfg.font.size;
        let mut font = FontSettings {
            family: cfg.font.family.clone(),
            size: cfg.font.size,
        };
        if let Some(size) = preferences.font_size {
            font.size = FontSettings::clamp_size(size);
        }
        let status_mode = preferences
            .statusbar_mode
            .as_deref()
            .map(|m| StatusBarMode::from_toml(Some(m)))
            .unwrap_or_else(|| StatusBarMode::from_toml(Some(&cfg.statusbar.mode)));

        let tabs = TabBar::new(cfg.ui.tab_bar_height);
        let layout = LayoutHost::new(theme.clone(), font.clone(), uses_tmux);
        let status = StatusBar::new(status_mode, theme.clone());
        status.container.add_css_class("status-bar");

        if cfg.ui.tab_bar_at_bottom() {
            root.append(&layout.root_box);
            root.append(&status.container);
            root.append(&tabs.container);
        } else {
            root.append(&tabs.container);
            root.append(&layout.root_box);
            root.append(&status.container);
        }
        window.set_child(Some(&root));

        let keymap = KeyMap::from_bindings(&cfg.keybindings);
        let mut pool =
            ConnectionPool::new(ConnectionPoolPolicy::new(cfg.pool.max_slots.max(1) as usize));
        let startup_key = startup_connection_key(uses_tmux, session.as_deref());
        pool.acquire(startup_key.clone(), |k| {
            WarmConnectionSlot::new(k.clone(), bridge)
        });
        let qc_store = QuickConnectStore::new(user_quickconnect_path());
        let state = Rc::new(RefCell::new(UiState {
            pool,
            qc_store,
            poll_source: None,
            uses_tmux,
            font,
            config_font_size,
            theme,
            theme_name,
            tabs,
            layout,
            status,
            status_mode,
            last_status_at: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
            status_interval: Duration::from_secs(1),
            keymap,
            active_tab: 0,
            active_pane: 0,
            last_client_size: None,
            tab_gate: TabSwitchGate::new(Duration::from_millis(1500)),
            preferences,
        }));

        // tab 点击
        {
            let st = state.clone();
            state.borrow().tabs.connect_activate(move |tab_id| {
                let mut s = st.borrow_mut();
                request_switch_tab(&mut s, tab_id);
            });
        }

        // status bar 窗口按钮
        {
            let st = state.clone();
            state
                .borrow()
                .status
                .connect_window_activate(move |tab_id| {
                    let mut s = st.borrow_mut();
                    request_switch_tab(&mut s, tab_id);
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
                let _ = st.borrow_mut();
                glib::Propagation::Proceed
            });
        }

        // 首次刷新 + 窗口级 16ms 轮询（切连接后仍打到当前 active slot）
        {
            let mut s = state.borrow_mut();
            let _ = s.bridge().poll_events();
            refresh_ui(&mut s);
            report_all_pane_colours(&s);
            maybe_refresh_status(&mut s, true);
        }

        {
            let st_weak = Rc::downgrade(&state);
            let win_weak = window.downgrade();
            let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                if win_weak.upgrade().is_none() {
                    return glib::ControlFlow::Break;
                }
                let Some(st) = st_weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let mut s = st.borrow_mut();
                s.pool.poll_background_slots();
                s.pool.evict_expired();
                let events = s.bridge().poll_events();
                let mut structural = false;
                for ev in events {
                    if StateEventPolicy::requires_layout_reload(ev.type_) {
                        structural = true;
                    }
                    dispatch_event(&mut s, &ev);
                }
                sync_pane_outputs(&mut s);
                sync_window_size(&mut s);
                maybe_refresh_status(&mut s, structural);
                glib::ControlFlow::Continue
            });
            state.borrow_mut().poll_source = Some(id);
        }

        Self {
            window,
            _state: state,
        }
    }

    /// 测试用：向当前激活 pane 发送原始输入（如 `echo hi\n` / `\x04` Ctrl+D）。
    pub fn test_send_input(&self, data: &[u8]) {
        let s = self._state.borrow();
        let _ = s.bridge().send_input(s.active_pane, data);
    }

    /// 测试用：当前激活 pane 的核心输出快照。
    pub fn test_active_pane_output(&self) -> Vec<u8> {
        let s = self._state.borrow();
        s.bridge().get_pane_output(s.active_pane)
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
        let n_tabs = s.bridge().get_tabs().len();
        let n_panes = s.bridge().get_panes(s.active_tab).len();
        (n_tabs, n_panes)
    }

    /// 测试用：状态栏文案。
    pub fn test_status_text(&self) -> String {
        self._state.borrow().status.plain_text()
    }

    /// 测试用：手动轮询一次核心事件并刷新输出（不等待 16ms 定时器）。
    pub fn test_poll_once(&self) {
        let mut s = self._state.borrow_mut();
        let events = s.bridge().poll_events();
        for ev in events {
            dispatch_event(&mut s, &ev);
        }
        sync_pane_outputs(&mut s);
        maybe_refresh_status(&mut s, true);
    }
}

fn handle_action(s: &mut UiState, action: Action, window: &Window, state: &Rc<RefCell<UiState>>) {
    match action {
        Action::NewTab | Action::NewWindow => {
            s.bridge().execute(tasks::new_tab());
        }
        Action::NewPane => {
            s.bridge().execute(tasks::split_h(s.active_pane));
        }
        Action::NewPaneVertical => {
            s.bridge().execute(tasks::split_v(s.active_pane));
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
            let tabs = s.bridge().get_tabs();
            if let Some(t) = tabs.last() {
                request_switch_tab(s, t.id);
            }
        }
        Action::SwitchPaneNext => {
            s.bridge().execute(tasks::next_pane());
        }
        Action::SwitchPanePrev => {
            s.bridge().execute(tasks::prev_pane());
        }
        Action::CommandPalette => {
            open_command_palette(s, window, state);
            return;
        }
        Action::Search | Action::Unknown => {}
        Action::QuickConnect => {
            open_quick_connect(s, window, state);
            return;
        }
        Action::Quit => {
            window.close();
            return;
        }
        Action::IncreaseFontSize => adjust_font(s, 1),
        Action::DecreaseFontSize => adjust_font(s, -1),
        Action::ResetFontSize => reset_font(s),
        Action::TogglePaneFullscreen => toggle_fullscreen(s),
    }
    refresh_ui(s);
}

fn open_command_palette(s: &UiState, window: &Window, state: &Rc<RefCell<UiState>>) {
    let parent = window.clone();
    let callback_parent = parent.clone();
    let callback_window = parent.clone();
    let callback_state = state.clone();
    let uses_tmux = s.uses_tmux;
    crate::platform::linux::command_palette::show_for_backend(&parent, uses_tmux, move |id| {
        run_palette_command(&callback_state, &callback_window, &callback_parent, id);
    });
}

fn run_palette_command(state: &Rc<RefCell<UiState>>, window: &Window, parent: &Window, id: &str) {
    match id {
        "language" => {
            let language_parent = parent.clone();
            let callback_state = state.clone();
            crate::platform::linux::command_palette::show_language(&language_parent, move |_| {
                let mut s = callback_state.borrow_mut();
                maybe_refresh_status(&mut s, true);
            });
        }
        "tmux_detach" => {
            let rc = state.borrow().bridge().detach();
            if rc == 0 {
                window.close();
            }
        }
        "quick_connect" => {
            let mut s = state.borrow_mut();
            open_quick_connect(&mut s, window, state);
        }
        "theme" => {
            let mut s = state.borrow_mut();
            toggle_theme(&mut s);
        }
        "statusbar_mode" => {
            let mut s = state.borrow_mut();
            toggle_status_mode(&mut s);
        }
        "toggle_pane_fullscreen" => {
            let mut s = state.borrow_mut();
            toggle_fullscreen(&mut s);
            refresh_ui(&mut s);
        }
        "increase_font_size" => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, 1);
        }
        "decrease_font_size" => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, -1);
        }
        "reset_font_size" => {
            let mut s = state.borrow_mut();
            reset_font(&mut s);
        }
        "quit" => window.close(),
        "new_tab" | "new_window" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::new_tab());
            refresh_ui(&mut s);
        }
        "new_pane" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::split_h(s.active_pane));
            refresh_ui(&mut s);
        }
        "new_pane_vertical" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::split_v(s.active_pane));
            refresh_ui(&mut s);
        }
        "close_pane" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::close_pane(s.active_pane));
            refresh_ui(&mut s);
        }
        "close_tab" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::close_tab(s.active_tab));
            refresh_ui(&mut s);
        }
        "close_window" => window.close(),
        "switch_pane_next" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::next_pane());
            refresh_ui(&mut s);
        }
        "switch_pane_prev" => {
            let mut s = state.borrow_mut();
            s.bridge().execute(tasks::prev_pane());
            refresh_ui(&mut s);
        }
        id if id.starts_with("switch_tab_") => {
            if let Ok(n) = id.trim_start_matches("switch_tab_").parse::<usize>() {
                let mut s = state.borrow_mut();
                switch_tab_n(&mut s, n);
            }
        }
        _ => {}
    }
}

fn toggle_fullscreen(s: &mut UiState) {
    let pane = s.active_pane;
    if s.uses_tmux {
        s.bridge().execute(tasks::toggle_pane_fullscreen(pane));
    } else {
        let next = match s.layout.fullscreen_pane() {
            Some(id) if id == pane => None,
            _ => Some(pane),
        };
        s.layout.set_fullscreen_pane(next);
    }
}

fn adjust_font(s: &mut UiState, direction: i32) {
    let next = FontSettings::zoomed(s.font.size, direction);
    if (next - s.font.size).abs() < f32::EPSILON {
        return;
    }
    s.font.size = next;
    s.layout.set_font_size(next);
    s.preferences.font_size = Some(next);
    s.preferences.save();
}

fn reset_font(s: &mut UiState) {
    s.font.size = s.config_font_size;
    let font = s.font.clone();
    s.layout.set_font(&font);
    s.preferences.font_size = None;
    s.preferences.save();
}

fn toggle_theme(s: &mut UiState) {
    let next_name = if s.theme_name == "dark" {
        "light"
    } else {
        "dark"
    };
    let Ok(theme) = Theme::load(next_name) else {
        return;
    };
    s.theme_name = next_name.to_string();
    s.theme = theme.clone();
    s.layout.apply_theme(&theme);
    s.status.apply_theme(&theme);
    s.preferences.theme = Some(next_name.to_string());
    s.preferences.save();
    report_all_pane_colours(s);
}

fn toggle_status_mode(s: &mut UiState) {
    let next = match s.status_mode {
        StatusBarMode::Tmux => StatusBarMode::Theme,
        StatusBarMode::Theme => StatusBarMode::Tmux,
    };
    s.status_mode = next;
    s.status.set_mode(next);
    s.preferences.statusbar_mode = Some(next.as_str().to_string());
    s.preferences.save();
    maybe_refresh_status(s, true);
}

fn report_all_pane_colours(s: &UiState) {
    if !s.uses_tmux {
        return;
    }
    let fg = rgb_hex(s.theme.foreground);
    let bg = rgb_hex(s.theme.background);
    let _ = s.bridge().report_all_pane_colours(&fg, &bg);
}

fn switch_tab_n(s: &mut UiState, n: usize) {
    let tabs = s.bridge().get_tabs();
    if let Some(t) = tabs.get(n.saturating_sub(1)) {
        request_switch_tab(s, t.id);
    }
}

fn request_switch_tab(s: &mut UiState, tab_id: u32) {
    if tab_id == s.active_tab {
        return;
    }
    s.tab_gate.request(tab_id);
    s.bridge().execute(tasks::switch_tab(tab_id));
}

fn dispatch_event(s: &mut UiState, ev: &BridgeEvent) {
    use crate::core::protocol::ffi::types::*;
    match ev.type_ {
        STATE_PANE_OUTPUT => {
            if let Some(view) = s.layout.pane(ev.pane_id).cloned() {
                if view.is_seeded() {
                    view.feed_output(&ev.data);
                } else {
                    // 首次输出前先按后端尺寸播种完整快照，避免增量叠在空模型上。
                    let out = s.bridge().get_pane_output(ev.pane_id);
                    let panes = s.bridge().get_panes(s.active_tab);
                    let (cols, rows) = panes
                        .iter()
                        .find(|p| p.id == ev.pane_id)
                        .map(|p| (p.cols, p.rows))
                        .unwrap_or((80, 24));
                    view.seed_snapshot(&out, cols, rows);
                }
                forward_parser_replies(s, ev.pane_id);
            }
        }
        STATE_ACTIVE_TAB_CHANGED => {
            s.tab_gate.on_tab_changed(ev.tab_id);
            s.active_tab = ev.tab_id;
            refresh_ui(s);
        }
        STATE_ACTIVE_PANE_CHANGED => {
            s.active_pane = ev.pane_id;
        }
        STATE_TAB_CLOSED => {
            s.tab_gate.on_tab_closed(ev.tab_id);
            refresh_ui(s);
        }
        STATE_TAB_ADDED | STATE_LAYOUT_CHANGED | STATE_PANE_ADDED | STATE_PANE_CLOSED => {
            if StateEventPolicy::should_reload_ui(ev.type_, ev.tab_id, s.active_tab) {
                refresh_ui(s);
            }
        }
        STATE_BACKEND_STATUS => {
            if ev.pane_id == 4 {
                // exited
                tracing::info!(target = "muxterm::linux", "backend exited");
            }
            maybe_refresh_status(s, true);
        }
        _ => {}
    }
}

fn refresh_ui(s: &mut UiState) {
    let tabs = s.bridge().get_tabs();
    s.tabs.set_tabs(&tabs);
    let tab_ids: Vec<u32> = tabs.iter().map(|t| t.id).collect();
    s.tab_gate.on_snapshot(&tab_ids);
    if let Some(active) = tabs.iter().find(|t| t.is_active) {
        s.active_tab = active.id;
    } else if let Some(first) = tabs.first() {
        s.active_tab = first.id;
    }

    if !s.tab_gate.is_released() {
        return;
    }

    // 重建布局
    if let Some(layout) = s.bridge().get_layout(s.active_tab) {
        let bridge_ptr = s.bridge() as *const CoreBridge;
        let input_cb = move |pane_id: u32, data: &[u8]| {
            // Safety: GTK 主线程，bridge 与窗口同寿
            let bridge = unsafe { &*bridge_ptr };
            let _ = bridge.send_input(pane_id, data);
        };
        s.layout.apply_layout(&layout, &input_cb);

        for pane in s.bridge().get_panes(s.active_tab) {
            if let Some(view) = s.layout.pane(pane.id).cloned() {
                if !view.is_seeded() {
                    let out = s.bridge().get_pane_output(pane.id);
                    view.seed_snapshot(&out, pane.cols, pane.rows);
                    forward_parser_replies(s, pane.id);
                }
                if pane.is_active {
                    s.active_pane = pane.id;
                    view.grab_focus();
                }
            }
        }
    }

    maybe_refresh_status(s, true);
}

fn local_status_snapshot(npanes: usize) -> StatusBarSnapshot {
    let connected = i18n::tr(Key::StatusConnected);
    let panes = i18n::tr(Key::Panes);
    let close_hint = i18n::tr(Key::WindowCloseHint);
    StatusBarSnapshot {
        enabled: true,
        position: "bottom".into(),
        justify: "left".into(),
        interval: 1,
        left: format!("{connected} | {npanes} {panes}"),
        right: close_hint,
        left_length: 40,
        right_length: 40,
        status_style: String::new(),
        left_style: String::new(),
        right_style: String::new(),
        separator: " ".into(),
        window_format: String::new(),
        window_current_format: String::new(),
        window_style: String::new(),
        window_current_style: String::new(),
        windows: Vec::new(),
        error: None,
    }
}

fn maybe_refresh_status(s: &mut UiState, force: bool) {
    if !s.uses_tmux {
        let npanes = s.bridge().get_panes(s.active_tab).len();
        s.status.apply(&local_status_snapshot(npanes));
        return;
    }
    let now = Instant::now();
    if !force && now.duration_since(s.last_status_at) < s.status_interval {
        return;
    }
    s.last_status_at = now;
    if let Some(snap) = s.bridge().status_snapshot() {
        let secs = snap.interval.max(1);
        s.status_interval = Duration::from_secs(secs);
        s.status.apply(&snap);
    }
}

fn sync_pane_outputs(s: &mut UiState) {
    // 只给尚未播种的 pane 补一次快照；已挂载 pane 的增量走 STATE_PANE_OUTPUT。
    for pane in s.bridge().get_panes(s.active_tab) {
        if let Some(view) = s.layout.pane(pane.id).cloned() {
            if view.is_seeded() {
                continue;
            }
            let out = s.bridge().get_pane_output(pane.id);
            view.seed_snapshot(&out, pane.cols, pane.rows);
            forward_parser_replies(s, pane.id);
        }
    }
}

fn forward_parser_replies(s: &UiState, pane_id: u32) {
    // tmux/SSH 镜像模式由 refresh-client -r 代答 OSC/DA，不能把 VTE 解析器
    // 应答写回 PTY，否则 `git lg` 一类查询会把 ESC 字面泄漏进输出。
    let replies = match s.layout.pane(pane_id) {
        Some(view) if view.is_tmux_mirror() || s.uses_tmux => {
            let _ = view.take_replies();
            Vec::new()
        }
        Some(view) => view.take_replies(),
        None => Vec::new(),
    };
    if !replies.is_empty() {
        let _ = s.bridge().send_input(pane_id, &replies);
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
        let _ = s.bridge().resize_client(cols, rows);
    } else {
        let _ = s.bridge().resize_pane(s.active_pane, cols, rows);
    }
}

fn open_quick_connect(s: &mut UiState, window: &Window, state: &Rc<RefCell<UiState>>) {
    s.qc_store.replace_recents(&s.pool.recent_target_configs(5));
    let current = s.pool.current_target_config();
    let store = s.qc_store.clone();
    let win = window.clone();
    let st = state.clone();
    crate::platform::linux::quickconnect_panel::show(
        &win,
        &store,
        current,
        QuickConnectCallbacks {
            on_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |cfg| {
                    connect_target(&st, cfg);
                })
            },
            on_edit: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move |cfg| {
                    open_target_config(&st, &win, Some(cfg));
                })
            },
            on_new_project: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move || {
                    open_target_config(&st, &win, None);
                })
            },
        },
    );
}

fn open_target_config(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    editing: Option<TargetConfig>,
) {
    let store = state.borrow().qc_store.clone();
    let hosts = CoreBridge::discover_ssh_hosts().unwrap_or_default();
    let st = state.clone();
    let win = window.clone();
    crate::platform::linux::target_config_window::show(
        window,
        editing,
        store,
        hosts,
        {
            let st = st.clone();
            let win = win.clone();
            move |saved| {
                let mut s = st.borrow_mut();
                s.qc_store.upsert_project(&saved);
                open_quick_connect(&mut s, &win, &st);
            }
        },
        {
            let st = st.clone();
            let win = win.clone();
            move || {
                let mut s = st.borrow_mut();
                open_quick_connect(&mut s, &win, &st);
            }
        },
    );
}

fn connect_target(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    match config.runtime {
        TargetRuntime::Tmux => run_project_flow(state, config),
        TargetRuntime::Shell => {
            if config.transport.is_ssh() {
                run_project_flow(state, config);
            } else {
                start_local_shell(state, config);
            }
        }
    }
}

fn start_local_shell(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    let session =
        crate::platform::linux::quickconnect::model::QuickConnect::default_name(&config.path);
    let key = connection_key(&config, &session);
    {
        let mut s = state.borrow_mut();
        if s.pool.get(&key).is_some() {
            activate_existing(&mut s, key);
            return;
        }
    }
    match CoreBridge::connect("local", None, None, None, Some(&config.path)) {
        Ok(bridge) => {
            let mut s = state.borrow_mut();
            activate_new(&mut s, key, bridge);
        }
        Err(e) => tracing::error!(target = "muxterm::linux", "local shell 连接失败: {e}"),
    }
}

fn run_project_flow(state: &Rc<RefCell<UiState>>, config: TargetConfig) {
    let flow = ProjectConnectFlow::new(&config);
    step_project_flow(state, config, flow);
}

fn step_project_flow(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    mut flow: ProjectConnectFlow,
) {
    match flow.state.clone() {
        ProjectConnectState::AttachExisting { session } => {
            attach_tmux(state, config, session, flow, true);
        }
        ProjectConnectState::CreateDetached { session, directory } => {
            let (backend, target) = transport_backend(&config);
            match CoreBridge::create_tmux_session(
                backend,
                target.as_deref(),
                None,
                &session,
                &directory,
            ) {
                Ok(_) => {
                    flow.create_succeeded();
                    step_project_flow(state, config, flow);
                }
                Err(e) => {
                    flow.create_failed(&e.to_string());
                    tracing::error!(target = "muxterm::linux", "create session failed: {e}");
                }
            }
        }
        ProjectConnectState::AttachCreated { session } => {
            attach_tmux(state, config, session, flow, false);
        }
        ProjectConnectState::Done => {}
        ProjectConnectState::Failed(failure) => {
            tracing::error!(
                target = "muxterm::linux",
                "project connect failed at {:?}: {}",
                failure.stage,
                failure.detail
            );
        }
    }
}

fn transport_backend(config: &TargetConfig) -> (&'static str, Option<String>) {
    match &config.transport {
        crate::platform::linux::quickconnect::model::TargetTransport::Local => ("tmux", None),
        crate::platform::linux::quickconnect::model::TargetTransport::Ssh { name } => {
            ("tmux-ssh", Some(name.clone()))
        }
    }
}

fn attach_tmux(
    state: &Rc<RefCell<UiState>>,
    config: TargetConfig,
    session: String,
    mut flow: ProjectConnectFlow,
    existing: bool,
) {
    let key = connection_key(&config, &session);
    {
        let mut s = state.borrow_mut();
        if s.pool.get(&key).is_some() {
            activate_existing(&mut s, key);
            if existing {
                flow.attach_existing_succeeded();
            } else {
                flow.attach_created_succeeded();
            }
            return;
        }
    }
    let (backend, alias) = transport_backend(&config);
    match CoreBridge::connect(
        backend,
        None,
        Some(&session),
        alias.as_deref(),
        Some(&config.path),
    ) {
        Ok(bridge) => {
            let mut s = state.borrow_mut();
            activate_new(&mut s, key, bridge);
            if existing {
                flow.attach_existing_succeeded();
            } else {
                flow.attach_created_succeeded();
            }
        }
        Err(e) => {
            if existing {
                flow.attach_existing_failed(&e.to_string());
                step_project_flow(state, config, flow);
            } else {
                flow.attach_created_failed(&e.to_string());
                tracing::error!(
                    target = "muxterm::linux",
                    "attach created session failed: {e}"
                );
            }
        }
    }
}

fn activate_existing(
    s: &mut UiState,
    key: crate::platform::linux::quickconnect::pool::ConnectionKey,
) {
    s.pool.acquire(key, |_| unreachable!("slot 已存在"));
    after_activate(s);
}

fn activate_new(
    s: &mut UiState,
    key: crate::platform::linux::quickconnect::pool::ConnectionKey,
    bridge: CoreBridge,
) {
    s.pool
        .acquire(key.clone(), |_| WarmConnectionSlot::new(key, bridge));
    after_activate(s);
}

fn after_activate(s: &mut UiState) {
    let uses = s.bridge().uses_tmux();
    s.uses_tmux = uses;
    s.layout.reset(uses);
    s.tab_gate = TabSwitchGate::new(Duration::from_millis(1500));
    s.last_client_size = None;
    s.qc_store.replace_recents(&s.pool.recent_target_configs(5));
    refresh_ui(s);
    report_all_pane_colours(s);
    maybe_refresh_status(s, true);
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
        .muxterm-status-window { padding: 0 6px; min-height: 18px; }
        .qc-badge { padding: 0 6px; border-radius: 4px; font-size: 9px; color: #fff; }
        .qc-badge-recent { background: #1e66f5; }
        .qc-badge-project { background: #40a02b; }
        .qc-badge-current { background: #df8e1d; }
        .qc-current { background: alpha(#89b4fa, 0.18); }
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
