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

const SIDEBAR_WIDTH: f32 = 200.0;
const TAB_BAR_HEIGHT: f32 = 30.0;
const STATUS_BAR_HEIGHT: f32 = 22.0;

/// 进入 gpui 前端。
pub fn run(socket: Option<String>, session: Option<String>) -> anyhow::Result<()> {
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
            titlebar: Some(TitlebarOptions {
                title: Some("Muxterm".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let view = cx
            .open_window(options, |window, cx| {
                let view = cx.new(|cx| GpuiApp::new(pump, store, commands, cx));
                let handle = view.read(cx).focus_handle();
                window.focus(&handle);
                view
            })
            .expect("gpui 窗口必须能创建");
        let _ = view;
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
        let theme = self.theme().clone();
        let cell_width = theme.cell_width();
        let cell_height = theme.cell_height();

        let sidebar_width = if self.sidebar_visible() {
            SIDEBAR_WIDTH
        } else {
            0.0
        };
        let main_width = (width - sidebar_width).max(1.0);
        let main_height = (height - TAB_BAR_HEIGHT - STATUS_BAR_HEIGHT).max(1.0);
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

        let sidebar = if self.sidebar_visible() {
            self.render_sidebar(cx)
        } else {
            div().w(px(0.0)).into_any_element()
        };
        let tab_bar = self.render_tab_bar(cx);
        let status_bar = self.render_status_bar();

        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.fg)
            .track_focus(&self.focus_handle())
            .on_key_down(cx.listener(|this, event, _, cx| this.handle_key(event, cx)))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(tab_bar)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .bg(theme.bg)
                            .children(panes),
                    )
                    .child(status_bar),
            )
    }
}
