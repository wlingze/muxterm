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
        let geometry = self
            .focused_pane()
            .and_then(|pane| self.pane_geometry(pane))
            .map(|(cols, rows)| format!("{cols}x{rows}"))
            .unwrap_or_else(|| String::from("-"));
        let active_pane = self.focused_pane().unwrap_or(0);
        let right_text = format!("pane {active_pane} · {geometry}");

        let attention = self.status_attention();
        let attention_badge = attention.is_active().then(|| {
            let color = attention
                .indicator
                .map(|indicator| indicator_color(indicator, &theme))
                .unwrap_or(theme.muted);
            div()
                .flex_none()
                .text_size(px(TAB_BAR_FONT_SIZE))
                .text_color(color)
                .child(SharedString::from(format!("◔ {}", attention.count)))
                .into_any_element()
        });

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
            .child(
                // `FlatChrome.newTabButtonWidth`：+ 按钮 28pt。
                div()
                    .id("new-tab")
                    .flex_none()
                    .w(px(28.0))
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
                    .child(SharedString::from("+")),
            )
            .child(div().flex_1())
            .when_some(attention_badge, |element, badge| element.child(badge))
            .child(
                div()
                    .flex_none()
                    .text_size(px(TAB_BAR_FONT_SIZE))
                    .line_height(px(14.0))
                    .text_color(theme.muted)
                    .child(SharedString::from(right_text)),
            )
            .into_any_element()
    }
}

fn indicator_color(indicator: Indicator, theme: &Theme) -> gpui::Rgba {
    match indicator {
        Indicator::Done => theme.success,
        Indicator::Blocked => theme.danger,
        Indicator::Working => theme.warning,
        Indicator::Idle => theme.muted,
    }
}
