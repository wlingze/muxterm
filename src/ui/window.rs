//! 主窗口。
//!
//! 顶部：工具栏（新建 tab + tmux 集成按钮 + session 名）+ Notebook tab 栏。
//! 中间：当前 tab 的 vte4 Terminal（本地 shell 直接可输入；tmux pane 显示输出）。
//! 底部：输入栏（仅对 tmux attach 的 pane 显示）+ 状态栏。
//!
//! 启动即一个本地 shell tab，用户可立刻敲命令。tmux 是可选的 attach 功能。
//!
//! 为了让多个闭包共享状态而不用借用 self，所有可变状态用 Arc 包裹，
//! `AppWindow` 构造完成后通过 `shared()` 产出 `Arc<SharedState>` 给闭包用。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use gtk4::prelude::*;
use gtk4::{Box, Button, Label, Orientation, Window};

use crate::config::Theme;
use crate::tmux::client::TmuxClientConfig;
use crate::tmux::protocol::PaneId;
use crate::ui::input_bar::InputBar;
use crate::ui::notebook::{PaneNotebook, TabKey};
use crate::ui::pane_view::PaneView;
use crate::ui::tmux_dialog::{self, TmuxAction};
use crate::ui::wiring::{spawn_bridge, CommandSender, UiEvent};

/// 主窗口控制器。
pub struct AppWindow {
    pub window: Window,
    shared: Arc<SharedState>,
}

/// 跨闭包共享的状态。
struct SharedState {
    notebook: Arc<RwLock<PaneNotebook>>,
    pane_views: Arc<RwLock<HashMap<TabKey, PaneView>>>,
    pane_window: Arc<RwLock<HashMap<u32, u32>>>,
    pane_tab: Arc<RwLock<HashMap<u32, TabKey>>>,
    session_name: Arc<RwLock<Option<String>>>,
    cfg: Arc<ConfigCache>,
    cmd_sender: Arc<Mutex<Option<CommandSender>>>,
    current_tab: Arc<Mutex<Option<TabKey>>>,
    input_bar: Arc<InputBar>,
    input_bar_container: gtk4::Box,
    status_label: Label,
    window: Window,
}

#[derive(Clone)]
struct ConfigCache {
    theme: Theme,
    font_family: String,
    font_size: u32,
    scrollback_lines: u32,
}

impl AppWindow {
    pub fn new(theme: Theme, font_family: &str, font_size: u32, scrollback_lines: u32) -> Self {
        let window = Window::builder()
            .title("muxterm")
            .default_width(1000)
            .default_height(650)
            .build();

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();

        // 工具栏：[新建 tab] [tmux 集成] | session 名
        let toolbar = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(4)
            .margin_start(4)
            .margin_end(4)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        let new_tab_btn = Button::with_label("+ 新建 tab");
        new_tab_btn.add_css_class("suggested-action");
        let tmux_btn = Button::with_label("tmux");
        let session_label = Label::new(Some("（本地 shell）"));
        session_label.set_halign(gtk4::Align::Start);
        session_label.set_hexpand(true);
        session_label.set_margin_start(8);
        toolbar.append(&new_tab_btn);
        toolbar.append(&tmux_btn);
        toolbar.append(&session_label);
        root.append(&toolbar);

        // Notebook
        let notebook = Arc::new(RwLock::new(PaneNotebook::new()));
        root.append(&notebook.read().unwrap().notebook);

        // 底部输入栏（仅 tmux pane 显示）
        let input_bar_container = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(0)
            .build();
        let input_bar = Arc::new(InputBar::new());
        input_bar_container.append(&input_bar.container);
        input_bar_container.set_visible(false);
        root.append(&input_bar_container);

        // 状态栏
        let status_label = Label::builder()
            .label("状态：本地 shell 模式")
            .halign(gtk4::Align::Start)
            .margin_start(6)
            .margin_end(6)
            .margin_top(2)
            .margin_bottom(2)
            .build();
        status_label.add_css_class("status-bar");
        root.append(&status_label);

        window.set_child(Some(&root));

        let cfg = Arc::new(ConfigCache {
            theme,
            font_family: font_family.to_string(),
            font_size,
            scrollback_lines,
        });

        let shared = Arc::new(SharedState {
            notebook,
            pane_views: Arc::new(RwLock::new(HashMap::new())),
            pane_window: Arc::new(RwLock::new(HashMap::new())),
            pane_tab: Arc::new(RwLock::new(HashMap::new())),
            session_name: Arc::new(RwLock::new(None)),
            cfg,
            cmd_sender: Arc::new(Mutex::new(None)),
            current_tab: Arc::new(Mutex::new(None)),
            input_bar,
            input_bar_container,
            status_label,
            window: window.clone(),
        });

        let app_win = AppWindow {
            window,
            shared: shared.clone(),
        };

        app_win.wire_toolbar(&new_tab_btn, &tmux_btn, &session_label);
        app_win.wire_notebook_switch();
        app_win.wire_input_bar();

        // 启动即一个本地 shell
        app_win.new_local_tab();

        app_win
    }

