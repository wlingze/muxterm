//! 启动装配与根视图：`muxterm gpui` 入口。
//!
//! Core handle 在 gpui 主线程创建并持有，之后只被 EventPump 消费。

use gpui::{
    div, prelude::*, px, size, AnyElement, App, Application, Bounds, Context, Render,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::frontend::command_queue::CommandQueue;
use crate::frontend::event_pump::EventPump;
use crate::frontend::gpui::chrome::pane_grid::{self, Rect};
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::utils::corebridge::FfiClient;
use crate::frontend::view_store::ViewStore;

/// 侧栏宽度：与 macOS `mainSplitController.setPosition(240)` 一致。
const SIDEBAR_WIDTH: f32 = 240.0;
/// tab 栏 / 状态栏高度统一从 chrome 常量取，避免两处漂移。
use crate::frontend::gpui::chrome::status_bar::STATUS_BAR_HEIGHT;

/// 进入 gpui 前端。
pub fn run(socket: Option<String>, session: Option<String>) -> anyhow::Result<()> {
    let connect_socket = socket.clone();
    let client = connect(socket, session)?;
    let pump = EventPump::new(client);
    let mut store = ViewStore::default();
    pump.sync_view_store(&mut store)?;
    let commands = CommandQueue::default();

    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| cx.quit()).detach();
        let bounds = Bounds::centered(None, size(px(1_280.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // macOS 侧 `titleVisibility` 是隐藏的：这里同样不画标题，只留 traffic lights。
            titlebar: Some(TitlebarOptions {
                title: Some(" ".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| GpuiApp::new(pump, store, commands, connect_socket.clone(), cx));
            let handle = view.read(cx).focus_handle();
            window.focus(&handle);
            // macOS 原生菜单栏：结构与 Swift 的 `buildMenu` 一致。
            if let Some(receiver) = crate::frontend::gpui::platform::install_menu() {
                view.update(cx, |app, cx| app.watch_menu(receiver, cx));
            }
            view
        })
        .expect("gpui 窗口必须能创建");
        cx.activate(true);
    });
    Ok(())
}

fn connect(socket: Option<String>, session: Option<String>) -> anyhow::Result<FfiClient> {
    match socket {
        Some(socket) => {
            FfiClient::new_connect("tmux", Some(&socket), session.as_deref(), None, None)
        }
        None => FfiClient::new_connect("local", None, session.as_deref(), None, Some("")),
    }
}

impl Render for GpuiApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height);
        self.measure_cell(window);
        let theme = self.theme().clone();
        let (cell_width, cell_height) = self.cell();

        let sidebar_width = if self.sidebar_visible() {
            SIDEBAR_WIDTH
        } else {
            0.0
        };
        let main_width = (width - sidebar_width).max(1.0);
        // 只有一条 bar（tab + 状态），高度统一取 STATUS_BAR_HEIGHT。
        let main_height = (height - STATUS_BAR_HEIGHT).max(1.0);
        let area = Rect::new(0.0, 0.0, main_width, main_height);

        let mut sizes: Vec<(u32, u16, u16)> = Vec::new();
        let mut panes: Vec<AnyElement> = Vec::new();
        if let Some(layout) = self.layout() {
            for (pane_id, rect) in pane_grid::pane_rects(&layout, area) {
                let cols = (rect.max_width() / cell_width).floor().max(1.0) as u16;
                let rows = (rect.max_height() / cell_height).floor().max(1.0) as u16;
                sizes.push((pane_id, cols, rows));
                panes.push(self.render_pane(pane_id, rect, cx));
            }
        }
        self.apply_sizes(&sizes);
        // 窗口级网格：tmux 控制 client 的首屏 seed 要等这一条。
        self.apply_client_size(
            (main_width / cell_width).floor().max(2.0) as u16,
            (main_height / cell_height).floor().max(1.0) as u16,
        );

        let sidebar = if self.sidebar_visible() {
            self.render_sidebar(cx)
        } else {
            div().w(px(0.0)).into_any_element()
        };
        let bar_on_top = self.tab_bar_position()
            == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top;
        let bar = self.render_status_bar(cx);
        let (bar_top, bar_bottom) = if bar_on_top {
            (bar, div().into_any_element())
        } else {
            (div().into_any_element(), bar)
        };

        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.fg)
            .track_focus(&self.focus_handle())
            .on_key_down(cx.listener(|this, event, window, cx| this.handle_key(event, window, cx)))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(bar_top)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .bg(theme.bg)
                            .children(panes)
                            .children(self.render_panel(cx))
                            .children(self.render_composer(cx))
                            .children(self.render_settings(cx))
                            .children(self.render_target_editor(cx))
                            .children(self.render_palette(cx))
                            .children(self.render_info(cx)),
                    )
                    .child(bar_bottom),
            )
    }
}
