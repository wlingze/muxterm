//! 主窗口（极简布局 + 程序/pane 生命周期）。
//!
//! 术语：我们的 **Tab** / **Pane** ↔ tmux 的 **window** / **pane**。
//! - 本地：嵌套 GtkPaned 分割（在当前激活 pane 内切分）
//! - tmux attach：新建独立 GTK 窗口；一个 tmux window = 一个 Tab；pane 按 layout 嵌套分割
//!
//! 程序退出模型：
//! - 正常/异常退出 → 关闭对应 pane（异常可提示）
//! - tab 内无 pane → 关 tab
//! - 无 tab → 按 `behavior.on_last_pane_exit`（默认关窗）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Box, CssProvider, EventControllerKey, Label, Orientation, Window};
use vte4::prelude::*;

use crate::core::config::{
    decode_wait_status, expand_config_value, parse_command_argv, Action, Config,
    OnProgramExitAbnormal, Theme,
};
use crate::core::ssh::{parse_ssh_connect_line, SshAuth, SshConfig};
use crate::core::tmux::client::TmuxClientConfig;
use crate::core::tmux::command::{
    kill_pane as tmux_kill_pane_cmd, kill_window as tmux_kill_window_cmd, send_keys, Key,
};
use crate::core::tmux::protocol::{PaneId, WindowId};
use crate::platform::linux::command_palette;
use crate::platform::linux::input_bar::InputBar;
use crate::platform::linux::keymap::KeyMap;
use crate::platform::linux::lifecycle::{
    last_tabs_closed_action, next_pane_index, palette_should_refocus_terminal,
    pane_exit_decision, tab_index_for_shortcut, LastTabsClosedAction, PaneExitDecision,
};
use crate::platform::linux::notebook::{
    parse_layout_tree, LocalPaneId, PaneKey, PaneNotebook, SplitOrient, TabKey,
};
use crate::platform::linux::pane_switcher::{self, PaneEntry};
use crate::platform::linux::pane_view::{PaneView, SpawnOpts};
use crate::platform::linux::quick_pick::{self, QuickPickItem};
use crate::platform::linux::tab_bar::{
    format_tab_bar_title, format_tab_display_name, TabBar, TabBarItem,
};
use crate::platform::linux::tmux_dialog::{self, TmuxAction};
use crate::platform::linux::wiring::{spawn_bridge, spawn_ssh_bridge, TmuxBridge, UiEvent};

pub struct AppWindow {
    pub window: Window,
    shared: Arc<SharedState>,
}

struct SharedState {
    notebook: Arc<RwLock<PaneNotebook>>,
    /// 所有 pane key → PaneView。
    pane_views: Arc<RwLock<HashMap<PaneKey, PaneView>>>,
    /// 本地 tab key → 该 tab 内的 pane keys。
    tab_panes: Arc<RwLock<HashMap<TabKey, Vec<PaneKey>>>>,
    /// tmux pane id → window id。
    pane_window: Arc<RwLock<HashMap<u32, u32>>>,
    /// tmux pane id → tab key（TmuxWindow）。
    pane_tab: Arc<RwLock<HashMap<u32, TabKey>>>,
    /// tmux window id → 用户/协议侧窗口名。
    window_names: Arc<RwLock<HashMap<u32, String>>>,
    session_name: Arc<RwLock<Option<String>>>,
    cfg: Arc<Config>,
    theme: Arc<Theme>,
    keymap: Arc<KeyMap>,
    /// 持有 Runtime，drop 会断开 tmux；发命令用 `bridge.sender()`。
    cmd_sender: Arc<Mutex<Option<TmuxBridge>>>,
    current_tab: Arc<Mutex<Option<TabKey>>>,
    current_pane: Arc<Mutex<Option<PaneKey>>>,
    input_bar: Arc<InputBar>,
    input_bar_container: gtk4::Box,
    status_label: Label,
    tab_bar: TabBar,
    window: Window,
}

/// 窗口启动模式。
enum WindowBoot {
    /// 默认：开一个本地 shell tab。
    LocalShell,
    /// tmux session 窗口：不建本地 tab，立即 connect。
    TmuxSession(TmuxAction),
    /// SSH → 远程 tmux -CC：不建本地 tab，立即 connect。
    SshSession {
        ssh: SshConfig,
        session_name: String,
    },
}

impl AppWindow {
    pub fn new(config: Config, theme: Theme) -> Self {
        Self::new_inner(config, theme, WindowBoot::LocalShell)
    }

    /// 为 tmux attach/new-session 新建独立 GTK 窗口。
    pub fn new_tmux_session(config: Config, theme: Theme, action: TmuxAction) -> Self {
        Self::new_inner(config, theme, WindowBoot::TmuxSession(action))
    }

