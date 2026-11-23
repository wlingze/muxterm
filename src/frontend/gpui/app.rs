//! 启动装配与根视图：`muxterm gpui` 入口。
//!
//! Core handle 在 gpui 主线程创建并持有，之后只被 EventPump 消费。

use gpui::{
    div, prelude::*, px, size, AnyElement, App, Application, Bounds, Context, Render, Window,
    WindowBounds, WindowOptions,
};

use crate::frontend::command_queue::CommandQueue;
use crate::frontend::event_pump::EventPump;
use crate::frontend::gpui::chrome::pane_grid::{self, Rect};
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::utils::corebridge::FfiClient;
use crate::frontend::view_store::ViewStore;

/// 侧栏宽度默认值与拖动范围见 `model/sidebar_layout`。
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
        // 内置字体：Linux / Windows 上没有 Menlo / JetBrains Mono，不注册就只能拿
        // 系统比例字体，终端列宽立刻漂（见 `docs/FRONTEND-GPUI.md` §8.1）。
        register_bundled_fonts(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.on_app_quit(|_| {
            #[cfg(target_os = "macos")]
            crate::frontend::gpui::platform::macos_keys::uninstall();
            async {}
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1_280.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // macOS：隐藏标题文字，只留 traffic lights。
            // Linux / Windows：标题栏由系统画，gpui 不接管。
            titlebar: crate::frontend::gpui::platform::titlebar_options(),
            // Wayland / XDG 的应用标识：desktop 文件、任务栏分组都靠它。
            app_id: Some("dev.muxterm.app".to_string()),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.quit();
                true
            });
            let view = cx.new(|cx| GpuiApp::new(pump, store, commands, connect_socket.clone(), cx));
            let handle = view.read(cx).focus_handle();
            window.focus(&handle);
            // macOS 原生菜单栏：结构与 Swift 的 `buildMenu` 一致。
            if let Some(receiver) = crate::frontend::gpui::platform::install_menu() {
                view.update(cx, |app, cx| app.watch_menu(receiver, cx));
            }
            // 物理键位记录器：Cmd 组合按物理键位匹配，不受 Colemak / Dvorak 影响。
            crate::frontend::gpui::platform::install_physical_key_tracker();
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
        // 侧栏分区与拖动夹取都读窗口尺寸：每帧写一次，比到处传参数简单。
        self.window_width = width;
        self.window_height = height;
        // 客户端装饰（GNOME Wayland）没有系统标题栏：由 `chrome/title_bar.rs`
        // 画一条带最小化/最大化/关闭的。X11 / Windows 这里是 false。
        self.client_title_bar =
            crate::frontend::gpui::platform::needs_client_title_bar(window.window_decorations());
        self.measure_cell(window);
        // 窗口标题：关掉 `[ui] show_title_bar` 就写空串（只剩 traffic lights）。
        let title = crate::frontend::gpui::state::window_title_text(
            self.show_title_bar(),
            &self.visible_workspace_name(),
        );
        if title != self.applied_window_title() {
            window.set_window_title(&title);
            self.set_applied_window_title(title);
        }
        let theme = self.theme().clone();
        let (cell_width, cell_height) = self.cell();

        let sidebar_width = if self.sidebar_visible() {
            // 与侧栏渲染用同一个夹取函数，正文 origin 不能和实际宽度错位。
            crate::frontend::gpui::model::sidebar_layout::clamp_sidebar_width(
                self.sidebar_width,
                width,
                320.0,
            )
        } else {
            0.0
        };
        let main_width = (width - sidebar_width).max(1.0);
        // 顶部 chrome：非 macOS 有自绘菜单条，CSD 平台还有自绘标题栏。
        let top_chrome = self.top_chrome_height();
        let main_height = (height - STATUS_BAR_HEIGHT - top_chrome).max(1.0);
        let area = Rect::new(0.0, 0.0, main_width, main_height);

        let mut sizes: Vec<(u32, u16, u16)> = Vec::new();
        let mut panes: Vec<AnyElement> = Vec::new();
        let bar_on_top = self.tab_bar_position()
            == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top;
        // 鼠标事件是窗口坐标：内容区原点是侧栏宽度 + 顶部（菜单条 + bar）高度。
        let content_origin = (
            sidebar_width,
            top_chrome + if bar_on_top { STATUS_BAR_HEIGHT } else { 0.0 },
        );
        if let Some(layout) = self.layout() {
            for (pane_id, rect) in pane_grid::pane_rects(&layout, area) {
                let cols = (rect.max_width() / cell_width).floor().max(1.0) as u16;
                let rows = (rect.max_height() / cell_height).floor().max(1.0) as u16;
                sizes.push((pane_id, cols, rows));
                panes.push(self.render_pane(pane_id, rect, content_origin, cx));
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
        let bar = self.render_status_bar(cx);
        let (bar_top, bar_bottom) = if bar_on_top {
            (bar, div().into_any_element())
        } else {
            (div().into_any_element(), bar)
        };

        // CSD 标题栏（只有客户端装饰平台有；其它平台返回 None）。
        let title_bar = self.render_title_bar(cx);
        // 非 macOS 的自绘菜单条（macOS 是原生 NSMenu，两个都返回 None）。
        let menu_bar = self.render_menu_bar(cx);
        let menu_panel = self.render_menu_panel(cx);

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.fg)
            .track_focus(&self.focus_handle())
            .on_key_down(cx.listener(|this, event, window, cx| this.handle_key(event, window, cx)))
            // Command 按下/松开：悬停链接的下划线要立刻出现/消失，不能等下一次
            // 鼠标移动（用户是「先按住 Cmd，再悬停」）。
            // Linux / Windows 的命令键来自配置（缺省 Alt），不是 Super/Win。
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, _, cx| {
                    this.command_modifiers_changed(
                        crate::frontend::gpui::platform::command_held(&event.modifiers),
                        cx,
                    );
                },
            ))
            .children(title_bar)
            .children(menu_bar)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
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
                                    // 分屏拖拽带露出的底色：Swift 是 `windowBackgroundColor`，
                                    // 所以 pane 之间看得到一条 chrome 色，而不是连成一片。
                                    .bg(theme.chrome_bg)
                                    .children(panes)
                                    .children(self.render_pane_menu(area, cx))
                                    .children(self.render_panel(window, cx))
                                    .children(self.render_composer(cx))
                                    .children(self.render_settings(window, cx))
                                    .when(!self.settings_project_editor, |element| {
                                        element.children(self.render_target_editor(cx))
                                    })
                                    .children(self.render_palette(cx))
                                    .children(self.render_info(cx))
                                    .children(self.render_prompt(cx))
                                    .children(
                                        crate::frontend::gpui::chrome::info::render_image_paste_banner(
                                            self, &theme, cx,
                                        ),
                                    ),
                            )
                            .child(bar_bottom),
                    ),
            )
            // 菜单下拉面板要盖在正文上（含侧栏），所以放在 body 之后。
            .children(menu_panel)
            .children(self.render_selection_menu(cx))
            // 全窗口遮罩要盖过侧栏与正文；否则宽度拖动回到侧栏后收不到松手。
            .child(self.render_sidebar_drag_overlay(cx))
    }
}

/// 注册仓库自带的等宽字体与符号回退。
///
/// `assets/fonts/JetBrainsMono-Regular.ttf` 是 Core 配置的缺省族名
/// （`JetBrains Mono`），Linux / Windows 上系统一般没有；
/// `NotoSansSymbols2` 补充箭头 / 图形字符。注册是幂等的（平台文本系统按 family 去重）。
fn register_bundled_fonts(cx: &mut App) {
    use std::borrow::Cow;
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice()),
        Cow::Borrowed(
            include_bytes!("../../../assets/fonts/NotoSansSymbols2-Regular.ttf").as_slice(),
        ),
    ];
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        // 字体缺失不是致命错误：回退栈里还有系统等宽字体，只影响字形。
        tracing::warn!(%error, "注册内置字体失败");
    }
}
