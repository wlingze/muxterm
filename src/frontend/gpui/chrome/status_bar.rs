//! 状态栏：Swift 只有一条 24pt 的 bar，里面同时放 workspace、tab 和右侧状态。
//!
//! 对照 macOS `UI/StatusBarView.swift`：
//! - 高度 `FlatChrome.tabBarHeight`（24pt），左右内边距 6pt；
//! - 左段：workspace 名（活动 tab 之外唯一的身份提示）；
//! - 中段：tab 单元，`TabBarPosition` 决定整条 bar 在上还是在下；
//! - 右段：焦点 pane 的网格尺寸与注意力徽标（`StatusBarAttention`）。
//!
//! 之前 gpui 把 tab 栏和状态栏拆成两条（24 + 18pt），比 Swift 多一条分隔线和
//! 一行重复文字；这里合并成一条，视觉与 `ContentView` 一致。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::tab_bar::{TAB_BAR_HEIGHT, TAB_BAR_INSET};
use crate::frontend::gpui::model::sidebar::Indicator;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::{Theme, TAB_BAR_FONT_SIZE};

/// 状态栏高度 = tab 栏高度：一条 bar 承担两者。
pub const STATUS_BAR_HEIGHT: f32 = TAB_BAR_HEIGHT;

/// 右侧控制按钮尺寸（对照 Swift `StatusBarView.updateTrailingButtonsVisibility`）：
/// 状态点 18、铃铛 / 全屏 / 设置各 24、新建 28。
pub const STATUS_DOT_WIDTH: f32 = 18.0;
pub const STATUS_BUTTON_WIDTH: f32 = 24.0;
pub const NEW_TAB_BUTTON_WIDTH: f32 = 28.0;
/// 左侧侧栏开关（Swift `sidebarToggleButton`：24×20）。
pub const SIDEBAR_TOGGLE_WIDTH: f32 = 24.0;

/// 连接状态点的颜色：Swift `StatusDotButton` 用系统语义色
///（connected 绿、connecting 黄、error / exited 红、其余灰）。
pub fn connection_color(status: u32, theme: &Theme) -> gpui::Rgba {
    use crate::frontend::gpui::model::sidebar::backend_status;
    match status {
        backend_status::CONNECTED => theme.success,
        backend_status::CONNECTING => theme.warning,
        backend_status::ERROR | backend_status::EXITED => theme.danger,
        _ => theme.muted,
    }
}

/// 一个右侧图标按钮。`id` 同时是 debug selector，测试按它点击。
fn status_button(
    id: &'static str,
    glyph: impl Into<gpui::SharedString>,
    color: gpui::Rgba,
    width: f32,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
    on_click: impl Fn(&mut GpuiApp, &mut gpui::Window, &mut gpui::Context<GpuiApp>) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .debug_selector(move || id.to_string())
        .flex_none()
        .w(px(width))
        .h(px(STATUS_BAR_HEIGHT - 4.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(px(TAB_BAR_FONT_SIZE + 1.0))
        .text_color(color)
        .hover(|style| style.bg(theme.active_bg))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, window, cx| {
                cx.stop_propagation();
                on_click(app, window, cx);
            }),
        )
        .child(glyph.into())
        .into_any_element()
}

impl GpuiApp {
    /// 唯一的窗口底（或顶）栏杆：workspace 名 + tab 单元 + 右侧状态。
    pub(crate) fn render_status_bar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme().clone();
        let workspace = self.visible_workspace().unwrap_or_default().to_string();
        let name = self.workspace_name(&workspace);
        let runtime = self.workspace_runtime(&workspace);
        let left = if runtime.is_empty() {
            name
        } else {
            format!("{name} · {runtime}")
        };
        // 直接读 Surface 的实时网格：store 里的 cols/rows 要等 Core 下一个快照，
        // 窗口刚变大时会停在旧值（状态栏看起来「不更新」）。
        let right_text = self.pane_geometry_label();
        // `statusbar.mode = tmux` 时右侧换成 tmux 自己的 status-right；
        // `theme` 模式用前端主题文案（下面 right_text）。
        let (tmux_left, tmux_right) = self
            .tmux_status_segments()
            .map(|(left, right)| (Some(left), Some(right)))
            .unwrap_or((None, None));
        let tmux_right_text = tmux_right.filter(|text| !text.trim().is_empty());
        let tmux_left_text = tmux_left.filter(|text| !text.trim().is_empty());

