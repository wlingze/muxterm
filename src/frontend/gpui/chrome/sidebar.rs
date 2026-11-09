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
/// 侧栏宽度：与 macOS `mainSplitController.setPosition(240)` 一致。
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// 段头高度：Swift 的 `NSButton` 段头 + 上下留白。
const SECTION_HEIGHT: f32 = 22.0;
/// 段头左右内边距。
const SECTION_INSET: f32 = 12.0;
/// 行左右内边距（Swift cell 的 leading/trailing）。
const ROW_INSET: f32 = 8.0;

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
            .debug_selector(|| "sidebar".to_string())
            .h_full()
            .overflow_hidden()
            .py(px(4.0))
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
        let header_title = section_header_title(&section);
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
            .child(
                div()
                    .id(SharedString::from(format!("section-{:?}", kind)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(SECTION_HEIGHT))
                    .px(px(SECTION_INSET))
                    .cursor_pointer()
                    .text_size(px(crate::frontend::gpui::theme::SIDEBAR_SECTION_FONT_SIZE))
                    .line_height(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_sidebar_section(kind, cx)),
                    )
                    .child(SharedString::from(header_title))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .when(kind == Section::Workspaces, |element| {
                                // 对照 `WorkspaceSidebar` 的 `addWorkspaceButton`：新建 project。
                                element.child(
                                    div()
                                        .id("sidebar-add-workspace")
                                        .px(px(4.0))
                                        .cursor_pointer()
                                        .text_color(theme.muted)
                                        .hover(|style| style.text_color(theme.label()))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.open_target_editor(None, cx);
                                            }),
                                        )
                                        .child(SharedString::from("+")),
                                )
                            })
                            .child(SharedString::from(if section.expanded {
                                "▾"
                            } else {
                                "▸"
                            })),
                    ),
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
                    .px(px(4.0))
                    .cursor_pointer()
                    .text_size(px(crate::frontend::gpui::theme::SIDEBAR_DETAIL_FONT_SIZE))
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
            .gap(px(6.0))
            .h(px(ROW_HEIGHT))
            .px(px(ROW_INSET))
            .rounded(px(4.0))
            .cursor_pointer()
            .bg(if row.active {
                theme.active_bg
            } else {
                theme.sidebar_bg
            })
            .hover(|style| style.bg(theme.active_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus_handle());
                    this.activate_sidebar_row(target.clone(), cx);
                }),
            )
            .child(
                // 标记列固定宽度，行与行之间才对得齐（Swift 用 9.5pt 居中标签）。
                div()
                    .flex_none()
                    .w(px(12.0))
                    .text_size(px(crate::frontend::gpui::theme::SIDEBAR_SHORTCUT_FONT_SIZE))
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
                            .text_size(px(crate::frontend::gpui::theme::SIDEBAR_TITLE_FONT_SIZE))
                            .line_height(px(15.0))
                            .when(row.emphasized, |element| {
                                element.font_weight(gpui::FontWeight::SEMIBOLD)
                            })
                            .text_color(theme.label())
                            .overflow_hidden()
                            .child(SharedString::from(row.title)),
                    )
                    .child(
                        div()
                            .text_size(px(crate::frontend::gpui::theme::SIDEBAR_DETAIL_FONT_SIZE))
                            .line_height(px(12.0))
                            .text_color(detail_color)
                            .overflow_hidden()
                            .child(SharedString::from(row.detail)),
                    ),
            )
            .when_some(shortcut, |element, shortcut| {
                element.child(
                    div()
                        .flex_none()
                        .text_size(px(crate::frontend::gpui::theme::SIDEBAR_SHORTCUT_FONT_SIZE))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(SharedString::from(shortcut)),
                )
            })
            .children(trailing_buttons)
            .into_any_element()
    }
}

/// 段头文案：`WORKSPACES 6`。
///
/// 对照 macOS `WorkspaceSidebar` 的段头：有行时才带计数，空段只留标题。
fn section_header_title(section: &SidebarSectionModel) -> String {
    if section.rows.is_empty() {
        section.title.clone()
    } else {
        format!("{}  {}", section.title, section.rows.len())
    }
}

/// 指示标记的颜色：done 用成功色、blocked 用危险色、working 用强调色。
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
