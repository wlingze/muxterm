//! 状态栏：左段是 workspace / runtime / 焦点 pane / 网格尺寸，右段是注意力徽标。
//!
//! 宽度预算与内联指令来自 `model::status_bar`（纯逻辑、已有单测）；这里只把
//! 结果铺成两段。Swift 侧开工在 `UI/StatusBarView.swift`。

use gpui::{div, prelude::*, px, AnyElement, SharedString};

use crate::frontend::gpui::model::sidebar::Indicator;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

const STATUS_BAR_HEIGHT: f32 = 22.0;

impl GpuiApp {
    pub(crate) fn render_status_bar(&self) -> AnyElement {
        let theme = self.theme().clone();
        let workspace = self.visible_workspace().unwrap_or_default().to_string();
        let name = self.workspace_name(&workspace);
        let runtime = self.workspace_runtime(&workspace);
        let geometry = self
            .focused_pane()
            .and_then(|pane| self.pane_geometry(pane))
            .map(|(cols, rows)| format!("{cols}x{rows}"))
            .unwrap_or_else(|| String::from("-"));
        let text = format!(
            "{name}  ·  {runtime}  ·  pane {}  ·  {geometry}",
            self.focused_pane().unwrap_or(0)
        );

        let attention = self.status_attention();
        let mut right: Vec<AnyElement> = Vec::new();
        if attention.is_active() {
            let color = attention
                .indicator
                .map(|indicator| indicator_color(indicator, &theme))
                .unwrap_or(theme.muted);
            right.push(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(color)
                    .child(SharedString::from(format!("◔ {}", attention.count)))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(STATUS_BAR_HEIGHT))
            .px_3()
            .bg(theme.chrome_bg)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted)
            .child(
                div()
                    .overflow_hidden()
                    .child(SharedString::from(text))
                    .into_any_element(),
            )
            .child(div().flex().flex_row().gap_2().children(right))
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
