//! 状态栏：workspace / runtime / focus pane / 网格尺寸。纯文本，不交互。

use gpui::{div, prelude::*, px, AnyElement, SharedString};

use crate::frontend::gpui::state::GpuiApp;

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

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(22.0))
            .px_3()
            .bg(theme.chrome_bg)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted)
            .child(SharedString::from(text))
            .into_any_element()
    }
}
