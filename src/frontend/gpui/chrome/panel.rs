//! Swift UnifiedPanel 的 640×420 快速面板：真实列表滚动 + Open 分步导航 + 右下角 Cmd-K。
use crate::frontend::gpui::chrome::panel_view::{AggregateKind, PanelRow, PickerAction};
use crate::frontend::gpui::model::panel::PanelTab;
use crate::frontend::gpui::model::target::TargetRuntime;
use crate::frontend::gpui::model::workspace_picker::PickerStage;
use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;
use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

pub const PANEL_OPEN_LABEL: &str = "+ Open";
pub const PANEL_WIDTH: f32 = 640.0;
pub const PANEL_HEIGHT: f32 = 420.0;

impl GpuiApp {
    pub(crate) fn render_panel(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.panel_open() {
            return None;
        }
        let theme = self.theme().clone();
        let query = if let Some(menu) = self.panel_actions() {
            menu.query.clone()
        } else {
            self.panel_query()
        };
        let tabs = PanelTab::ALL
            .iter()
            .map(|tab| {
                let tab = *tab;
                let selected = tab == self.panel_tab();
                div()
                    .id(SharedString::from(format!("panel-tab-{tab:?}")))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(if selected {
                        theme.active_bg
                    } else {
                        theme.chrome_bg
                    })
                    .text_color(if selected { theme.label() } else { theme.muted })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app, _, _, cx| app.show_panel(tab, cx)),
                    )
                    .child(panel_tab_title(tab))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let mut rows = self
            .panel_rows()
            .into_iter()
            .enumerate()
            .map(|(index, row)| render_row(row, index, index == self.panel_selected(), &theme, cx))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            rows.push(
                div()
                    .p_3()
                    .text_color(theme.muted)
                    .child("No results")
                    .into_any_element(),
            );
        }
        let draft = match self.panel_stage() {
            PickerStage::Confirm(draft) => Some(draft),
            _ => None,
        };
        let runtime_controls = draft.map(|draft| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(14.))
                .py(px(8.))
                .child(
                    div()
                        .text_color(theme.muted)
                        .child(SharedString::from(format!(
                            "{} · {}",
                            draft.transport.label(),
                            draft.path
                        ))),
                )
                .child(
                    div().flex().gap(px(8.)).children(
                        [TargetRuntime::Herdr, TargetRuntime::Tmux]
                            .into_iter()
                            .map(|runtime| {
                                div()
                                    .id(SharedString::from(format!("panel-runtime-{runtime:?}")))
                                    .debug_selector(|| format!("panel-runtime-{runtime:?}"))
                                    .px(px(12.))
                                    .py(px(4.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .bg(if runtime == draft.runtime {
                                        theme.accent
                                    } else {
                                        theme.active_bg
                                    })
                                    .text_color(if runtime == draft.runtime {
                                        gpui::rgb(0xffffff)
                                    } else {
                                        theme.label()
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |app, _, _, cx| {
                                            app.choose_panel_runtime(runtime, cx)
                                        }),
                                    )
                                    .child(runtime.label())
                                    .into_any_element()
                            }),
                    ),
                )
                .into_any_element()
        });
        let search = div()
            .id("panel-query")
            .debug_selector(|| "panel-query".into())
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_none()
            .h(px(28.))
            .mx(px(14.))
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.active_bg)
            .child(div().text_color(theme.muted).child("⌕"))
            // 查询框：选区高亮 + 可见光标（`Cmd-A` 能看见结果）。
            .child(crate::frontend::gpui::chrome::input_line::input_line(
                &self.panel.model.input,
                self.panel_placeholder(),
                &theme,
            ));
        let action_menu = self.panel_actions().map(|menu| {
            div()
                .id("panel-actions")
                .debug_selector(|| "panel-actions".into())
                .absolute()
                .right(px(8.))
                .bottom(px(8.))
                .w(px(260.))
                .max_h(px(280.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(8.))
                .rounded(px(6.))
                .shadow_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.chrome_bg)
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .child(SharedString::from(menu.target.title())),
                )
                .child(
                    div()
                        .py(px(5.))
                        .text_color(if query.is_empty() {
                            theme.muted
                        } else {
                            theme.label()
                        })
                        .child(SharedString::from(if query.is_empty() {
                            self.panel_placeholder().into()
                        } else {
                            query
                        })),
                )
                .children(menu.rows().into_iter().enumerate().map(|(index, action)| {
                    div()
                        .id(SharedString::from(format!("panel-action-{index}")))
                        .debug_selector(|| format!("panel-action-{index}"))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .bg(if index == menu.selected {
                            theme.active_bg
                        } else {
                            theme.chrome_bg
                        })
                        .hover(|style| style.bg(theme.active_bg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |app, _, _, cx| app.select_panel_action(index, cx)),
                        )
                        .child(action.title())
                        .into_any_element()
                }))
                .into_any_element()
        });
        Some(
            div()
                .id("panel-overlay")
                .absolute()
                .inset_0()
                .size_full()
                .bg(gpui::rgba(0x00000033))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|app, _, _, cx| app.close_panel(cx)),
                )
                .child(
                    div()
                        .id("quick-panel")
                        .debug_selector(|| "quick-panel".into())
                        .relative()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .w(px(PANEL_WIDTH))
                        .h(px(PANEL_HEIGHT))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.chrome_bg)
                        .shadow_lg()
                        .text_size(px(13.))
                        .line_height(px(16.))
                        .text_color(theme.label())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .h(px(28.))
                                .items_center()
                                .justify_between()
                                .px(px(14.))
                                .pt(px(6.))
                                .child(div().flex().children(tabs))
                                .child(
                                    div()
                                        .id("panel-cmd-k")
                                        .debug_selector(|| "panel-cmd-k".into())
                                        .px(px(8.))
                                        .rounded(px(4.))
                                        .bg(theme.active_bg)
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|app, _, _, cx| {
                                                app.toggle_panel_actions(cx)
                                            }),
                                        )
                                        .child("⌘K"),
                                ),
                        )
                        .child(search)
                        .children(runtime_controls)
                        .child(
                            div()
                                .id("panel-list")
                                .debug_selector(|| "panel-list".into())
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .mx(px(7.))
                                .mb(px(5.))
                                .overflow_y_scroll()
                                .track_scroll(self.panel_scroll_handle())
                                .children(rows),
                        )
                        .when(self.panel_operation_pending(), |element| {
                            element.child(
                                div()
                                    .px(px(14.))
                                    .pb(px(6.))
                                    .text_color(theme.muted)
                                    .child("Working…"),
                            )
                        })
                        .children(action_menu),
                )
                .into_any_element(),
        )
    }
}

