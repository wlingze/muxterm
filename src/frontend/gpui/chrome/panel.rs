//! 统一面板的渲染：tab 条 + query + 行列表。
//!
//! 行内容来自 `panel_view::PanelState::rows`（纯投影）。视觉上是一个居中的浮层，
//! 打开时盖住 pane 区域但不改布局；Esc 关闭由 `state.rs` 处理。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::panel_view::PanelRow;
use crate::frontend::gpui::model::panel::PanelTab;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

const PANEL_WIDTH: f32 = 640.0;
const PANEL_HEIGHT: f32 = 420.0;
const MAX_VISIBLE_ROWS: usize = 9;

impl GpuiApp {
    /// 面板浮层；面板没打开时返回空元素。
    pub(crate) fn render_panel(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.panel_open() {
            return None;
        }
        let theme = self.theme().clone();
        let tabs: Vec<AnyElement> = PanelTab::ALL
            .iter()
            .map(|tab| {
                let tab = *tab;
                let active = self.panel_tab() == tab;
                div()
                    .id(SharedString::from(format!("panel-tab-{:?}", tab)))
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_sm()
                    .bg(if active {
                        theme.active_bg
                    } else {
                        theme.chrome_bg
                    })
                    .text_color(if active { theme.fg } else { theme.muted })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.show_panel(tab, cx)),
                    )
                    .child(SharedString::from(panel_tab_title(tab)))
                    .into_any_element()
            })
            .collect();

        let new_project: Option<AnyElement> =
            (self.panel_tab() == PanelTab::Workspaces).then(|| {
                div()
                    .id("panel-new-project")
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme.accent)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_panel(cx);
                            this.open_target_editor(None, cx);
                        }),
                    )
                    .child(SharedString::from(PANEL_OPEN_LABEL))
                    .into_any_element()
            });

        let query = self.panel_query();
        let query_line = if query.is_empty() {
            "搜索 workspace 或 attach 现有 runtime…".to_string()
        } else {
            query.clone()
        };
        let selected = self.panel_selected();
        let rows: Vec<AnyElement> = self
            .panel_rows()
            .into_iter()
            .enumerate()
            .skip(selected.saturating_sub(MAX_VISIBLE_ROWS - 1))
            .take(MAX_VISIBLE_ROWS)
            .map(|(index, row)| panel_row(row, &theme, index == selected))
            .collect();

        let empty_hint = if rows.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.muted)
                .child(SharedString::from("no results"))
                .into_any_element()
        } else {
            div().into_any_element()
        };

        Some(
            div()
                .id("panel-overlay")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .bg(gpui::rgba(0x00000080))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_panel(cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .w(px(PANEL_WIDTH))
                        .h(px(PANEL_HEIGHT))
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.chrome_bg)
                        // 浮层内部点击不关闭面板。
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex().flex_row().gap_2().children(tabs))
                        .child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(theme.border)
                                .text_sm()
                                .text_color(if query.is_empty() {
                                    theme.muted
                                } else {
                                    theme.fg
                                })
                                .child(SharedString::from(query_line)),
                        )
                        .child(empty_hint)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .flex_1()
                                .overflow_hidden()
                                .children(rows)
                                .children(new_project),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// 面板根列表的行动作：Swift 的 `panelOpen`（"+ Open"），不再有 New project。
pub const PANEL_OPEN_LABEL: &str = "+ Open";

fn panel_row(row: PanelRow, theme: &Theme, selected: bool) -> AnyElement {
    let title = row.title();
    let detail = row.detail();
    // 现有 runtime 用 + 前缀区分「要 attach 的东西」和「已经打开的 workspace」。
    let is_existing = matches!(row, PanelRow::Existing { .. });
    let title = if is_existing {
        format!("+ {title}")
    } else {
        title
    };
    let active = matches!(row, PanelRow::Workspace { active: true, .. });
    div()
        .id(SharedString::from(format!("panel-row-{title}")))
        .flex()
        .flex_col()
        .gap_0p5()
        .px_3()
        .py_1()
        .rounded_sm()
        .bg(if selected {
            theme.active_bg
        } else if active {
            theme.active_bg
        } else {
            theme.chrome_bg
        })
        .when(selected, |element| {
            element.border_l_2().border_color(theme.accent)
        })
        .child(
            div()
                .text_sm()
                .text_color(if selected { theme.fg } else { theme.label() })
                .overflow_hidden()
                .child(SharedString::from(title)),
        )
        .when_some(detail, |element, detail| {
            element.child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .overflow_hidden()
                    .child(SharedString::from(detail)),
            )
        })
        .into_any_element()
}

fn panel_tab_title(tab: PanelTab) -> String {
    crate::frontend::utils::i18n::tr(match tab {
        PanelTab::Workspaces => crate::frontend::utils::i18n::TextKey::SidebarWorkspaces,
        PanelTab::Attention => crate::frontend::utils::i18n::TextKey::SidebarAgents,
        PanelTab::Search => crate::frontend::utils::i18n::TextKey::StatusAttention,
    })
}
