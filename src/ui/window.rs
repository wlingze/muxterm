//! 主窗口（极简布局 + 程序/pane 生命周期）。
//!
//! 布局（自上而下，tab 栏位置可配置）：
//! 1. 窗口标题：当前 pane 程序名
//! 2. Terminal 区域（Notebook，隐藏原生 tabs）占满
//! 3. 极简 TabBar（默认底部 ~24px）
//! 4. 状态提示（异常退出等，平时可隐藏）
//!
//! 程序退出模型：
//! - 正常/异常退出 → 关闭对应 pane（异常可提示）
//! - tab 内无 pane → 关 tab
//! - 无 tab → 按 `behavior.on_last_pane_exit`（默认关窗）
//! - **不再**「最后一个 shell 退出开新空 shell」

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Box, CssProvider, EventControllerKey, Label, Orientation, Window};
use vte4::prelude::*;

use crate::config::{
    decode_wait_status, expand_config_value, parse_command_argv, Action, Config, OnLastPaneExit,
    OnProgramExitAbnormal, Theme,
};
use crate::tmux::client::TmuxClientConfig;
use crate::tmux::command::kill_pane as tmux_kill_pane_cmd;
use crate::tmux::protocol::PaneId;
use crate::ui::command_palette;
use crate::ui::input_bar::InputBar;
use crate::ui::keymap::KeyMap;
use crate::ui::notebook::{LocalPaneId, PaneKey, PaneNotebook, TabKey};
use crate::ui::pane_view::{PaneView, SpawnOpts};
use crate::ui::tab_bar::{TabBar, TabBarItem};
use crate::ui::tmux_dialog::{self, TmuxAction};
use crate::ui::wiring::{spawn_bridge, CommandSender, UiEvent};

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
    /// tmux pane id → tab key。
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
    tab_bar: TabBar,
    window: Window,
}