fn render_row(
    row: PanelRow,
    index: usize,
    selected: bool,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let title = row.title();
    let detail = row.detail();
    // 状态色一律走主题 token：Attention 行必须能一眼看出 working / done / blocked，
    // 聚合槽用身份色（Shells 青 / Agents 紫），不借用状态色。
    // 标记列也走矢量图标：`◆` / `●` 在不同字体里粗细不一，看起来像贴纸。
    let (icon, symbol, color) = match &row {
        PanelRow::Aggregate(AggregateKind::Shells) => (
            Some(crate::frontend::gpui::chrome::icons::Icon::Diamond),
            String::new(),
            crate::frontend::gpui::chrome::sidebar::identity_color(
                crate::frontend::gpui::model::aggregate::AggregateIdentity::Shells,
                theme,
            ),
        ),
        PanelRow::Aggregate(AggregateKind::Agents) => (
            Some(crate::frontend::gpui::chrome::icons::Icon::Diamond),
            String::new(),
            crate::frontend::gpui::chrome::sidebar::identity_color(
                crate::frontend::gpui::model::aggregate::AggregateIdentity::Agents,
                theme,
            ),
        ),
        PanelRow::Action(PickerAction::Open) => (
            Some(crate::frontend::gpui::chrome::icons::Icon::Plus),
            String::new(),
            theme.accent,
        ),
        PanelRow::Workspace { shortcut, .. } => (
            None,
            shortcut.map(|n| n.to_string()).unwrap_or_default(),
            theme.muted,
        ),
        PanelRow::Attention { status, .. } => (
            Some(crate::frontend::gpui::chrome::icons::Icon::CircleFilled),
            String::new(),
            attention_color(*status, theme),
        ),
        _ => (None, String::new(), theme.muted),
    };
    let is_project = matches!(
        &row,
        PanelRow::Target { .. } | PanelRow::Existing { saved: true, .. }
    );
    let can_save = matches!(&row, PanelRow::Existing { saved: false, .. });
    let attention_label = match &row {
        PanelRow::Attention { status, .. } => Some((status.as_str().to_string(), color)),
        _ => None,
    };
    let closable = match &row {
        PanelRow::Workspace { workspace_id, .. } => Some(workspace_id.clone()),
        _ => None,
    };
    let row_background = if selected {
        theme.active_bg
    } else {
        theme.chrome_bg
    };
    div()
        .id(SharedString::from(format!("panel-row-{index}")))
        .debug_selector(|| format!("panel-row-{index}"))
        .flex()
        .items_center()
        .flex_none()
        .min_h(px(if detail.is_some() { 46. } else { 29. }))
        .px(px(12.))
        .py(px(5.))
        .gap(px(8.))
        .cursor_pointer()
        .bg(row_background)
        .hover(|style| style.bg(theme.active_bg))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| app.select_panel_row(index, cx)),
        )
        .child(
            div()
                .w(px(14.))
                .h(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .text_color(color)
                .when_some(icon, |element, icon| {
                    element.child(crate::frontend::gpui::chrome::icons::icon_element(
                        crate::frontend::gpui::chrome::icons::IconSpec::with_size(
                            icon, color, 10.0,
                        ),
                        14.0,
                    ))
                })
                .when(!symbol.is_empty(), |element| {
                    element.child(SharedString::from(symbol))
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .overflow_hidden()
                                .text_color(
                                    if matches!(
                                        row,
                                        PanelRow::Aggregate(_)
                                            | PanelRow::Action(PickerAction::Open)
                                    ) {
                                        color
                                    } else {
                                        theme.label()
                                    },
                                )
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(SharedString::from(title)),
                        )
                        .when(is_project, |element| {
                            element.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.success)
                                    .child("● Project"),
                            )
                        }),
                )
                .when_some(detail, |element, detail| {
                    element.child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(14.))
                            .text_color(theme.muted)
                            .overflow_hidden()
                            .child(SharedString::from(detail)),
                    )
                }),
        )
        .when_some(attention_label, |element, (label, color)| {
            element.child(
                div()
                    .id(SharedString::from(format!("panel-status-{index}")))
                    .debug_selector(|| format!("panel-status-{index}"))
                    .flex_none()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(color)
                    .bg(gpui::Rgba { a: 0.12, ..color })
                    .child(SharedString::from(label)),
            )
        })
        .when(can_save, |element| {
            element.child(
                div()
                    .id(SharedString::from(format!("panel-save-{index}")))
                    .debug_selector(|| format!("panel-save-{index}"))
                    .flex_none()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(theme.active_bg)
                    .child("Save")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.save_panel_existing(index, cx);
                        }),
                    ),
            )
        })
        .when_some(closable, |element, workspace| {
            element.child(
                div()
                    .id(SharedString::from(format!("panel-close-{index}")))
                    .text_color(theme.muted)
                    .child("×")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.close_workspace(workspace.clone(), cx);
                        }),
                    ),
            )
        })
        .into_any_element()
}
/// Attention 行的状态色，与侧栏 `indicatorColor` 同一套 token。
fn attention_color(
    status: crate::frontend::gpui::model::attention::PaneAttentionStatus,
    theme: &Theme,
) -> gpui::Rgba {
    use crate::frontend::gpui::model::attention::PaneAttentionStatus as Status;
    match status {
        Status::Working => theme.working,
        Status::Done => theme.done,
        Status::Blocked => theme.blocked,
        Status::Idle | Status::Unknown => theme.muted,
    }
}

fn panel_tab_title(tab: PanelTab) -> &'static str {
    match tab {
        PanelTab::Workspaces => "Workspaces",
        PanelTab::Attention => "Attention",
        PanelTab::Search => "Search",
    }
}
