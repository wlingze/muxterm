//! 主窗口：Notebook + 输入栏 + 状态栏。
//!
//! 维护 pane id → window id 映射、当前 session 名，把 tmux 事件落到 UI。
//! 所有事件处理在 UI 线程内完成（`on_event` 由 wiring 的轮询 source 派发）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use gtk4::prelude::*;
use gtk4::{Box, Label, Orientation, Window};

use crate::config::Theme;
use crate::tmux::client::TmuxClientConfig;
use crate::tmux::protocol::PaneId;
use crate::ui::input_bar::InputBar;
use crate::ui::notebook::PaneNotebook;
use crate::ui::pane_view::PaneView;
use crate::ui::wiring::{spawn_bridge, CommandSender, UiEvent};

/// 主窗口控制器。
pub struct AppWindow {
    pub window: Window,
    notebook: Arc<RwLock<PaneNotebook>>,
    input_bar: Arc<InputBar>,
    status_label: Label,
    /// pane id → window id。
    pane_window: Arc<RwLock<HashMap<u32, u32>>>,
    /// pane id → PaneView（持有 vte4 Terminal 引用，用于 feed 输出）。
    pane_views: Arc<RwLock<HashMap<u32, PaneView>>>,
    /// 当前 session 名。
    session_name: Arc<RwLock<Option<String>>>,
    /// 配置缓存。
    cfg: Arc<ConfigCache>,
    /// 命令发送器。
    cmd_sender: Arc<Mutex<Option<CommandSender>>>,
}

#[derive(Clone)]
struct ConfigCache {
    theme: Theme,
    font_family: String,
    font_size: u32,
    scrollback_lines: u32,
}

impl AppWindow {
    /// 构造主窗口（含所有 UI 控件，但不连 tmux；连接由 `connect` 触发）。
    pub fn new(theme: Theme, font_family: &str, font_size: u32, scrollback_lines: u32) -> Self {
        let window = Window::builder()
            .title("muxterm — session: ?")
            .default_width(1000)
            .default_height(650)
            .build();

        let root = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(0)
            .build();

        let notebook = Arc::new(RwLock::new(PaneNotebook::new()));
        let input_bar = Arc::new(InputBar::new());

        let status_label = Label::builder()
            .label("状态：未连接")
            .halign(gtk4::Align::Start)
            .margin_start(6)
            .margin_end(6)
            .margin_top(2)
            .margin_bottom(2)
            .build();
        status_label.add_css_class("status-bar");

        root.append(&notebook.read().unwrap().notebook);
        root.append(&input_bar.container);
        root.append(&status_label);
        window.set_child(Some(&root));

        let cfg = Arc::new(ConfigCache {
            theme,
            font_family: font_family.to_string(),
            font_size,
            scrollback_lines,
        });

        let app_win = Self {
            window,
            notebook,
            input_bar,
            status_label,
            pane_window: Arc::new(RwLock::new(HashMap::new())),
            pane_views: Arc::new(RwLock::new(HashMap::new())),
            session_name: Arc::new(RwLock::new(None)),
            cfg,
            cmd_sender: Arc::new(Mutex::new(None)),
        };

        // 输入栏接线：dispatcher 把命令字符串送后台写循环。
        // dispatcher 和 current_pane 都是 Send+Sync 闭包（不持有 GTK 对象）。
        let dispatcher: Arc<dyn Fn(&str) + Send + Sync> = {
            let sender = app_win.cmd_sender.clone();
            Arc::new(move |line: &str| {
                if let Ok(g) = sender.lock() {
                    if let Some(s) = g.as_ref() {
                        s.send(line);
                    }
                }
            })
        };
        let current_pane: Arc<dyn Fn() -> Option<PaneId> + Send + Sync> = {
            // 不直接访问 Notebook（非 Send），而是用一个共享的原子索引快照，
            // 由 UI 线程的 switch-page 回调更新。这里用 Arc<Mutex<Option<u32>>>。
            let current = Arc::new(Mutex::new(None::<u32>));
            let current_for_cb = current.clone();
            let nb = app_win.notebook.clone();
            {
                let nbook = nb.read().unwrap().notebook.clone();
                let current = current_for_cb.clone();
                nbook.connect_switch_page(move |_, _w, page_num| {
                    // page_num 是切换后的页面索引；反查 pane id
                    let pane = nb
                        .read()
                        .unwrap()
                        .panes()
                        .into_iter()
                        .find(|(_, idx)| *idx == page_num)
                        .map(|(p, _)| p.0);
                    *current.lock().unwrap() = pane;
                });
            }
            Arc::new(move || current.lock().unwrap().map(PaneId))
                as Arc<dyn Fn() -> Option<PaneId> + Send + Sync>
        };
        app_win.input_bar.wire(dispatcher, current_pane);

        app_win
    }

