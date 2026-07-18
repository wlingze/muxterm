//! 主窗口。
//!
//! - 顶部：只有 Notebook tab 栏（无按钮）。
//! - 中间：当前 tab 的 pane 区域（本地 shell 可分割；tmux pane 1:1）。
//! - 底部：输入栏（仅 tmux pane 显示）+ 状态栏。
//!
//! 快捷键（EventControllerKey 绑在 window 上）：
//! Alt+N/T/D/Shift+D/1-9/0/[ ]/R/P，详见 `configs/config.example.toml`。
//!
//! 启动即一个本地 shell tab。shell 退出 → 自动关 tab；最后一个 tab 关 → 新开
//! 空 shell tab。tmux 是可选的 attach（Alt+P → tmux_attach / tmux_new）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Box, EventControllerKey, Label, Orientation, Window};
use vte4::prelude::*;

use crate::config::{Action, Config, Theme};
use crate::tmux::client::TmuxClientConfig;
use crate::tmux::command::{kill_pane as tmux_kill_pane, send_keys, Key, PaneId as CmdPaneId};
use crate::tmux::protocol::PaneId;
use crate::ui::command_palette;
use crate::ui::input_bar::InputBar;
use crate::ui::keymap::KeyMap;
use crate::ui::notebook::{LocalPaneId, PaneKey, PaneNotebook, TabContent, TabKey};
use crate::ui::pane_view::{PaneMode, PaneView};
use crate::ui::tmux_dialog::{self, TmuxAction};
use crate::ui::wiring::{spawn_bridge, CommandSender, UiEvent};

pub struct AppWindow {
    pub window: Window,
    shared: Arc<SharedState>,
}

struct SharedState {
    notebook: Arc<RwLock<PaneNotebook>>,
    /// 所有 pane key → PaneView（持有 terminal，用于 feed / child-exited 查找）。
    pane_views: Arc<RwLock<HashMap<PaneKey, PaneView>>>,
    /// 本地 tab key → 该 tab 内的 pane keys（用于 child-exited 时定位 tab）。
    tab_panes: Arc<RwLock<HashMap<TabKey, Vec<PaneKey>>>>,
    /// tmux pane id → window id（tmux 侧 window/pane 关系）。
    pane_window: Arc<RwLock<HashMap<u32, u32>>>,
    /// tmux pane id → tab key（用于 close 反查）。
    pane_tab: Arc<RwLock<HashMap<u32, TabKey>>>,
    session_name: Arc<RwLock<Option<String>>>,
    cfg: Arc<Config>,
    theme: Arc<Theme>,
    keymap: Arc<KeyMap>,
    cmd_sender: Arc<Mutex<Option<CommandSender>>>,
    current_tab: Arc<Mutex<Option<TabKey>>>,
    current_pane: Arc<Mutex<Option<PaneKey>>>,
    input_bar: Arc<InputBar>,
    input_bar_container: gtk4::Box,
    status_label: Label,
    window: Window,
}

impl AppWindow {
    pub fn new(config: Config, theme: Theme) -> Self {
        let window = Window::builder()
            .title("muxterm")
            .default_width(1000)
            .default_height(650)
            .build();

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        let notebook = Arc::new(RwLock::new(PaneNotebook::new()));
        root.append(&notebook.read().unwrap().notebook);

        let input_bar_container = Box::builder().orientation(Orientation::Horizontal).build();
        let input_bar = Arc::new(InputBar::new());
        input_bar_container.append(&input_bar.container);
        input_bar_container.set_visible(false);
        root.append(&input_bar_container);

        let status_label = Label::builder()
            .label("状态：本地 shell")
            .halign(gtk4::Align::Start)
            .margin_start(6)
            .margin_end(6)
            .margin_top(2)
            .margin_bottom(2)
            .build();
        status_label.add_css_class("status-bar");
        root.append(&status_label);

        window.set_child(Some(&root));

        let keymap = KeyMap::from_bindings(&config.keybindings);
        let shared = Arc::new(SharedState {
            notebook,
            pane_views: Arc::new(RwLock::new(HashMap::new())),
            tab_panes: Arc::new(RwLock::new(HashMap::new())),
            pane_window: Arc::new(RwLock::new(HashMap::new())),
            pane_tab: Arc::new(RwLock::new(HashMap::new())),
            session_name: Arc::new(RwLock::new(None)),
            cfg: Arc::new(config),
            theme: Arc::new(theme),
            keymap: Arc::new(keymap),
            cmd_sender: Arc::new(Mutex::new(None)),
            current_tab: Arc::new(Mutex::new(None)),
            current_pane: Arc::new(Mutex::new(None)),
            input_bar,
            input_bar_container,
            status_label,
            window: window.clone(),
        });

        let app_win = AppWindow {
            window: window.clone(),
            shared: shared.clone(),
        };

        app_win.wire_notebook_switch();
        app_win.wire_input_bar();
        app_win.wire_global_key_events();

        // 启动即一个本地 shell
        app_win.new_local_tab();

        app_win
    }