    /// 工具栏按钮接线。
    fn wire_toolbar(&self, new_tab_btn: &Button, tmux_btn: &Button, session_label: &Label) {
        {
            let shared = self.shared.clone();
            new_tab_btn.connect_clicked(move |_| {
                SharedState::new_local_tab(&shared);
            });
        }
        {
            let shared = self.shared.clone();
            let win = self.window.clone();
            let session_label = session_label.clone();
            tmux_btn.connect_clicked(move |_| {
                let session_label = session_label.clone();
                let shared = shared.clone();
                tmux_dialog::show(&win, move |action| {
                    let (config, label) = match action {
                        TmuxAction::Attach { session } => {
                            let cfg = crate::ui::wiring::attach_config(&session);
                            (cfg, format!("session: {session}"))
                        }
                        TmuxAction::NewSession { name } => {
                            let cfg = crate::ui::wiring::new_session_config(name.clone());
                            let label = match &name {
                                Some(n) => format!("session: {n}"),
                                None => "session: (new)".into(),
                            };
                            (cfg, label)
                        }
                    };
                    session_label.set_label(&label);
                    SharedState::connect_tmux(&shared, config);
                });
            });
        }
    }

    /// Notebook 切 tab 时：更新 current_tab 快照 + 输入栏可见性/目标 pane。
    fn wire_notebook_switch(&self) {
        let shared = self.shared.clone();
        let nbook = self.shared.notebook.read().unwrap().notebook.clone();
        nbook.connect_switch_page(move |_, _w, page_num| {
            SharedState::on_switch_page(&shared, page_num);
        });
    }

    /// 输入栏接线。
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
        let current_tab = self.shared.current_tab.clone();
        let current_pane: Arc<dyn Fn() -> Option<PaneId> + Send + Sync> = Arc::new(move || {
            current_tab.lock().unwrap().and_then(|k| match k {
                TabKey::Tmux(p) => Some(p),
                _ => None,
            })
        });
        self.shared.input_bar.wire(dispatcher, current_pane);
    }

    /// 新建一个本地 shell tab。
    fn new_local_tab(&self) {
        SharedState::new_local_tab(&self.shared);
    }
}

impl SharedState {
    /// 新建一个本地 shell tab。
    fn new_local_tab(self: &Arc<Self>) {
        let view = PaneView::new_local(
            &self.cfg.theme,
            &self.cfg.font_family,
            self.cfg.font_size,
            self.cfg.scrollback_lines,
        );
        let key = self.notebook.write().unwrap().add_pane(&view, "shell");
        self.pane_views.write().unwrap().insert(key, view);
        // 本地 shell tab：输入栏隐藏
        self.input_bar_container.set_visible(false);
        *self.current_tab.lock().unwrap() = Some(key);
    }

    /// 连接 tmux（按需触发）。
    fn connect_tmux(self: &Arc<Self>, config: TmuxClientConfig) {
        let shared = self.clone();
        let on_event = move |ev: &UiEvent| shared.handle_ui_event(ev);

        if let Some(sender) = spawn_bridge(config, on_event) {
            if let Ok(mut g) = self.cmd_sender.lock() {
                *g = Some(sender);
            }
        }
    }

    /// 处理一条 UI 事件（在 UI 线程被调用）。
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
                dlg.connect_response(move |_dlg, _| {
                    d.close();
                });
                dlg.show();
            }
            UiEvent::PaneOutput { pane, data } => {
                // 懒建 tmux pane 视图
                let key = TabKey::Tmux(*pane);
                if !self.pane_views.read().unwrap().contains_key(&key) {
                    let view = PaneView::new_tmux(
                        *pane,
                        &self.cfg.theme,
                        &self.cfg.font_family,
                        self.cfg.font_size,
                        self.cfg.scrollback_lines,
                    );
                    let title = PaneNotebook::default_title(key, None);
                    let k = self.notebook.write().unwrap().add_pane(&view, &title);
                    self.pane_views.write().unwrap().insert(k, view);
                    self.pane_tab.write().unwrap().insert(pane.0, k);
                }
                if let Some(view) = self.pane_views.read().unwrap().get(&key) {
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
                    let pt = self.pane_tab.read().unwrap().get(&p.0).copied();
                    if let Some(key) = pt {
                        self.notebook.write().unwrap().remove(key);
                        self.pane_views.write().unwrap().remove(&key);
                    }
                    self.pane_tab.write().unwrap().remove(&p.0);
                }
                self.refresh_input_visibility();
            }
            UiEvent::WindowRenamed { window, name } => {
                let pw = self.pane_window.read().unwrap();
                if let Some(pane) = pw.iter().find(|(_, w)| **w == window.0).map(|(p, _)| *p) {
                    let pt = self.pane_tab.read().unwrap().get(&pane).copied();
                    if let Some(key) = pt {
                        self.notebook.read().unwrap().set_title(key, name);
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
                let pane_count = self.notebook.read().unwrap().pane_count();
                let nm = name.clone().unwrap_or_else(|| format!("${sid}"));
                self.status_label.set_label(&format!(
                    "状态：已连接 | session: {nm} | panes: {pane_count}"
                ));
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

    /// Notebook 切 tab 回调。
    fn on_switch_page(self: &Arc<Self>, page_num: u32) {
        let key = self.notebook.read().unwrap().find_key_by_index(page_num);
        *self.current_tab.lock().unwrap() = key;
        self.refresh_input_visibility();
    }

    /// 根据当前 tab 更新输入栏可见性 + 目标 pane。
    fn refresh_input_visibility(self: &Arc<Self>) {
        let key = self.notebook.read().unwrap().current_key();
        match key {
            Some(k) => {
                let is_tmux = self
                    .pane_views
                    .read()
                    .unwrap()
                    .get(&k)
                    .map(|v| v.is_tmux())
                    .unwrap_or(false);
                if is_tmux {
                    let pane = match k {
                        TabKey::Tmux(p) => Some(p),
                        _ => None,
                    };
                    self.input_bar.set_target(pane);
                    self.input_bar_container.set_visible(true);
                } else {
                    self.input_bar_container.set_visible(false);
                }
            }
            None => self.input_bar_container.set_visible(false),
        }
    }
}