    fn new_inner(config: Config, theme: Theme, boot: WindowBoot) -> Self {
        let title = match &boot {
            WindowBoot::LocalShell => "muxterm".to_string(),
            WindowBoot::TmuxSession(TmuxAction::Attach { session }) => {
                format!("muxterm — tmux:{session}")
            }
            WindowBoot::TmuxSession(TmuxAction::NewSession { name }) => {
                format!("muxterm — tmux:{}", name.as_deref().unwrap_or("new"))
            }
            WindowBoot::SshSession { ssh, session_name } => {
                let sess = if session_name.is_empty() {
                    "new".to_string()
                } else {
                    session_name.clone()
                };
                format!(
                    "muxterm — ssh:{}@{}:{}/{}",
                    ssh.user, ssh.host, ssh.port, sess
                )
            }
        };
        let window = Window::builder()
            .title(&title)
            .default_width(1000)
            .default_height(650)
            .build();

        apply_css();

        if config.ui.borderless {
            tracing::debug!(
                target = "muxterm::window",
                "borderless 已配置，当前为预留项"
            );
        }

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();
        root.add_css_class("muxterm-root");
        root.set_margin_start(0);
        root.set_margin_end(0);
        root.set_margin_top(0);
        root.set_margin_bottom(0);

        let notebook = Arc::new(RwLock::new(PaneNotebook::new()));
        let nb_widget = notebook.read().unwrap().notebook.clone();
        nb_widget.set_hexpand(true);
        nb_widget.set_vexpand(true);

        let tab_bar = TabBar::new(config.ui.tab_bar_height);

        let input_bar_container = Box::builder().orientation(Orientation::Horizontal).build();
        let input_bar = Arc::new(InputBar::new());
        input_bar_container.append(&input_bar.container);
        input_bar_container.set_visible(false);

        let status_label = Label::builder()
            .label("")
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .build();
        status_label.add_css_class("status-bar");
        status_label.add_css_class("hidden");
        status_label.set_visible(false);

        if config.ui.tab_bar_at_bottom() {
            root.append(&nb_widget);
            root.append(&input_bar_container);
            root.append(&status_label);
            root.append(&tab_bar.container);
        } else {
            root.append(&tab_bar.container);
            root.append(&nb_widget);
            root.append(&input_bar_container);
            root.append(&status_label);
        }

        window.set_child(Some(&root));

        let keymap = KeyMap::from_bindings(&config.keybindings);
        let shared = Arc::new(SharedState {
            notebook,
            pane_views: Arc::new(RwLock::new(HashMap::new())),
            tab_panes: Arc::new(RwLock::new(HashMap::new())),
            pane_window: Arc::new(RwLock::new(HashMap::new())),
            pane_tab: Arc::new(RwLock::new(HashMap::new())),
            window_names: Arc::new(RwLock::new(HashMap::new())),
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
            tab_bar,
            window: window.clone(),
        });

        let app_win = AppWindow {
            window: window.clone(),
            shared: shared.clone(),
        };

        {
            let shared_click = shared.clone();
            shared.tab_bar.connect_activate(move |key| {
                let (nbook, idx) = {
                    let nb = shared_click.notebook.read().unwrap();
                    (nb.notebook.clone(), nb.tabs.get(&key).map(|(i, _)| *i))
                };
                if let Some(idx) = idx {
                    nbook.set_current_page(Some(idx));
                }
            });
        }

        app_win.wire_notebook_switch();
        app_win.wire_input_bar();
        app_win.wire_global_key_events();
        app_win.wire_title_watch();

        match boot {
            WindowBoot::LocalShell => {
                app_win.new_local_tab();
                let default_session = shared.cfg.tmux.default_session.trim().to_string();
                if !default_session.is_empty() {
                    SharedState::do_tmux_action(
                        &shared,
                        TmuxAction::Attach {
                            session: default_session,
                        },
                    );
                }
            }
            WindowBoot::TmuxSession(action) => {
                SharedState::connect_tmux_action(&shared, action);
            }
            WindowBoot::SshSession { ssh, session_name } => {
                SharedState::connect_ssh(&shared, ssh, session_name);
            }
        }

        app_win
    }

    /// 为 SSH 远程 tmux 新建独立 GTK 窗口。
    pub fn new_ssh_session(
        config: Config,
        theme: Theme,
        ssh: SshConfig,
        session_name: String,
    ) -> Self {
        Self::new_inner(config, theme, WindowBoot::SshSession { ssh, session_name })
    }

    fn wire_notebook_switch(&self) {
        let shared = self.shared.clone();
        let nbook = self.shared.notebook.read().unwrap().notebook.clone();
        nbook.connect_switch_page(move |_, _w, page_num| {
            SharedState::on_switch_page(&shared, page_num);
        });
    }

    fn wire_input_bar(&self) {
        let dispatcher: Arc<dyn Fn(&str) + Send + Sync> = {
            let sender = self.shared.cmd_sender.clone();
            Arc::new(move |line: &str| {
                if let Ok(g) = sender.lock() {
                    if let Some(bridge) = g.as_ref() {
                        bridge.sender().send(line);
                    }
                }
            })
        };
        let current_pane = self.shared.current_pane.clone();
        let current_pane_fn: Arc<dyn Fn() -> Option<PaneId> + Send + Sync> =
            Arc::new(move || match *current_pane.lock().unwrap() {
                Some(PaneKey::Tmux(p)) => Some(p),
                _ => None,
            });
        self.shared.input_bar.wire(dispatcher, current_pane_fn);
    }

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

    fn new_local_tab(&self) {
        SharedState::new_local_tab(&self.shared);
    }

    /// 每秒刷新 pane 标题（进程名变化 → tab 名更新）。
    fn wire_title_watch(&self) {
        let shared = self.shared.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            shared.refresh_pane_titles();
            glib::ControlFlow::Continue
        });
    }
}

impl SharedState {
    /// 构造本地 spawn 参数（命令 + 工作目录）。
    fn local_spawn_opts(self: &Arc<Self>, workdir_override: Option<PathBuf>) -> SpawnOpts {
        let argv = parse_command_argv(&self.cfg.pane.default_command);
        let workdir = workdir_override.or_else(|| {
            let w = expand_config_value(&self.cfg.pane.workdir);
            Some(PathBuf::from(w))
        });
        SpawnOpts { argv, workdir }
    }

    /// 新建本地程序 tab（1 个 pane）。
    fn new_local_tab(self: &Arc<Self>) -> (TabKey, PaneKey) {
        let workdir = self
            .inherit_workdir()
            .or_else(|| Some(PathBuf::from(expand_config_value(&self.cfg.pane.workdir))));
        let opts = self.local_spawn_opts(workdir);
        let view = PaneView::new_local(
            &self.theme,
            &self.cfg.font.family,
            self.cfg.font.size,
            self.cfg.scrollback.lines,
            &opts,
        );
        let title_name = view.display_name();
        let term = view.terminal.clone();
        let (tab_key, pane_key) = {
            let mut nb = self.notebook.write().unwrap();
            nb.add_local_tab(&view, &title_name)
        };
        let shared = self.clone();
        let tab_for_cb = tab_key;
        let pane_for_cb = pane_key;
        term.connect_child_exited(move |_t, status| {
            shared.on_local_pane_exited(tab_for_cb, pane_for_cb, status);
        });
        self.pane_views.write().unwrap().insert(pane_key, view);
        self.tab_panes
            .write()
            .unwrap()
            .insert(tab_key, vec![pane_key]);
        *self.current_tab.lock().unwrap() = Some(tab_key);
        *self.current_pane.lock().unwrap() = Some(pane_key);
        self.input_bar_container.set_visible(false);
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.focus_active_pane();
        (tab_key, pane_key)
    }

    /// 继承当前激活 pane 的工作目录（若可得）。
    fn inherit_workdir(self: &Arc<Self>) -> Option<PathBuf> {
        let pane = *self.current_pane.lock().unwrap();
        let pane = pane?;
        let views = self.pane_views.read().unwrap();
        views.get(&pane).and_then(|v| v.current_workdir())
    }

