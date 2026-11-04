//! 侧边栏：workspace 列表。点击只切可见 Scene，不查询 Core。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::state::GpuiApp;

impl GpuiApp {
    pub(crate) fn render_sidebar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme().clone();
        let visible = self.visible_workspace().map(str::to_string);
        let rows: Vec<AnyElement> = self
            .workspace_ids()
            .into_iter()
            .map(|workspace| {
                let active = visible.as_deref() == Some(workspace.as_str());
                let name = self.workspace_name(&workspace);
                let runtime = self.workspace_runtime(&workspace);
                let id = workspace.clone();
                div()
                    .id(SharedString::from(format!("workspace-{workspace}")))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if active {
                        theme.active_bg
                    } else {
                        theme.sidebar_bg
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.focus(&this.focus_handle());
                            this.select_workspace(id.clone(), cx);
                        }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.fg)
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if active { theme.accent } else { theme.muted })
                            .child(SharedString::from(runtime)),
                    )
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(200.0))
            .h_full()
            .p_2()
            .bg(theme.sidebar_bg)
            .border_r_1()
            .border_color(theme.border)
            .children(rows)
            .into_any_element()
    }
}