    /// Notebook 切 tab 回调。
    fn wire_notebook_switch(&self) {
        let shared = self.shared.clone();
        let nbook = self.shared.notebook.read().unwrap().notebook.clone();
        nbook.connect_switch_page(move |_, _w, page_num| {
            SharedState::on_switch_page(&shared, page_num);
        });
    }

    /// 输入栏接线（tmux pane 用）。
    fn wire_input_bar(&self) {
        let dispatcher: Arc<dyn Fn(&str) + Send + Sync> = {
            let sender = self.shared.cmd_sender.clone();
            Arc::new(move |line: &str| {
                if let Ok(g) = sender.lock() {
                    if let Some(s) = g.as_ref() {
                        s.send(line);
                    }
                }
            })
        };
        let current_pane = self.shared.current_pane.clone();
        let current_pane_fn: Arc<dyn Fn() -> Option<PaneId> + Send + Sync> = Arc::new(move || {
            // 当前激活 pane 若是 tmux，返回 pane id
            match *current_pane.lock().unwrap() {
                Some(PaneKey::Tmux(p)) => Some(p),
                _ => None,
            }
        });
        self.shared.input_bar.wire(dispatcher, current_pane_fn);
    }

    /// 全局快捷键：EventControllerKey 绑在 window 上。
    fn wire_global_key_events(&self) {
        let shared = self.shared.clone();
        let controller = EventControllerKey::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_c, keyval, _keycode, mods| {
            if let Some(action) = shared.keymap.lookup(keyval, mods) {
                SharedState::dispatch_action(&shared, action);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(controller);
    }

    /// 新建一个本地 shell tab。
    fn new_local_tab(&self) {
        SharedState::new_local_tab(&self.shared);
    }
}

impl SharedState {
    /// 新建本地 shell tab（1 个 pane）。
    fn new_local_tab(self: &Arc<Self>) -> (TabKey, PaneKey) {
        let view = PaneView::new_local(
            &self.theme,
            &self.cfg.font.family,
            self.cfg.font.size,
            self.cfg.scrollback.lines,
        );
        let term = view.terminal.clone();
        let (tab_key, pane_key) = {
            let mut nb = self.notebook.write().unwrap();
            nb.add_local_tab(&view, "shell")
        };
        // 注册 child-exited：shell 退出 → 关 pane/tab
        let shared = self.clone();
        let tab_for_cb = tab_key;
        let pane_for_cb = pane_key;
        term.connect_child_exited(move |_t, _status| {
            shared.on_local_pane_exited(tab_for_cb, pane_for_cb);
        });
        self.pane_views.write().unwrap().insert(pane_key, view);
        self.tab_panes
            .write()
            .unwrap()
            .insert(tab_key, vec![pane_key]);
        *self.current_tab.lock().unwrap() = Some(tab_key);
        *self.current_pane.lock().unwrap() = Some(pane_key);
        self.input_bar_container.set_visible(false);
        (tab_key, pane_key)
    }

    /// 本地 pane 子进程退出：移除该 pane；若 tab 内无 pane 则关 tab；
    /// 若最后一个 tab 被关则新开一个空 shell tab。
    fn on_local_pane_exited(self: &Arc<Self>, tab: TabKey, pane: PaneKey) {
        tracing::info!(target = "muxterm::window", ?tab, ?pane, "本地 pane 退出");
        // 从 tab_panes 移除该 pane
        let mut tp = self.tab_panes.write().unwrap();
        if let Some(panes) = tp.get_mut(&tab) {
            panes.retain(|p| *p != pane);
        }
        let remaining: Vec<PaneKey> = tp.get(&tab).cloned().unwrap_or_default();
        let tab_empty = remaining.is_empty();
        drop(tp);

        // 移除 pane view
        self.pane_views.write().unwrap().remove(&pane);

        if tab_empty {
            // 关 tab
            self.notebook.write().unwrap().remove(tab);
            self.tab_panes.write().unwrap().remove(&tab);
            // 若是最后一个 tab → 新开空 shell
            let n = self.notebook.read().unwrap().n_tabs();
            if n == 0 {
                self.new_local_tab();
            } else {
                // 切到第一个 tab
                self.notebook.read().unwrap().select_by_index(0);
            }
        } else {
            // tab 内还有 pane，rebuild
            self.rebuild_local_tab(tab);
        }
        self.refresh_input_visibility();
    }

    /// 重建本地 tab 的分割布局（pane 增减后）。
    fn rebuild_local_tab(self: &Arc<Self>, tab: TabKey) {
        let panes: Vec<PaneKey> = self
            .tab_panes
            .read()
            .unwrap()
            .get(&tab)
            .cloned()
            .unwrap_or_default();
        if panes.is_empty() {
            return;
        }
        let terminals: HashMap<PaneKey, vte4::Terminal> = {
            let views = self.pane_views.read().unwrap();
            panes
                .iter()
                .filter_map(|p| views.get(p).map(|v| (*p, v.terminal.clone())))
                .collect()
        };
        {
            let mut nb = self.notebook.write().unwrap();
            // 设 orientation：取该 tab 第一个 PaneContent 已存的，或默认水平
            // 简化：split 时设了 orientation，这里读不到——用 HashMap 外存？
            // 这里用 tab_panes 的顺序 + 默认水平。split 时若改过方向需更新。
            nb.rebuild_local_root(tab, &terminals);
            nb.relayout_local_tab(tab, "shell");
        }
    }

    /// 分割当前激活的本地 pane，加一个新 pane。
    fn split_current_pane(self: &Arc<Self>, vertical: bool) {
        let tab = match *self.current_tab.lock().unwrap() {
            Some(TabKey::Local(_)) => self.current_tab.lock().unwrap().unwrap(),
            _ => {
                tracing::info!(target: "muxterm::window", "split 仅对本地 tab 有效");
                return;
            }
        };
        // 新建一个本地 shell pane
        let view = PaneView::new_local(
            &self.theme,
            &self.cfg.font.family,
            self.cfg.font.size,
            self.cfg.scrollback.lines,
        );
        let term = view.terminal.clone();
        // 新 pane key
        let new_pane = {
            let panes = self.tab_panes.read().unwrap();
            let max = panes
                .get(&tab)
                .unwrap()
                .iter()
                .filter_map(|p| match p {
                    PaneKey::Local(LocalPaneId(n)) => Some(*n),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                + 1;
            PaneKey::Local(LocalPaneId(max))
        };
        // 注册 child-exited
        let shared = self.clone();
        let tab_cb = tab;
        let pane_cb = new_pane;
        term.connect_child_exited(move |_t, _s| {
            shared.on_local_pane_exited(tab_cb, pane_cb);
        });
        self.pane_views.write().unwrap().insert(new_pane, view);
        self.tab_panes
            .write()
            .unwrap()
            .get_mut(&tab)
            .map(|v| v.push(new_pane));
        // 设 orientation
        let orient = if vertical {
            gtk4::Orientation::Vertical
        } else {
            gtk4::Orientation::Horizontal
        };
        // 在 notebook 里更新 TabContent orientation 并 rebuild
        {
            let mut nb = self.notebook.write().unwrap();
            if let Some((_idx, Some(content))) = nb.tabs.get_mut(&tab) {
                content.orientation = orient;
            }
            let terminals: HashMap<PaneKey, vte4::Terminal> = {
                let views = self.pane_views.read().unwrap();
                let panes = self.tab_panes.read().unwrap();
                panes
                    .get(&tab)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| views.get(p).map(|v| (*p, v.terminal.clone())))
                    .collect()
            };
            nb.rebuild_local_root(tab, &terminals);
            nb.relayout_local_tab(tab, "shell");
        }
        *self.current_pane.lock().unwrap() = Some(new_pane);
    }

    /// 切换 tab（按数字 1-9 / 最后）。
    fn switch_tab_n(self: &Arc<Self>, n: u32) {
        let nb = self.notebook.read().unwrap();
        let total = nb.n_tabs();
        if total == 0 {
            return;
        }
        let idx = if n == 0 { total - 1 } else { n.min(total) - 1 };
        nb.select_by_index(idx);
    }

    /// 切换当前 tab 内的 pane（前/后）。
    fn switch_pane(self: &Arc<Self>, next: bool) {
        let tab = self.current_tab.lock().unwrap();
        let tab = match *tab {
            Some(t) => t,
            None => return,
        };
        let panes = self.tab_panes.read().unwrap().get(&tab).cloned();
        let Some(panes) = panes else {
            return;
        };
        if panes.len() <= 1 {
            return;
        }
        let cur = self.current_pane.lock().unwrap();
        let idx = panes.iter().position(|p| Some(*p) == *cur).unwrap_or(0);
        let new_idx = if next {
            (idx + 1) % panes.len()
        } else if idx == 0 {
            panes.len() - 1
        } else {
            idx - 1
        };
        *self.current_pane.lock().unwrap() = Some(panes[new_idx]);
    }

    /// 执行一个 action。
    fn dispatch_action(self: &Arc<Self>, action: Action) {
        match action {
            Action::NewWindow => {
                self.new_local_tab();
            }
            Action::NewTab => {
                self.new_local_tab();
            }
            Action::NewPane => self.split_current_pane(false),
            Action::NewPaneVertical => self.split_current_pane(true),
            Action::SwitchTab1 => self.switch_tab_n(1),
            Action::SwitchTab2 => self.switch_tab_n(2),
            Action::SwitchTab3 => self.switch_tab_n(3),
            Action::SwitchTab4 => self.switch_tab_n(4),
            Action::SwitchTab5 => self.switch_tab_n(5),
            Action::SwitchTab6 => self.switch_tab_n(6),
            Action::SwitchTab7 => self.switch_tab_n(7),
            Action::SwitchTab8 => self.switch_tab_n(8),
            Action::SwitchTab9 => self.switch_tab_n(9),
            Action::SwitchTabLast => self.switch_tab_n(0),
            Action::SwitchPanePrev => self.switch_pane(false),
            Action::SwitchPaneNext => self.switch_pane(true),
            Action::Search => command_palette::show_search(&self.window),
            Action::CommandPalette => {
                let shared = self.clone();
                command_palette::show(&self.window, move |cmd| {
                    SharedState::run_palette_command(&shared, cmd);
                });
            }
            Action::Unknown => {}
        }
    }

    /// 命令面板执行。
    fn run_palette_command(self: &Arc<Self>, cmd: &str) {
        match cmd {
            "new_window" | "new_tab" => {
                self.new_local_tab();
            }
            "new_pane" => self.split_current_pane(false),
            "new_pane_vertical" => self.split_current_pane(true),
            "tmux_attach" => {
                let shared = self.clone();
                let win = self.window.clone();
                tmux_dialog::show(&win, move |action| {
                    SharedState::do_tmux_action(&shared, action);
                });
            }
            "tmux_new" => {
                self.do_tmux_action(TmuxAction::NewSession { name: None });
            }
            "search" => command_palette::show_search(&self.window),
            "quit" => self.window.close(),
            other => tracing::info!(target = "muxterm::window", cmd = %other, "未知命令"),
        }
    }

    fn do_tmux_action(self: &Arc<Self>, action: TmuxAction) {
        let (config, auto_mouse) = match action {
            TmuxAction::Attach { session } => {
                let cfg = crate::ui::wiring::attach_config(&session);
                (cfg, self.cfg.tmux.auto_mouse)
            }
            TmuxAction::NewSession { name } => {
                let cfg = crate::ui::wiring::new_session_config(name);
                (cfg, self.cfg.tmux.auto_mouse)
            }
        };
        self.connect_tmux(config, auto_mouse);
    }

    fn connect_tmux(self: &Arc<Self>, config: TmuxClientConfig, auto_mouse: bool) {
        let shared = self.clone();
        let on_event = move |ev: &UiEvent| shared.handle_ui_event(ev);
        if let Some(sender) = spawn_bridge(config, auto_mouse, on_event) {
            if let Ok(mut g) = self.cmd_sender.lock() {
                *g = Some(sender);
            }
        }
    }

    /// 处理一条 tmux UI 事件（UI 线程）。
    fn handle_ui_event(self: &Arc<Self>, ev: &UiEvent) {
        match ev {
            UiEvent::Connected => {
                self.status_label
                    .set_label("状态：已连接 tmux | 等待 pane…");
            }
            UiEvent::Error { msg } => {
                self.status_label.set_label("状态：tmux 连接失败");
                let dlg = gtk4::MessageDialog::builder()
                    .transient_for(&self.window)
                    .modal(true)
                    .buttons(gtk4::ButtonsType::Ok)
                    .text("连接 tmux 失败")
                    .secondary_text(msg)
                    .build();
                let d = dlg.clone();
                dlg.connect_response(move |_, _| {
                    d.close();
                });
                dlg.show();
            }
            UiEvent::PaneOutput { pane, data } => {
                let key = TabKey::Tmux(*pane);
                let pane_key = PaneKey::Tmux(*pane);
                if !self.pane_views.read().unwrap().contains_key(&pane_key) {
                    let view = PaneView::new_tmux(
                        *pane,
                        &self.theme,
                        &self.cfg.font.family,
                        self.cfg.font.size,
                        self.cfg.scrollback.lines,
                    );
                    let title = PaneNotebook::default_title(key, None);
                    let k = self.notebook.write().unwrap().add_tmux_tab(&view, &title);
                    self.pane_views.write().unwrap().insert(pane_key, view);
                    self.pane_tab.write().unwrap().insert(pane.0, k);
                }
                if let Some(view) = self.pane_views.read().unwrap().get(&pane_key) {
                    view.feed_output(data);
                }
            }
            UiEvent::WindowAdd { .. } => {}
            UiEvent::WindowClose { window } => {
                let pw = self.pane_window.read().unwrap();
                let to_remove: Vec<PaneId> = pw
                    .iter()
                    .filter_map(|(p, w)| {
                        if *w == window.0 {
                            Some(PaneId(*p))
                        } else {
                            None
                        }
                    })
                    .collect();
                drop(pw);
                for p in to_remove {
                    let tab = self.pane_tab.read().unwrap().get(&p.0).copied();
                    if let Some(t) = tab {
                        self.notebook.write().unwrap().remove(t);
                        self.pane_views.write().unwrap().remove(&PaneKey::Tmux(p));
                    }
                    self.pane_tab.write().unwrap().remove(&p.0);
                }
                self.refresh_input_visibility();
            }
            UiEvent::WindowRenamed { window, name } => {
                let pw = self.pane_window.read().unwrap();
                if let Some(pane) = pw.iter().find(|(_, w)| **w == window.0).map(|(p, _)| *p) {
                    let tab = self.pane_tab.read().unwrap().get(&pane).copied();
                    if let Some(t) = tab {
                        self.notebook.read().unwrap().set_title(t, name);
                    }
                }
            }
            UiEvent::SessionChanged { sid, name } => {
                *self.session_name.write().unwrap() = name.clone();
                let title = match name {
                    Some(n) => format!("muxterm — session: {n}"),
                    None => format!("muxterm — session: ${sid}"),
                };
                self.window.set_title(Some(&title));
                let n = self.notebook.read().unwrap().tab_count();
                let nm = name.clone().unwrap_or_else(|| format!("${sid}"));
                self.status_label
                    .set_label(&format!("状态：已连接 | session: {nm} | tabs: {n}"));
            }
            UiEvent::Exit { reason } => {
                let msg = match reason {
                    Some(r) => format!("状态：tmux 已断开（{r}）"),
                    None => "状态：tmux 已断开".to_string(),
                };
                self.status_label.set_label(&msg);
            }
        }
    }

    fn on_switch_page(self: &Arc<Self>, page_num: u32) {
        let nb = match self.notebook.try_read() {
            Ok(g) => g,
            Err(_) => return,
        };
        let key = nb.find_key_by_index(page_num);
        drop(nb);
        *self.current_tab.lock().unwrap() = key;
        // 更新当前 pane：本地 tab 取第一个 pane，tmux tab 取对应 pane
        let pane = match key {
            Some(TabKey::Tmux(p)) => Some(PaneKey::Tmux(p)),
            Some(t @ TabKey::Local(_)) => self
                .tab_panes
                .read()
                .unwrap()
                .get(&t)
                .and_then(|ps| ps.first().copied()),
            None => None,
        };
        *self.current_pane.lock().unwrap() = pane;
        self.refresh_input_visibility();
    }

    fn refresh_input_visibility(self: &Arc<Self>) {
        let key = self.current_pane.lock().unwrap().clone();
        let is_tmux = matches!(key, Some(PaneKey::Tmux(_)));
        if is_tmux {
            let pane = match key {
                Some(PaneKey::Tmux(p)) => Some(p),
                _ => None,
            };
            self.input_bar.set_target(pane);
            self.input_bar_container.set_visible(true);
        } else {
            self.input_bar_container.set_visible(false);
        }
    }
}

// （TabContent orientation 更新通过 pub(crate) tabs 字段直接访问）