    /// 本地 pane 子进程退出。
    fn on_local_pane_exited(self: &Arc<Self>, tab: TabKey, pane: PaneKey, status: i32) {
        let code = decode_wait_status(status);
        let prog = self
            .pane_views
            .read()
            .unwrap()
            .get(&pane)
            .map(|v| v.display_name())
            .unwrap_or_else(|| "program".into());

        tracing::info!(
            target = "muxterm::window",
            ?tab,
            ?pane,
            code,
            %prog,
            "本地 pane 程序退出"
        );

        let policy = self.cfg.behavior.on_program_exit_abnormal;
        if pane_exit_decision(code, policy) == PaneExitDecision::Keep {
            self.show_status(&format!("{prog} exited with code {code}"));
            return;
        }

        if code != 0 && policy == OnProgramExitAbnormal::Notify {
            self.show_status(&format!("{prog} exited with code {code}"));
        }

        self.pane_views.write().unwrap().remove(&pane);

        let terminals: HashMap<PaneKey, vte4::Terminal> = {
            let views = self.pane_views.read().unwrap();
            views
                .iter()
                .map(|(k, v)| (*k, v.terminal.clone()))
                .collect()
        };
        let title = self.tab_display_name(tab);
        let tab_empty = {
            let mut nb = self.notebook.write().unwrap();
            nb.remove_pane_and_relayout(tab, pane, &terminals, &title)
        };

        if tab_empty {
            self.notebook.write().unwrap().remove(tab);
            self.tab_panes.write().unwrap().remove(&tab);
            let n = self.notebook.read().unwrap().n_tabs();
            if n == 0 {
                self.on_all_tabs_closed();
            } else {
                self.notebook.read().unwrap().select_by_index(0);
            }
        } else {
            // 同步 tab_panes 与树
            let leaves = self
                .notebook
                .read()
                .unwrap()
                .tabs
                .get(&tab)
                .map(|(_, c)| c.tree.leaves())
                .unwrap_or_default();
            self.tab_panes.write().unwrap().insert(tab, leaves.clone());
            *self.current_pane.lock().unwrap() = leaves.first().copied();
        }
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.refresh_input_visibility();
        self.focus_active_pane();
    }

    /// 所有 tab 都关了之后的行为（**不再**默认开新 shell）。
    fn on_all_tabs_closed(self: &Arc<Self>) {
        match last_tabs_closed_action(self.cfg.behavior.on_last_pane_exit) {
            LastTabsClosedAction::CloseWindow => {
                tracing::info!(target = "muxterm::window", "无剩余 tab，关闭窗口");
                self.window.close();
            }
            LastTabsClosedAction::KeepEmpty => {
                tracing::info!(target = "muxterm::window", "无剩余 tab，保留空窗口");
                self.show_status("所有 pane 已关闭");
                *self.current_tab.lock().unwrap() = None;
                *self.current_pane.lock().unwrap() = None;
                self.refresh_window_title();
            }
            LastTabsClosedAction::NewShell => {
                // 废弃旧逻辑：仍兼容配置，但打 warn
                tracing::warn!(
                    target = "muxterm::window",
                    "on_last_pane_exit=new_shell 已废弃，将开新 tab（请改用 close_window）"
                );
                self.new_local_tab();
            }
        }
    }

    fn show_status(self: &Arc<Self>, msg: &str) {
        self.status_label.remove_css_class("hidden");
        self.status_label.set_visible(true);
        self.status_label.set_label(msg);
    }

    /// Alt+D / Alt+Shift+D：在当前激活 pane 内嵌套分割（不新建 tab）。
    /// - 本地：spawn 新 shell，GtkPaned 嵌套
    /// - tmux：发 `split-window -h/-v`（layout-change 后按树重建 Paned）
    fn new_pane_action(self: &Arc<Self>, vertical: bool) {
        let pane = *self.current_pane.lock().unwrap();
        match pane {
            Some(PaneKey::Tmux(p)) => {
                let flag = if vertical { "-v" } else { "-h" };
                let cmd = format!("split-window -t {} {}\n", p.as_str(), flag);
                if let Ok(g) = self.cmd_sender.lock() {
                    if let Some(bridge) = g.as_ref() {
                        bridge.sender().send(&cmd);
                        self.show_status(&format!(
                            "tmux split-window {}",
                            if vertical { "vertical" } else { "horizontal" }
                        ));
                    } else {
                        self.show_status("未连接 tmux，无法分割");
                    }
                }
            }
            Some(PaneKey::Local(_)) => self.split_local_pane(vertical),
            None => {
                if matches!(*self.current_tab.lock().unwrap(), Some(TabKey::Local(_))) {
                    self.split_local_pane(vertical);
                } else {
                    self.new_local_tab();
                }
            }
        }
    }

