//! tab 栏：当前 workspace 的 tab 行。多 pane 只加数量后缀，不做每 pane 一个 tab。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::state::GpuiApp;

impl GpuiApp {
    pub(crate) fn render_tab_bar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme().clone();
        let tabs = self.tabs();
        let pane_counts = self.pane_counts();
        let rows: Vec<AnyElement> = tabs
            .iter()
            .map(|tab| {
                let active = tab.is_active;
                let label = match pane_counts.get(&tab.id) {
                    Some(count) if *count > 1 => format!("{} ({count})", tab.name),
                    _ => tab.name.clone(),
                };
                let tab_id = tab.id;
                div()
                    .id(SharedString::from(format!("tab-{}", tab.id)))
                    .flex()
                    .items_center()
                    .px_3()
                    .h_full()
                    .cursor_pointer()
                    .bg(if active { theme.bg } else { theme.chrome_bg })
                    .text_color(if active { theme.fg } else { theme.muted })
                    .text_sm()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.select_tab(tab_id, cx)),
                    )
                    .child(SharedString::from(label))
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(30.0))
            .px_2()
            .bg(theme.chrome_bg)
            .border_b_1()
            .border_color(theme.border)
            .children(rows)
            .into_any_element()
    }
}