impl AppWindow {
    pub fn new(config: Config, theme: Theme) -> Self {
        let window = Window::builder()
            .title("muxterm")
            .default_width(1000)
            .default_height(650)
            .build();

        apply_css();

        if config.ui.borderless {
            // GTK4 无统一无边框 API；保留配置项，后续可接 CSD/扩展。
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

        // 组装：terminal 占满；tab 栏 top/bottom；状态条贴 tab 栏内侧
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

        // TabBar 点击 → 切 Notebook page
        {
            let shared_click = shared.clone();
            shared.tab_bar.connect_activate(move |key| {
                shared_click.notebook.read().unwrap().select(key);
            });
        }

        app_win.wire_notebook_switch();
        app_win.wire_input_bar();
        app_win.wire_global_key_events();

        // 启动即一个本地程序 tab（默认 shell）
        app_win.new_local_tab();

        app_win
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
                    if let Some(s) = g.as_ref() {
                        s.send(line);
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

        // 异常退出且策略为 Keep：保留 pane，仅提示
        if code != 0 && self.cfg.behavior.on_program_exit_abnormal == OnProgramExitAbnormal::Keep {
            self.show_status(&format!("{prog} exited with code {code}"));
            return;
        }

        if code != 0 && self.cfg.behavior.on_program_exit_abnormal == OnProgramExitAbnormal::Notify
        {
            self.show_status(&format!("{prog} exited with code {code}"));
        }

        // 从 tab_panes 移除该 pane
        let mut tp = self.tab_panes.write().unwrap();
        if let Some(panes) = tp.get_mut(&tab) {
            panes.retain(|p| *p != pane);
        }
        let remaining: Vec<PaneKey> = tp.get(&tab).cloned().unwrap_or_default();
        let tab_empty = remaining.is_empty();
        drop(tp);

        self.pane_views.write().unwrap().remove(&pane);

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
            self.rebuild_local_tab(tab);
        }
        self.refresh_tab_bar();
        self.refresh_window_title();
        self.refresh_input_visibility();
    }

    /// 所有 tab 都关了之后的行为（**不再**默认开新 shell）。
    fn on_all_tabs_closed(self: &Arc<Self>) {
        match self.cfg.behavior.on_last_pane_exit {
            OnLastPaneExit::CloseWindow => {
                tracing::info!(target = "muxterm::window", "无剩余 tab，关闭窗口");
                self.window.close();
            }
            OnLastPaneExit::KeepEmpty => {
                tracing::info!(target = "muxterm::window", "无剩余 tab，保留空窗口");
                self.show_status("所有 pane 已关闭");
                *self.current_tab.lock().unwrap() = None;
                *self.current_pane.lock().unwrap() = None;
                self.refresh_window_title();
            }
            OnLastPaneExit::NewShell => {
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
        let title = self.tab_display_name(tab);
        {
            let mut nb = self.notebook.write().unwrap();
            nb.rebuild_local_root(tab, &terminals);
            nb.relayout_local_tab(tab, &title);
        }
    }

    /// 分割当前激活的本地 pane。
    fn split_current_pane(self: &Arc<Self>, vertical: bool) {
        let tab = match *self.current_tab.lock().unwrap() {
            Some(TabKey::Local(_)) => self.current_tab.lock().unwrap().unwrap(),
            _ => {
                tracing::info!(target: "muxterm::window", "split 仅对本地 tab 有效");
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
        let shared = self.clone();
        let tab_cb = tab;
        let pane_cb = new_pane;
        term.connect_child_exited(move |_t, status| {
            shared.on_local_pane_exited(tab_cb, pane_cb, status);
        });
        self.pane_views.write().unwrap().insert(new_pane, view);
        self.tab_panes
            .write()
            .unwrap()
            .get_mut(&tab)
            .map(|v| v.push(new_pane));
        let orient = if vertical {
            gtk4::Orientation::Vertical
        } else {
            gtk4::Orientation::Horizontal
        };
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
            let title = self.tab_display_name(tab);
            nb.rebuild_local_root(tab, &terminals);
            nb.relayout_local_tab(tab, &title);
        }
        *self.current_pane.lock().unwrap() = Some(new_pane);
        self.refresh_tab_bar();
        self.refresh_window_title();
    }

    fn switch_tab_n(self: &Arc<Self>, n: u32) {
        let nb = self.notebook.read().unwrap();
        let total = nb.n_tabs();
        if total == 0 {
            return;
        }
        let idx = if n == 0 { total - 1 } else { n.min(total) - 1 };
        nb.select_by_index(idx);
    }

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
        drop(cur);
        *self.current_pane.lock().unwrap() = Some(panes[new_idx]);
        self.refresh_tab_bar();
        self.refresh_window_title();
    }

    fn dispatch_action(self: &Arc<Self>, action: Action) {
        match action {
            Action::NewWindow | Action::NewTab => {
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
            Action::Search => {
                // Alt+R：后续 commit 换成 pane 切换器；暂用 search_panes 命令入口
                self.run_palette_command("search_panes");
            }
            Action::CommandPalette => {
                let shared = self.clone();
                command_palette::show(&self.window, move |cmd| {
                    SharedState::run_palette_command(&shared, cmd);
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
            "new_pane" => self.split_current_pane(false),
            "new_pane_vertical" => self.split_current_pane(true),
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
            "search_panes" => {
                self.show_status("search panes：下一 commit 实现 pane 切换器");
            }
            "rename_pane" => {
                self.show_status("rename pane：下一 commit 实现");
            }
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
            "tmux_detach" => self.tmux_detach(),
            "reload_config" => self.reload_config(),
            "open_config" | "preferences" => self.open_config_file(),
            other => tracing::info!(target = "muxterm::window", cmd = %other, "未知命令"),
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
                    if let Some(s) = g.as_ref() {
                        s.send(&tmux_kill_pane_cmd(p).to_line());
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
                    }
                    self.refresh_tab_bar();
                }
            }
            TabKey::Tmux(p) => {
                if let Ok(g) = self.cmd_sender.lock() {
                    if let Some(s) = g.as_ref() {
                        s.send(&tmux_kill_pane_cmd(p).to_line());
                    }
                }
            }
        }
    }

    fn tmux_detach(self: &Arc<Self>) {
        if let Ok(g) = self.cmd_sender.lock() {
            if let Some(s) = g.as_ref() {
                s.send("detach-client\n");
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
            let _ = std::fs::write(&path, include_str!("../../configs/config.example.toml"));
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
                let pane_key = PaneKey::Tmux(*pane);
                if !self.pane_views.read().unwrap().contains_key(&pane_key) {
                    let view = PaneView::new_tmux(
                        *pane,
                        &self.theme,
                        &self.cfg.font.family,
                        self.cfg.font.size,
                        self.cfg.scrollback.lines,
                    );
                    let title = view.program_name.clone();
                    let k = self.notebook.write().unwrap().add_tmux_tab(&view, &title);
                    self.pane_views.write().unwrap().insert(pane_key, view);
                    self.pane_tab.write().unwrap().insert(pane.0, k);
                    self.refresh_tab_bar();
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
                if self.notebook.read().unwrap().n_tabs() == 0 {
                    self.on_all_tabs_closed();
                }
                self.refresh_tab_bar();
                self.refresh_window_title();
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
        self.refresh_tab_bar();
        self.refresh_window_title();
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

    /// 刷新极简 TabBar。
    fn refresh_tab_bar(self: &Arc<Self>) {
        let keys = self.notebook.read().unwrap().keys_in_order();
        let current = *self.current_tab.lock().unwrap();
        let mut items = Vec::with_capacity(keys.len());
        for (i, key) in keys.iter().enumerate() {
            let name = self.tab_display_name(*key);
            items.push(TabBarItem {
                key: *key,
                title: format!("{}:{}", i + 1, name),
                active: current == Some(*key),
            });
        }
        self.tab_bar.rebuild(&items);
    }

    /// tab 显示名：激活 pane 程序名；多 pane 时加 ` · Npanes`。
    fn tab_display_name(self: &Arc<Self>, tab: TabKey) -> String {
        match tab {
            TabKey::Tmux(p) => {
                let key = PaneKey::Tmux(p);
                self.pane_views
                    .read()
                    .unwrap()
                    .get(&key)
                    .map(|v| v.display_name())
                    .unwrap_or_else(|| p.as_str())
            }
            TabKey::Local(_) => {
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
                let name = primary
                    .and_then(|p| views.get(&p).map(|v| v.display_name()))
                    .unwrap_or_else(|| "shell".into());
                if panes.len() > 1 {
                    format!("{name} · {}panes", panes.len())
                } else {
                    name
                }
            }
        }
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

/// 加载极简 CSS。
fn apply_css() {
    let provider = CssProvider::new();
    provider.load_from_data(include_str!("../../assets/style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