    /// 本地：在当前激活 pane 位置嵌套 GtkPaned（原 pane + 新 shell）。
    fn split_local_pane(self: &Arc<Self>, vertical: bool) {
        let tab = match *self.current_tab.lock().unwrap() {
            Some(t @ TabKey::Local(_)) => t,
            _ => {
                tracing::info!(target: "muxterm::window", "split 仅对本地 tab 有效");
                return;
            }
        };
        let target = match *self.current_pane.lock().unwrap() {
            Some(p @ PaneKey::Local(_)) => p,
            _ => {
                tracing::info!(target: "muxterm::window", "无激活本地 pane，无法分割");
                return;
            }
        };

        let workdir = self.inherit_workdir();
        let opts = self.local_spawn_opts(workdir);
        let view = PaneView::new_local(
            &self.theme,
            &self.cfg.font.family,
            self.cfg.font.size,
            self.cfg.scrollback.lines,
            &opts,
        );
        let term = view.terminal.clone();

        let new_pane = {
            let panes = self.tab_panes.read().unwrap();
            let max = panes
                .get(&tab)
                .map(|v| {
                    v.iter()
                        .filter_map(|p| match p {
                            PaneKey::Local(LocalPaneId(n)) => Some(*n),
                            _ => None,
                        })
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
                + 1;
            PaneKey::Local(LocalPaneId(max))
        };

        let shared = self.clone();
        let tab_cb = tab;
        let pane_cb = new_pane;
        term.connect_child_exited(move |_t, status| {
            shared.on_local_pane_exited(tab_cb, pane_cb, status);
        });

        self.pane_views.write().unwrap().insert(new_pane, view);

        let terminals: HashMap<PaneKey, vte4::Terminal> = {
            let views = self.pane_views.read().unwrap();
            views
                .iter()
                .map(|(k, v)| (*k, v.terminal.clone()))
                .collect()
        };
        let title = self.tab_display_name(tab);
        let orient = if vertical {
            SplitOrient::Vertical
        } else {
            SplitOrient::Horizontal
        };

        let ok = {
            let mut nb = self.notebook.write().unwrap();
            nb.split_and_relayout(tab, target, new_pane, orient, &terminals, &title)
        };
        if !ok {
            self.pane_views.write().unwrap().remove(&new_pane);
            self.show_status("分割失败：找不到激活 pane");
            return;
        }

        let leaves = self
            .notebook
            .read()
            .unwrap()
            .tabs
            .get(&tab)
            .map(|(_, c)| c.tree.leaves())
            .unwrap_or_default();
        self.tab_panes.write().unwrap().insert(tab, leaves.clone());
        // 焦点立刻落到新 pane（ARCHITECTURE §2.3）
        *self.current_pane.lock().unwrap() = Some(new_pane);
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.refresh_input_visibility();
        self.focus_active_pane();
        self.show_status(&format!(
            "已嵌套分割（{}，{} panes）",
            if vertical { "竖直" } else { "水平" },
            leaves.len()
        ));
    }

    fn switch_tab_n(self: &Arc<Self>, n: u32) {
        // 先算索引并释放 notebook 读锁，再 select——避免持锁触发 switch-page
        let (nbook, idx) = {
            let nb = self.notebook.read().unwrap();
            let total = nb.n_tabs() as usize;
            let Some(idx) = tab_index_for_shortcut(n, total) else {
                return;
            };
            (nb.notebook.clone(), idx as u32)
        };
        nbook.set_current_page(Some(idx));
    }

    /// 切换当前 tab 内的 pane。所有锁短持、先拷贝再改，避免嵌套死锁。
    fn switch_pane(self: &Arc<Self>, next: bool) {
        let tab = match *self.current_tab.lock().unwrap() {
            Some(t) => t,
            None => return,
        };
        let panes = self
            .tab_panes
            .read()
            .unwrap()
            .get(&tab)
            .cloned()
            .unwrap_or_default();
        let cur = *self.current_pane.lock().unwrap();
        let idx = panes.iter().position(|p| Some(*p) == cur).unwrap_or(0);
        let Some(new_idx) = next_pane_index(panes.len(), idx, next) else {
            return;
        };
        let new_pane = panes[new_idx];
        *self.current_pane.lock().unwrap() = Some(new_pane);
        if let Ok(mut nb) = self.notebook.try_write() {
            if let Some((_, c)) = nb.tabs.get_mut(&tab) {
                c.active = Some(new_pane);
            }
        }
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.focus_active_pane();
    }

    /// 把键盘焦点落到当前激活 pane 的 terminal（不持锁调用 grab_focus）。
    fn focus_active_pane(self: &Arc<Self>) {
        let pane = *self.current_pane.lock().unwrap();
        let Some(pane) = pane else {
            return;
        };
        let term = self
            .pane_views
            .read()
            .unwrap()
            .get(&pane)
            .map(|v| v.terminal.clone());
        if let Some(term) = term {
            term.grab_focus();
        }
    }

    fn dispatch_action(self: &Arc<Self>, action: Action) {
        match action {
            Action::NewWindow | Action::NewTab => {
                self.new_local_tab();
            }
            Action::NewPane => self.new_pane_action(false),
            Action::NewPaneVertical => self.new_pane_action(true),
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
            Action::Search => self.show_pane_switcher(),
            Action::CommandPalette => {
                let shared = self.clone();
                command_palette::show(&self.window, move |cmd| {
                    SharedState::run_palette_command(&shared, cmd);
                    // 打开二级对话框的命令自行抢焦点；其余回到 terminal
                    if palette_should_refocus_terminal(cmd) {
                        shared.refresh_input_visibility();
                        shared.focus_active_pane();
                    }
                });
            }
            Action::Unknown => {}
        }
    }

    fn run_palette_command(self: &Arc<Self>, cmd: &str) {
        match cmd {
            "new_window" | "new_tab" => {
                self.new_local_tab();
            }
            "new_pane" => self.new_pane_action(false),
            "new_pane_vertical" => self.new_pane_action(true),
            "close_pane" => self.close_current_pane(),
            "close_tab" => self.close_current_tab(),
            "close_window" | "quit" => self.window.close(),
            "switch_tab_1" => self.switch_tab_n(1),
            "switch_tab_2" => self.switch_tab_n(2),
            "switch_tab_3" => self.switch_tab_n(3),
            "switch_tab_4" => self.switch_tab_n(4),
            "switch_tab_5" => self.switch_tab_n(5),
            "switch_tab_6" => self.switch_tab_n(6),
            "switch_tab_7" => self.switch_tab_n(7),
            "switch_tab_8" => self.switch_tab_n(8),
            "switch_tab_9" => self.switch_tab_n(9),
            "switch_pane_prev" => self.switch_pane(false),
            "switch_pane_next" => self.switch_pane(true),
            "search_panes" => self.show_pane_switcher(),
            "rename_pane" => self.rename_current_pane(),
            "tmux_attach" => {
                let shared = self.clone();
                let win = self.window.clone();
                tmux_dialog::show(&win, move |action| {
                    SharedState::do_tmux_action(&shared, action);
                });
            }
            "tmux_new" => {
                let shared = self.clone();
                let default_name = format!(
                    "muxterm-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                pane_switcher::show_rename(&self.window, &default_name, move |name| {
                    SharedState::do_tmux_action(
                        &shared,
                        TmuxAction::NewSession { name: Some(name) },
                    );
                });
            }
            "tmux_detach" => self.tmux_detach(),
            "ssh_connect" => self.show_ssh_connect(),
            "ssh_disconnect" => self.ssh_disconnect(),
            "reload_config" => self.reload_config(),
            "open_config" | "preferences" => self.open_config_file(),
            other => tracing::info!(target = "muxterm::window", cmd = %other, "未知命令"),
        }
    }

    /// 命令面板「ssh: connect」：QuickPick 选配置预设或输入 `user@host[:port][/session]`。
    fn show_ssh_connect(self: &Arc<Self>) {
        let mut presets = Vec::new();
        if self.cfg.ssh.is_configured() {
            let user = if self.cfg.ssh.user.trim().is_empty() {
                std::env::var("USER").unwrap_or_else(|_| "root".into())
            } else {
                self.cfg.ssh.user.clone()
            };
            let label = format!("{}@{}:{}", user, self.cfg.ssh.host, self.cfg.ssh.port);
            presets.push(QuickPickItem {
                id: "config".into(),
                label,
                detail: Some("from config [ssh]".into()),
            });
        }
        let shared = self.clone();
        let key_path = self.cfg.ssh.key_path.clone();
        quick_pick::show_freeform(
            &self.window,
            "user@host[:port][/session]…",
            presets,
            move |picked| {
                let Some(item) = picked else {
                    shared.refresh_input_visibility();
                    shared.focus_active_pane();
                    return;
                };
                let line = item.label;
                let Some((user, host, port, session)) = parse_ssh_connect_line(&line) else {
                    shared.show_status("无法解析 SSH 目标（格式: user@host[:port][/session]）");
                    return;
                };
                let auth = if key_path.trim().is_empty() {
                    SshAuth::Agent
                } else {
                    SshAuth::Key {
                        path: key_path.clone(),
                        passphrase: None,
                    }
                };
                let ssh = SshConfig {
                    host,
                    port,
                    user,
                    auth,
                };
                SharedState::do_ssh_connect(&shared, ssh, session);
            },
        );
    }

    /// 断开 SSH/tmux：释放桥接并关闭相关 tmux tab。
    fn ssh_disconnect(self: &Arc<Self>) {
        if let Ok(mut g) = self.cmd_sender.lock() {
            if g.is_none() {
                self.show_status("未连接远程/tmux");
                return;
            }
            *g = None; // drop Runtime → 断开
        }
        self.close_all_tmux_tabs();
        self.show_status("已断开 SSH/tmux");
        self.focus_active_pane();
    }

    fn do_ssh_connect(self: &Arc<Self>, ssh: SshConfig, session_name: String) {
        let Some(app) = self.window.application() else {
            SharedState::connect_ssh(self, ssh, session_name);
            return;
        };
        let win = AppWindow::new_ssh_session(
            (*self.cfg).clone(),
            (*self.theme).clone(),
            ssh,
            session_name,
        );
        app.add_window(&win.window);
        win.window.present();
    }

    fn connect_ssh(self: &Arc<Self>, ssh: SshConfig, session_name: String) {
        let shared = self.clone();
        let auto_mouse = self.cfg.tmux.auto_mouse;
        let on_event = move |ev: &UiEvent| shared.handle_ui_event(ev);
        match spawn_ssh_bridge(ssh, session_name, auto_mouse, on_event) {
            Some(bridge) => {
                self.show_status("正在连接 SSH tmux…");
                if let Ok(mut g) = self.cmd_sender.lock() {
                    *g = Some(bridge);
                }
                self.refresh_input_visibility();
            }
            None => self.show_status("启动 SSH 桥接失败"),
        }
    }

    /// 关闭窗口内所有 tmux tab（断开连接时调用）。
    fn close_all_tmux_tabs(self: &Arc<Self>) {
        let keys: Vec<TabKey> = self
            .notebook
            .read()
            .unwrap()
            .keys_in_order()
            .into_iter()
            .filter(|k| matches!(k, TabKey::TmuxWindow(_)))
            .collect();
        for tab in keys {
            if let TabKey::TmuxWindow(window) = tab {
                let panes = self
                    .tab_panes
                    .read()
                    .unwrap()
                    .get(&tab)
                    .cloned()
                    .unwrap_or_default();
                for p in &panes {
                    if let PaneKey::Tmux(pid) = p {
                        self.pane_window.write().unwrap().remove(&pid.0);
                        self.pane_tab.write().unwrap().remove(&pid.0);
                        self.pane_views.write().unwrap().remove(p);
                    }
                }
                self.tab_panes.write().unwrap().remove(&tab);
                self.window_names.write().unwrap().remove(&window.0);
                self.notebook.write().unwrap().remove(tab);
            }
        }
        if self.notebook.read().unwrap().n_tabs() == 0 {
            self.on_all_tabs_closed();
        } else {
            self.notebook.read().unwrap().select_by_index(0);
            self.refresh_tab_bar();
            self.refresh_window_title();
            self.refresh_input_visibility();
            self.focus_active_pane();
        }
    }

    /// 关闭当前 pane（本地 kill 子进程；tmux 发 kill-pane）。
    fn close_current_pane(self: &Arc<Self>) {
        let pane = match *self.current_pane.lock().unwrap() {
            Some(p) => p,
            None => return,
        };
        let tab = *self.current_tab.lock().unwrap();
        match pane {
            PaneKey::Local(_) => {
                let views = self.pane_views.read().unwrap();
                if let Some(v) = views.get(&pane) {
                    if !v.kill_child() {
                        // 无 pid 时仍按退出路径移除
                        drop(views);
                        if let Some(t) = tab {
                            self.on_local_pane_exited(t, pane, 0);
                        }
                        return;
                    }
                }
                // 等 child-exited 回调走统一清理
            }
            PaneKey::Tmux(p) => {
                if let Ok(g) = self.cmd_sender.lock() {
                    if let Some(bridge) = g.as_ref() {
                        bridge.sender().send(&tmux_kill_pane_cmd(p).to_line());
                    }
                }
            }
        }
    }

    /// 关闭当前 tab：关掉其中所有 pane。
    fn close_current_tab(self: &Arc<Self>) {
        let tab = match *self.current_tab.lock().unwrap() {
            Some(t) => t,
            None => return,
        };
        match tab {
            TabKey::Local(_) => {
                let panes = self
                    .tab_panes
                    .read()
                    .unwrap()
                    .get(&tab)
                    .cloned()
                    .unwrap_or_default();
                for p in panes {
                    if let Some(v) = self.pane_views.read().unwrap().get(&p) {
                        let _ = v.kill_child();
                    }
                }
                // 子进程退出会逐个清理；若全部已无进程则直接拆 tab
                let still = self
                    .tab_panes
                    .read()
                    .unwrap()
                    .get(&tab)
                    .map(|v| v.len())
                    .unwrap_or(0);
                if still == 0 {
                    self.notebook.write().unwrap().remove(tab);
                    self.tab_panes.write().unwrap().remove(&tab);
                    if self.notebook.read().unwrap().n_tabs() == 0 {
                        self.on_all_tabs_closed();
                    } else {
                        self.notebook.read().unwrap().select_by_index(0);
                    }
                    self.refresh_tab_bar();
                    self.refresh_window_title();
                    self.refresh_input_visibility();
                    self.focus_active_pane();
                }
            }
            TabKey::TmuxWindow(w) => {
                if let Ok(g) = self.cmd_sender.lock() {
                    if let Some(bridge) = g.as_ref() {
                        bridge.sender().send(&tmux_kill_window_cmd(w).to_line());
                    }
                }
            }
        }
    }

    /// Alt+R / search panes：按名字切换 pane。
    fn show_pane_switcher(self: &Arc<Self>) {
        let entries = self.collect_pane_entries();
        if entries.is_empty() {
            self.show_status("没有可切换的 pane");
            return;
        }
        let shared = self.clone();
        pane_switcher::show(&self.window, entries, move |entry| {
            shared.jump_to_pane(entry.tab, entry.pane);
        });
    }

    fn collect_pane_entries(self: &Arc<Self>) -> Vec<PaneEntry> {
        let keys = self.notebook.read().unwrap().keys_in_order();
        let views = self.pane_views.read().unwrap();
        let tab_panes = self.tab_panes.read().unwrap();
        let mut out = Vec::new();
        for (ti, tab) in keys.iter().enumerate() {
            let tab_no = ti + 1;
            let panes = tab_panes.get(tab).cloned().unwrap_or_default();
            let detail = match tab {
                TabKey::TmuxWindow(_) => "tmux",
                TabKey::Local(_) => "local",
            };
            for (pi, pane) in panes.iter().enumerate() {
                let name = views
                    .get(pane)
                    .map(|v| v.display_name())
                    .unwrap_or_else(|| "pane".into());
                let label = if panes.len() > 1 {
                    format!("{tab_no}:{name} · pane{}", pi + 1)
                } else {
                    format!("{tab_no}:{name}")
                };
                out.push(PaneEntry {
                    tab: *tab,
                    pane: *pane,
                    name,
                    label,
                    detail: Some(detail.into()),
                });
            }
        }
        out
    }

    fn jump_to_pane(self: &Arc<Self>, tab: TabKey, pane: PaneKey) {
        let (nbook, idx) = {
            let nb = self.notebook.read().unwrap();
            (nb.notebook.clone(), nb.tabs.get(&tab).map(|(i, _)| *i))
        };
        if let Some(idx) = idx {
            nbook.set_current_page(Some(idx));
        }
        *self.current_tab.lock().unwrap() = Some(tab);
        *self.current_pane.lock().unwrap() = Some(pane);
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.refresh_input_visibility();
        self.focus_active_pane();
    }

    fn rename_current_pane(self: &Arc<Self>) {
        let pane = match *self.current_pane.lock().unwrap() {
            Some(p) => p,
            None => {
                self.show_status("没有激活的 pane");
                return;
            }
        };
        let current = self
            .pane_views
            .read()
            .unwrap()
            .get(&pane)
            .map(|v| v.display_name())
            .unwrap_or_else(|| "pane".into());
        let shared = self.clone();
        pane_switcher::show_rename(&self.window, &current, move |name| {
            if let Some(v) = shared.pane_views.write().unwrap().get_mut(&pane) {
                v.custom_name = Some(name);
            }
            shared.refresh_tab_bar();
            shared.refresh_window_title();
            // 同步 notebook 页标题（虽已隐藏，保持一致）
            if let Some(tab) = *shared.current_tab.lock().unwrap() {
                let title = shared.tab_display_name(tab);
                shared.notebook.read().unwrap().set_title(tab, &title);
            }
            shared.refresh_input_visibility();
            shared.focus_active_pane();
        });
    }

    fn tmux_detach(self: &Arc<Self>) {
        if let Ok(g) = self.cmd_sender.lock() {
            if let Some(bridge) = g.as_ref() {
                bridge.sender().send("detach-client\n");
            } else {
                self.show_status("未连接 tmux");
            }
        }
    }

    fn reload_config(self: &Arc<Self>) {
        match Config::load() {
            Ok(cfg) => {
                // 热更新：字体/滚动等已建 terminal 不重建，仅刷新配置引用与状态提示
                // 完整热替换后续 phase；这里替换 Arc 内无法原地写，提示用户重启生效的字段
                let _ = cfg;
                self.show_status("配置已重新加载（部分项需新 pane 生效）");
            }
            Err(e) => self.show_status(&format!("重新加载配置失败: {e}")),
        }
    }

    fn open_config_file(self: &Arc<Self>) {
        let path = match Config::user_config_path() {
            Some(p) => p,
            None => {
                self.show_status("无法定位配置目录");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !path.exists() {
            let _ = std::fs::write(&path, include_str!("../../../configs/config.example.toml"));
        }
        let uri = format!("file://{}", path.display());
        if let Err(e) =
            gtk4::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk4::gio::AppLaunchContext>)
        {
            // 回退 xdg-open
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
            tracing::warn!(
                target = "muxterm::window",
                "launch_default_for_uri 失败: {e}"
            );
        }
        self.show_status(&format!("已打开 {}", path.display()));
    }

    /// attach / new-session：新建独立 GTK 窗口承载该 session（不污染当前本地窗口）。
    fn do_tmux_action(self: &Arc<Self>, action: TmuxAction) {
        let Some(app) = self.window.application() else {
            tracing::warn!(
                target = "muxterm::window",
                "无 Application，回退为当前窗口内 attach"
            );
            self.connect_tmux_action(action);
            return;
        };
        let win = AppWindow::new_tmux_session((*self.cfg).clone(), (*self.theme).clone(), action);
        app.add_window(&win.window);
        win.window.present();
        // AppWindow 可 drop：SharedState 由信号闭包持有 Arc
    }

    fn connect_tmux_action(self: &Arc<Self>, action: TmuxAction) {
        let (config, auto_mouse) = match action {
            TmuxAction::Attach { session } => {
                let cfg = crate::platform::linux::wiring::attach_config(&session);
                (cfg, self.cfg.tmux.auto_mouse)
            }
            TmuxAction::NewSession { name } => {
                let cfg = crate::platform::linux::wiring::new_session_config(name);
                (cfg, self.cfg.tmux.auto_mouse)
            }
        };
        self.connect_tmux(config, auto_mouse);
    }

    fn connect_tmux(self: &Arc<Self>, config: TmuxClientConfig, auto_mouse: bool) {
        let shared = self.clone();
        let on_event = move |ev: &UiEvent| shared.handle_ui_event(ev);
        match spawn_bridge(config, auto_mouse, on_event) {
            Some(bridge) => {
                self.show_status("正在连接 tmux…");
                if let Ok(mut g) = self.cmd_sender.lock() {
                    *g = Some(bridge); // 持有 Runtime，防止 task 被 cancel
                }
                // tmux attach：无底部输入框，焦点进 terminal
                self.refresh_input_visibility();
                self.focus_active_pane();
            }
            None => {
                self.show_status("启动 tmux 桥接失败");
            }
        }
    }

    fn handle_ui_event(self: &Arc<Self>, ev: &UiEvent) {
        match ev {
            UiEvent::Connected => {
                self.show_status("已连接 tmux");
            }
            UiEvent::Error { msg } => {
                self.show_status("tmux 连接失败");
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
                self.ensure_tmux_pane_view(*pane);
                // 若尚无 layout 归属，用 pane id 占位成一个 window tab，避免 attach 后空白
                if !self.pane_tab.read().unwrap().contains_key(&pane.0) {
                    let fake_layout = format!("80x24,0,0,{}", pane.0);
                    self.apply_tmux_layout(WindowId(pane.0), &fake_layout);
                }
                if let Some(view) = self.pane_views.read().unwrap().get(&PaneKey::Tmux(*pane)) {
                    view.feed_output(data);
                }
            }
            UiEvent::WindowAdd { window } => {
                tracing::debug!(target = "muxterm::window", ?window, "tmux window-add");
            }
            UiEvent::LayoutChange { window, layout, .. } => {
                self.apply_tmux_layout(*window, layout);
            }
            UiEvent::WindowClose { window } => {
                let tab = TabKey::TmuxWindow(*window);
                let panes = self
                    .tab_panes
                    .read()
                    .unwrap()
                    .get(&tab)
                    .cloned()
                    .unwrap_or_default();
                for p in &panes {
                    if let PaneKey::Tmux(pid) = p {
                        self.pane_window.write().unwrap().remove(&pid.0);
                        self.pane_tab.write().unwrap().remove(&pid.0);
                        self.pane_views.write().unwrap().remove(p);
                    }
                }
                self.tab_panes.write().unwrap().remove(&tab);
                self.window_names.write().unwrap().remove(&window.0);
                self.notebook.write().unwrap().remove(tab);
                if self.notebook.read().unwrap().n_tabs() == 0 {
                    // 仍可能有本地 tab；只在全空时关窗
                    self.on_all_tabs_closed();
                } else {
                    self.notebook.read().unwrap().select_by_index(0);
                }
                self.refresh_tab_bar();
                self.refresh_window_title();
                self.refresh_input_visibility();
            }
            UiEvent::WindowRenamed { window, name } => {
                self.window_names
                    .write()
                    .unwrap()
                    .insert(window.0, name.clone());
                let tab = TabKey::TmuxWindow(*window);
                if self.notebook.read().unwrap().contains(tab) {
                    self.notebook.read().unwrap().set_title(tab, name);
                }
                self.refresh_tab_bar();
            }
            UiEvent::SessionChanged { sid, name } => {
                *self.session_name.write().unwrap() = name.clone();
                let n = self.notebook.read().unwrap().tab_count();
                let nm = name.clone().unwrap_or_else(|| format!("${sid}"));
                self.show_status(&format!("session: {nm} | tabs: {n}"));
                self.refresh_window_title();
            }
            UiEvent::Exit { reason } => {
                let msg = match reason {
                    Some(r) => format!("tmux 已断开（{r}）"),
                    None => "tmux 已断开".to_string(),
                };
                self.show_status(&msg);
                if let Ok(mut g) = self.cmd_sender.lock() {
                    *g = None;
                }
                self.close_all_tmux_tabs();
                self.refresh_input_visibility();
                self.focus_active_pane();
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
        let pane = match key {
            Some(t) => self
                .tab_panes
                .read()
                .unwrap()
                .get(&t)
                .and_then(|ps| ps.first().copied()),
            None => None,
        };
        *self.current_pane.lock().unwrap() = pane;
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.refresh_input_visibility();
        self.focus_active_pane();
    }

    /// 底部输入栏已废弃：tmux/本地都直接在 terminal 打字。
    /// 始终 `set_visible(false)`（不 remove / 不销毁控件）。
    fn refresh_input_visibility(self: &Arc<Self>) {
        debug_assert!(!crate::platform::linux::lifecycle::input_bar_should_be_visible());
        self.input_bar_container.set_visible(false);
    }

    /// 刷新极简 TabBar。
    fn refresh_tab_bar(self: &Arc<Self>) {
        let keys = self.notebook.read().unwrap().keys_in_order();
        let current = *self.current_tab.lock().unwrap();
        let mut items = Vec::with_capacity(keys.len());
        for (i, key) in keys.iter().enumerate() {
            let name = self.tab_display_name(*key);
            items.push(TabBarItem {
                key: *key,
                title: format_tab_bar_title(i + 1, &name),
                active: current == Some(*key),
            });
        }
        self.tab_bar.rebuild(&items);
    }

    /// tab 显示名：优先 window 名 / 激活 pane 程序名；多 pane 时加 ` · Npanes`。
    fn tab_display_name(self: &Arc<Self>, tab: TabKey) -> String {
        if let TabKey::TmuxWindow(w) = tab {
            if let Some(n) = self.window_names.read().unwrap().get(&w.0) {
                if !n.is_empty() {
                    let n_panes = self
                        .tab_panes
                        .read()
                        .unwrap()
                        .get(&tab)
                        .map(|p| p.len())
                        .unwrap_or(1);
                    return format_tab_display_name(n, n_panes);
                }
            }
        }
        let panes = self
            .tab_panes
            .read()
            .unwrap()
            .get(&tab)
            .cloned()
            .unwrap_or_default();
        let active = self.current_pane.lock().unwrap().clone();
        let views = self.pane_views.read().unwrap();
        let primary = active
            .filter(|p| panes.contains(p))
            .or_else(|| panes.first().copied());
        let fallback = match tab {
            TabKey::Local(_) => "shell",
            TabKey::TmuxWindow(_) => "tmux",
        };
        let name = primary
            .and_then(|p| views.get(&p).map(|v| v.display_name()))
            .unwrap_or_else(|| fallback.into());
        format_tab_display_name(&name, panes.len())
    }

    /// 轮询更新各 pane 的 program_name（用户未自定义时）。
    fn refresh_pane_titles(self: &Arc<Self>) {
        let mut changed = false;
        {
            let mut views = self.pane_views.write().unwrap();
            for (key, view) in views.iter_mut() {
                if view.custom_name.is_some() {
                    continue;
                }
                let new_name = match key {
                    PaneKey::Local(_) => view
                        .child_pid
                        .get()
                        .and_then(crate::platform::linux::title_watch::local_foreground_name),
                    PaneKey::Tmux(p) => crate::platform::linux::title_watch::tmux_pane_command(*p),
                };
                if let Some(n) = new_name {
                    if n != view.program_name {
                        view.program_name = n;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.refresh_tab_bar();
            self.refresh_window_title();
        }
    }

    /// 确保 tmux pane 有对应 PaneView（尚未关联 window/tab 也可以先建）。
    fn ensure_tmux_pane_view(self: &Arc<Self>, pane: PaneId) {
        let pane_key = PaneKey::Tmux(pane);
        if self.pane_views.read().unwrap().contains_key(&pane_key) {
            return;
        }
        let view = PaneView::new_tmux(
            pane,
            &self.theme,
            &self.cfg.font.family,
            self.cfg.font.size,
            self.cfg.scrollback.lines,
        );
        self.wire_tmux_commit(&view, pane);
        self.pane_views.write().unwrap().insert(pane_key, view);
    }

    /// tmux terminal 键盘 → send-keys（Capture，不走 VTE 本地回显）。
    fn wire_tmux_commit(self: &Arc<Self>, view: &PaneView, pane: PaneId) {
        let sender = self.cmd_sender.clone();
        let controller = EventControllerKey::new();
        // Capture：在 VTE 之前截获；全局 Alt 快捷键在 window Capture 里已处理
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_c, keyval, _keycode, mods| {
            let Some(keys) = gdk_key_to_tmux(keyval, mods) else {
                return glib::Propagation::Proceed;
            };
            if let Ok(g) = sender.lock() {
                if let Some(bridge) = g.as_ref() {
                    let cmd = send_keys(pane, &keys);
                    bridge.sender().send(&cmd.to_line());
                }
            }
            glib::Propagation::Stop
        });
        view.terminal.add_controller(controller);
    }

    /// 按 layout-change 把 tmux window 映射为一个 Tab，内部按嵌套树分割。
    fn apply_tmux_layout(self: &Arc<Self>, window: WindowId, layout: &str) {
        let layout_tree = match parse_layout_tree(layout) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    target = "muxterm::window",
                    %layout,
                    "layout 未解析出树"
                );
                return;
            }
        };
        let ids = layout_tree.leaves();
        if ids.is_empty() {
            return;
        }
        let pane_tree = layout_tree.to_pane_node();
        let tab = TabKey::TmuxWindow(window);

        for id in &ids {
            self.pane_window.write().unwrap().insert(*id, window.0);
            self.ensure_tmux_pane_view(PaneId(*id));
            self.pane_tab.write().unwrap().insert(*id, tab);
        }

        let pane_keys: Vec<PaneKey> = pane_tree.leaves();
        self.tab_panes
            .write()
            .unwrap()
            .insert(tab, pane_keys.clone());

        let title = self
            .window_names
            .read()
            .unwrap()
            .get(&window.0)
            .cloned()
            .unwrap_or_else(|| self.tab_display_name(tab));

        let need_create = !self.notebook.read().unwrap().contains(tab);
        if need_create {
            let views = self.pane_views.read().unwrap();
            let first = views
                .get(&pane_keys[0])
                .expect("apply_tmux_layout: 缺 first pane view");
            let mut nb = self.notebook.write().unwrap();
            nb.ensure_tmux_window_tab(window, first, &title);
        }

        let terminals: HashMap<PaneKey, vte4::Terminal> = {
            let views = self.pane_views.read().unwrap();
            pane_keys
                .iter()
                .filter_map(|p| views.get(p).map(|v| (*p, v.terminal.clone())))
                .collect()
        };
        let active = *self.current_pane.lock().unwrap();
        {
            let mut nb = self.notebook.write().unwrap();
            nb.set_tree_and_relayout(tab, pane_tree, active, &terminals, &title);
        }

        *self.current_tab.lock().unwrap() = Some(tab);
        if active.map(|a| !pane_keys.contains(&a)).unwrap_or(true) {
            *self.current_pane.lock().unwrap() = pane_keys.first().copied();
        }
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.refresh_input_visibility();
        self.focus_active_pane();
        self.show_status(&format!(
            "tmux window @{} → tab（{} panes）",
            window.0,
            pane_keys.len()
        ));
    }

    fn refresh_window_title(self: &Arc<Self>) {
        if !self.cfg.ui.show_title_bar {
            self.window.set_title(Some("muxterm"));
            return;
        }
        let pane = *self.current_pane.lock().unwrap();
        let title = match pane {
            Some(p) => self
                .pane_views
                .read()
                .unwrap()
                .get(&p)
                .map(|v| v.display_name())
                .unwrap_or_else(|| "muxterm".into()),
            None => "muxterm".into(),
        };
        self.window.set_title(Some(&title));
    }
}

/// GDK 按键 → tmux send-keys。Alt 组合留给全局快捷键，此处不处理。
fn gdk_key_to_tmux(keyval: gdk::Key, mods: gdk::ModifierType) -> Option<Vec<Key>> {
    if mods.contains(gdk::ModifierType::ALT_MASK) {
        return None;
    }
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);

    if ctrl {
        // Ctrl+Shift+C/V 等留给 VTE/系统，不转发
        if shift {
            return None;
        }
        if let Some(c) = keyval.to_unicode() {
            if c.is_ascii_alphabetic() || c.is_ascii_digit() {
                return Some(vec![Key::ctrl(c)]);
            }
        }
        // 常见控制键
        match keyval {
            gdk::Key::c | gdk::Key::C => return Some(vec![Key::ctrl('c')]),
            gdk::Key::d | gdk::Key::D => return Some(vec![Key::ctrl('d')]),
            gdk::Key::z | gdk::Key::Z => return Some(vec![Key::ctrl('z')]),
            gdk::Key::l | gdk::Key::L => return Some(vec![Key::ctrl('l')]),
            gdk::Key::u | gdk::Key::U => return Some(vec![Key::ctrl('u')]),
            gdk::Key::w | gdk::Key::W => return Some(vec![Key::ctrl('w')]),
            _ => {}
        }
    }

    match keyval {
        gdk::Key::Return | gdk::Key::KP_Enter => return Some(vec![Key::enter()]),
        gdk::Key::BackSpace => return Some(vec![Key::bspace()]),
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => return Some(vec![Key::tab()]),
        gdk::Key::Escape => return Some(vec![Key::escape()]),
        gdk::Key::Up | gdk::Key::KP_Up => return Some(vec![Key::up()]),
        gdk::Key::Down | gdk::Key::KP_Down => return Some(vec![Key::down()]),
        gdk::Key::Left | gdk::Key::KP_Left => return Some(vec![Key::left()]),
        gdk::Key::Right | gdk::Key::KP_Right => return Some(vec![Key::right()]),
        _ => {}
    }

    if let Some(ch) = keyval.to_unicode() {
        if !ch.is_control() {
            return Some(vec![Key::literal(ch.to_string())]);
        }
    }
    None
}

/// 加载极简 CSS。
fn apply_css() {
    let provider = CssProvider::new();
    provider.load_from_data(include_str!("../../../assets/style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