        let attention = self.status_attention();
        let attention_color = attention
            .indicator
            .map(|indicator| indicator_color(indicator, &theme))
            .unwrap_or(theme.muted);
        // 铃铛：没有未读时是灰色空铃；有未读显示数量并按主导色着色。
        let attention_glyph = if attention.is_active() {
            format!("◔{}", attention.count)
        } else {
            String::from("◔")
        };
        let attention_button = status_button(
            "status-attention",
            SharedString::from(attention_glyph),
            attention_color,
            STATUS_BUTTON_WIDTH,
            &theme,
            cx,
            |app, _, cx| {
                app.show_panel(crate::frontend::gpui::model::panel::PanelTab::Attention, cx);
            },
        );
        let connection = connection_color(self.visible_backend_status(), &theme);
        let status_dot = status_button(
            "status-dot",
            "●",
            connection,
            STATUS_DOT_WIDTH,
            &theme,
            cx,
            |app, _, cx| {
                app.open_info(cx);
            },
        );
        let fullscreen_button = status_button(
            "status-fullscreen",
            "⛶",
            theme.muted,
            STATUS_BUTTON_WIDTH,
            &theme,
            cx,
            |app, window, cx| {
                app.run_action(
                    crate::frontend::gpui::model::shortcuts::KeyAction::ToggleWindowFullscreen,
                    window,
                    cx,
                );
            },
        );
        let settings_button = status_button(
            "status-settings",
            "⚙",
            theme.muted,
            STATUS_BUTTON_WIDTH,
            &theme,
            cx,
            |app, window, cx| {
                app.run_action(
                    crate::frontend::gpui::model::shortcuts::KeyAction::OpenSettings,
                    window,
                    cx,
                );
            },
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .debug_selector(|| "status-bar".to_string())
            .h(px(STATUS_BAR_HEIGHT))
            .px(px(TAB_BAR_INSET))
            .overflow_hidden()
            .bg(theme.chrome_bg)
            .border_color(theme.border)
            .when(
                self.tab_bar_position()
                    == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Top,
                |element| element.border_b_1(),
            )
            .when(
                self.tab_bar_position()
                    == crate::frontend::gpui::chrome::tab_bar_view::TabBarPosition::Bottom,
                |element| element.border_t_1(),
            )
            .child(status_button(
                "sidebar-toggle",
                if self.sidebar_visible() { "◧" } else { "▭" },
                theme.muted,
                SIDEBAR_TOGGLE_WIDTH,
                &theme,
                cx,
                |app, window, cx| {
                    app.run_action(
                        crate::frontend::gpui::model::shortcuts::KeyAction::ToggleSidebar,
                        window,
                        cx,
                    );
                },
            ))
            .child(
                div()
                    .flex_none()
                    .max_w(px(200.0))
                    .overflow_hidden()
                    .text_size(px(TAB_BAR_FONT_SIZE))
                    .line_height(px(14.0))
                    .text_color(theme.label())
                    .child(SharedString::from(left)),
            )
            .child(div().flex_none().w(px(1.0)).h(px(12.0)).bg(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .flex_none()
                    .children(self.render_tabs(cx)),
            )
            .when_some(tmux_left_text, |element, text| {
                element.child(
                    div()
                        .id("tmux-status-left")
                        .debug_selector(|| "tmux-status-left".into())
                        .flex_none()
                        .max_w(px(240.0))
                        .overflow_hidden()
                        .text_size(px(TAB_BAR_FONT_SIZE))
                        .line_height(px(14.0))
                        .text_color(theme.muted)
                        .child(SharedString::from(text)),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .max_w(px(200.0))
                    .overflow_hidden()
                    .text_size(px(TAB_BAR_FONT_SIZE))
                    .line_height(px(14.0))
                    .text_color(theme.muted)
                    .child(SharedString::from(tmux_right_text.unwrap_or(right_text))),
            )
            // 右端顺序照 Swift：状态点 → 铃铛 → 全屏 → 设置 → 新建。
            .child(status_dot)
            .child(attention_button)
            .child(fullscreen_button)
            .child(settings_button)
            .child({
                let theme = theme.clone();
                div()
                    .id("new-tab")
                    .debug_selector(|| "new-tab".into())
                    .flex_none()
                    .w(px(NEW_TAB_BUTTON_WIDTH))
                    .h(px(STATUS_BAR_HEIGHT - 6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(px(TAB_BAR_FONT_SIZE + 2.0))
                    .text_color(theme.muted)
                    .hover(|style| style.bg(theme.active_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.run_action(
                                crate::frontend::gpui::model::shortcuts::KeyAction::NewTab,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(SharedString::from("+"))
                    .into_any_element()
            })
            .into_any_element()
    }
}

fn indicator_color(indicator: Indicator, theme: &Theme) -> gpui::Rgba {
    // 与 macOS `WorkspaceSidebar.indicatorColor` 完全一致：
    // working = systemYellow、blocked = systemPink、done = systemTeal。
    match indicator {
        Indicator::Done => theme.done,
        Indicator::Blocked => theme.blocked,
        Indicator::Working => theme.working,
        Indicator::Idle => theme.muted,
    }
}