    /// 连接 tmux（启动桥接）。
    pub fn connect(&self, config: TmuxClientConfig) {
        let notebook = self.notebook.clone();
        let pane_views = self.pane_views.clone();
        let pane_window = self.pane_window.clone();
        let session_name = self.session_name.clone();
        let status_label = self.status_label.clone();
        let window = self.window.clone();
        let cfg = self.cfg.clone();
        let input_bar = self.input_bar.clone();

        let on_event = move |ev: &UiEvent| match ev {
            UiEvent::Connected => {
                status_label.set_label("状态：已连接 | session: ? | panes: 0");
            }
            UiEvent::Error { msg } => {
                let dlg = gtk4::MessageDialog::builder()
                    .transient_for(&window)
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
                // 懒建 pane 视图
                if !pane_views.read().unwrap().contains_key(&pane.0) {
                    let view = PaneView::new(
                        *pane,
                        &cfg.theme,
                        &cfg.font_family,
                        cfg.font_size,
                        cfg.scrollback_lines,
                    );
                    let title = PaneNotebook::default_title(*pane, None);
                    let widget = view.terminal.clone().upcast::<gtk4::Widget>();
                    notebook.write().unwrap().add_pane(*pane, widget, &title);
                    pane_views.write().unwrap().insert(pane.0, view);
                }
                if let Some(view) = pane_views.read().unwrap().get(&pane.0) {
                    view.feed_output(data);
                }
            }
            UiEvent::WindowAdd { .. } => {
                // tmux new-session 自带 window；pane 视图懒建在首次 %output。
            }
            UiEvent::WindowClose { window } => {
                let pw = pane_window.read().unwrap();
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
                    notebook.write().unwrap().remove_pane(p);
                    pane_views.write().unwrap().remove(&p.0);
                }
                let pane = notebook.read().unwrap().current_pane();
                input_bar.set_target(pane);
            }
            UiEvent::WindowRenamed { window, name } => {
                let pw = pane_window.read().unwrap();
                if let Some(pane) = pw.iter().find(|(_, w)| **w == window.0).map(|(p, _)| *p) {
                    notebook.read().unwrap().set_title(PaneId(pane), name);
                }
            }
            UiEvent::SessionChanged { sid, name } => {
                *session_name.write().unwrap() = name.clone();
                let title = match name {
                    Some(n) => format!("muxterm — session: {n}"),
                    None => format!("muxterm — session: ${sid}"),
                };
                window.set_title(Some(&title));
                let pane_count = notebook.read().unwrap().pane_count();
                let nm = name.clone().unwrap_or_else(|| format!("${sid}"));
                status_label.set_label(&format!(
                    "状态：已连接 | session: {nm} | panes: {pane_count}"
                ));
            }
            UiEvent::Exit { reason } => {
                let msg = match reason {
                    Some(r) => format!("状态：已断开（{r}）"),
                    None => "状态：已断开".to_string(),
                };
                status_label.set_label(&msg);
            }
        };

        if let Some(sender) = spawn_bridge(config, on_event) {
            if let Ok(mut g) = self.cmd_sender.lock() {
                *g = Some(sender);
            }
        }
    }
}
