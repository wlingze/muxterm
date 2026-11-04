//! 侧栏渲染：四段（Workspaces / Agents / Commands / Hidden Commands）。
//!
//! 行内容全部来自 `sidebar_view::build`（纯投影，带单测）；这里只负责铺像素和
//! 接点击。对照 macOS `App/WorkspaceSidebar.swift`：36pt 行、段头可折叠、
//! 命令行尾部按钮切换可见性、可关闭工作区尾部一个关闭按钮。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::chrome::sidebar_view::{
    CommandKey, RowTarget, Section, SidebarRow, SidebarSectionModel,
};
use crate::frontend::gpui::model::sidebar::Indicator;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;

/// 与 Swift `table.rowHeight` 一致。
const ROW_HEIGHT: f32 = 36.0;
const SIDEBAR_WIDTH: f32 = 200.0;

impl GpuiApp {
    pub(crate) fn render_sidebar(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme().clone();
        let sections: Vec<AnyElement> = self
            .sidebar_model()
            .into_iter()
            .map(|section| self.render_section(section, &theme, cx))
            .collect();

        div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .overflow_hidden()
            .py_1()
            .bg(theme.sidebar_bg)
            .border_r_1()
            .border_color(theme.border)
            .children(sections)
            .into_any_element()
    }

    fn render_section(
        &self,
        section: SidebarSectionModel,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let kind = section.section;
        // 空段只留段头：Swift 的表格段没有行时不占高度，但段头仍然可点。
        let mut children: Vec<AnyElement> = Vec::new();
        if section.expanded {
            children.extend(
                section
                    .rows
                    .into_iter()
                    .map(|row| self.render_sidebar_row(kind, row, theme, cx)),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .id(SharedString::from(format!("section-{:?}", kind)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(22.0))
                    .px_3()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme.muted)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_sidebar_section(kind, cx)),
                    )
                    .child(SharedString::from(section.title))
                    .child(SharedString::from(if section.expanded {
                        "▾"
                    } else {
                        "▸"
                    })),
            )
            .children(children)
            .into_any_element()
    }

    fn render_sidebar_row(
        &self,
        section: Section,
        row: SidebarRow,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let marker_color = indicator_color(row.indicator, theme);
        let detail_color = if row.detail_alert {
            theme.danger
        } else {
            theme.muted
        };
        let target = row.target.clone();
        let shortcut = row.shortcut.clone();
        let trailing = row.trailing;
        let command_key = match (&row.target, section) {
            (
                RowTarget::Pane {
                    workspace_id,
                    pane_id,
                    ..
                },
                Section::Commands | Section::HiddenCommands,
            ) => Some(CommandKey {
                workspace_id: workspace_id.clone(),
                pane_id: *pane_id,
            }),
            _ => None,
        };

        let mut trailing_buttons: Vec<AnyElement> = Vec::new();
        if let Some(symbol) = trailing {
            let label = match symbol {
                "xmark" => "✕",
                "eye" => "👁",
                "eye.slash" => "🚫",
                _ => symbol,
            };
            let close_target = if symbol == "xmark" {
                match &row.target {
                    RowTarget::Workspace(id) => Some(id.clone()),
                    _ => None,
                }
            } else {
                None
            };
            trailing_buttons.push(
                div()
                    .id(SharedString::from(format!(
                        "trailing-{:?}-{}-{}",
                        section, row.title, symbol
                    )))
                    .flex_none()
                    .px_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme.muted)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // 尾部按钮不触发整行跳转。
                            cx.stop_propagation();
                            if let Some(key) = command_key.clone() {
                                this.toggle_command_visibility(key, cx);
                            } else if let Some(id) = close_target.clone() {
                                this.close_workspace(id, cx);
                            }
                        }),
                    )
                    .child(SharedString::from(label))
                    .into_any_element(),
            );
        }

        div()
            .id(SharedString::from(format!(
                "row-{:?}-{}",
                section, row.title
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(ROW_HEIGHT))
            .px_3()
            .rounded_sm()
            .cursor_pointer()
            .bg(if row.active {
                theme.active_bg
            } else {
                theme.sidebar_bg
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus_handle());
                    this.activate_sidebar_row(target.clone(), cx);
                }),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(marker_color)
                    .child(SharedString::from(row.marker)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_sm()
                            .when(row.emphasized, |element| {
                                element.font_weight(gpui::FontWeight::BOLD)
                            })
                            .text_color(theme.fg)
                            .overflow_hidden()
                            .child(SharedString::from(row.title)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(detail_color)
                            .overflow_hidden()
                            .child(SharedString::from(row.detail)),
                    ),
            )
            .when_some(shortcut, |element, shortcut| {
                element.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(SharedString::from(shortcut)),
                )
            })
            .children(trailing_buttons)
            .into_any_element()
    }
}

/// 指示标记的颜色：done 用成功色、blocked 用危险色、working 用强调色。
fn indicator_color(indicator: Indicator, theme: &Theme) -> gpui::Rgba {
    match indicator {
        Indicator::Done => theme.success,
        Indicator::Blocked => theme.danger,
        Indicator::Working => theme.accent,
        Indicator::Idle => theme.muted,
    }
}
