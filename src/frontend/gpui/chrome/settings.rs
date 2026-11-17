//! Swift 风格设置：固定图标侧栏、可滚动分组表单、全宽管理页和固定操作栏。
//! 生产环境位于独立 SettingsWindow；无头旧测试复用同一内容构建器。
use super::icons::{self, Icon, IconSpec};
use crate::frontend::gpui::{
    model::settings::{self, SettingsCategory, SettingsField},
    state::{GpuiApp, SettingsFocus},
    theme::Theme,
};
use crate::frontend::utils::i18n::{tr, tr_args, TextKey};
use gpui::{div, prelude::*, px, AnyElement, FontWeight, MouseButton, SharedString};

pub const SETTINGS_WIDTH: f32 = 840.0;
pub const SETTINGS_HEIGHT: f32 = 680.0;
pub const RAIL_WIDTH: f32 = 200.0;
const FOOTER_HEIGHT: f32 = 52.0;

impl GpuiApp {
    /// 无头渲染兼容入口；实际 app 不再在终端上叠一层设置。
    pub(crate) fn render_settings(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        if !self.settings_open || self.settings_detached {
            return None;
        }
        let viewport = window.viewport_size();
        Some(
            div()
                .id("settings-overlay")
                .debug_selector(|| "settings-overlay".into())
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("settings-scrim")
                        .debug_selector(|| "settings-scrim".into())
                        .absolute()
                        .inset_0()
                        .bg(gpui::rgba(0x00000055))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.settings_cancel(cx)),
                        ),
                )
                .child(
                    div()
                        .id("settings-panel")
                        .debug_selector(|| "settings-panel".into())
                        .w(px(
                            SETTINGS_WIDTH.min((f32::from(viewport.width) - 32.0).max(320.0))
                        ))
                        .h(px(
                            SETTINGS_HEIGHT.min((f32::from(viewport.height) - 32.0).max(240.0))
                        ))
                        .rounded_lg()
                        .overflow_hidden()
                        .shadow_lg()
                        .child(self.render_settings_content(window, cx)),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_settings_content(
        &self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let mut theme = self.theme().clone();
        // 设置的中性 chrome 不借用终端背景/字体，保持 Swift controlBackground 观感。
        if !theme.is_dark {
            theme.chrome_bg = gpui::rgb(0xf4f4f4);
            theme.sidebar_bg = gpui::rgb(0xf8f8f8);
            theme.bg = gpui::rgb(0xffffff);
            theme.fg = gpui::rgb(0x262626);
            theme.muted = gpui::rgb(0x808086);
            theme.border = gpui::rgb(0xdadadd);
            theme.active_bg = gpui::rgb(0xe7e7ea);
        }
        let categories = self
            .settings_categories()
            .into_iter()
            .map(|category| category_row(category, self, &theme, cx))
            .collect::<Vec<_>>();
        let page = self
            .settings_selected()
            .map(|category| category_page(&category, self, &theme, cx))
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .p_6()
                    .text_color(theme.muted)
                    .child(tr(TextKey::SettingsSearchPreferences))
                    .into_any_element()
            });
        div()
            .id("settings-surface")
            .size_full()
            .relative()
            .flex()
            .text_size(px(13.0))
            .line_height(px(18.0))
            .text_color(theme.fg)
            .bg(theme.chrome_bg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(RAIL_WIDTH))
                    .h_full()
                    .bg(theme.sidebar_bg)
                    .border_r_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .pt(px(52.0))
                            .pb(px(12.0))
                            .child(settings_search_box(self, &theme, cx)),
                    )
                    .child(
                        div()
                            .id("settings-categories")
                            .debug_selector(|| "settings-categories".into())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(px(8.0))
                            .pb(px(16.0))
                            .children(categories),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(page)
                    .child(settings_footer(self, &theme, cx)),
            )
            .when(self.settings_project_editor, |element| {
                element.children(self.render_target_editor(cx))
            })
            .into_any_element()
    }
}

fn settings_search_box(
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let focused = app.settings_focus() == SettingsFocus::Search;
    let query = app.settings_query();
    div()
        .id("settings-search")
        .debug_selector(|| "settings-search".into())
        .mx(px(12.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(7.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(if focused { theme.accent } else { theme.border })
        .bg(theme.bg)
        .child(icons::icon_element(
            IconSpec::with_size(Icon::Search, theme.muted, 14.0),
            16.0,
        ))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_size(px(12.0))
                .text_color(if query.is_empty() {
                    theme.muted
                } else {
                    theme.fg
                })
                .child(SharedString::from(if query.is_empty() {
                    tr(TextKey::SettingsSearchPreferences)
                } else {
                    query.to_string()
                })),
        )
        .cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|app, _, _, cx| {
                cx.stop_propagation();
                app.settings_focus_search(cx);
            }),
        )
        .into_any_element()
}

fn category_row(
    category: SettingsCategory,
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let selected = app.settings_draft.selected() == Some(&category.id);
    let id = category.id.clone();
    let icon = match id.as_str() {
        "appearance" => Icon::TextFormat,
        "runtime" => Icon::Terminal,
        "projects" => Icon::Folder,
        "shortcuts" => Icon::Keyboard,
        "attention" => Icon::Bell,
        "quick_panel" => Icon::Clock,
        "ui" | "platform" => Icon::Sidebar,
        "ssh" => Icon::Network,
        "update" => Icon::Download,
        _ => Icon::Gear,
    };
    div()
        .id(SharedString::from(format!("settings-category-{id}")))
        .debug_selector({
            let id = id.clone();
            move || format!("settings-category-{id}")
        })
        .flex()
        .items_center()
        .gap(px(10.0))
        .h(px(34.0))
        .px(px(9.0))
        .my(px(1.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .bg(if selected {
            theme.active_bg
        } else {
            theme.sidebar_bg
        })
        .hover(|s| s.bg(theme.active_bg))
        .child(icons::icon_element(
            IconSpec::with_size(
                icon,
                if selected { theme.accent } else { theme.muted },
                17.0,
            ),
            20.0,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.0))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(theme.fg)
                .child(category.title),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.settings_select(&id, cx);
                app.settings_focus_field(0, cx);
            }),
        )
        .into_any_element()
}

fn category_page(
    category: &SettingsCategory,
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let focused = match app.settings_focus() {
        SettingsFocus::Field(index) => Some(index),
        _ => None,
    };
    let mut rows = Vec::new();
    for (index, field) in category.fields.iter().enumerate() {
        rows.push(match &field.control {
            settings::ControlKind::ProjectEditor => {
                super::settings_managers::projects(app, theme, cx)
            }
            settings::ControlKind::ShortcutEditor => {
                super::settings_managers::shortcuts(app, theme, cx)
            }
            _ => field_row(field, index, focused == Some(index), app, theme, cx),
        });
    }
    div()
        .id("settings-page")
        .debug_selector(|| "settings-page".into())
        .flex_1()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .px(px(24.0))
        .pt(px(22.0))
        .pb(px(16.0))
        .gap(px(16.0))
        .child(
            div()
                .flex_none()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(category.section.to_uppercase()),
        )
        .child(
            div()
                .id(SharedString::from(format!(
                    "settings-scroll-{}",
                    category.id
                )))
                .debug_selector(|| "settings-page-scroll".into())
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg)
                        .overflow_hidden()
                        .children(rows),
                ),
        )
        .into_any_element()
}

fn field_row(
    field: &SettingsField,
    index: usize,
    focused: bool,
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let control = field_control(field, index, app, theme, cx);
    div()
        .id(SharedString::from(format!("settings-field-{}", field.path)))
        .debug_selector({
            let path = field.path.clone();
            move || format!("settings-field-{path}")
        })
        .flex()
        .items_center()
        .gap(px(16.0))
        .px(px(18.0))
        .py(px(16.0))
        .min_h(px(64.0))
        .border_b_1()
        .border_color(theme.border)
        .when(focused, |s| {
            s.bg(gpui::Rgba {
                a: 0.035,
                ..theme.accent
            })
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.settings_focus_field(index, cx);
            }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(field.title()),
                )
                .when(!field.description().is_empty(), |s| {
                    s.child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(16.0))
                            .text_color(theme.muted)
                            .child(field.description()),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .w(px(170.0))
                .flex()
                .justify_end()
                .child(control),
        )
        .into_any_element()
}

fn field_control(
    field: &SettingsField,
    index: usize,
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    use settings::ControlKind;
    let value = app.settings_value(&field.path);
    let editing = app
        .settings_edit()
        .filter(|edit| edit.path == field.path)
        .map(|edit| edit.buffer.clone());
    let clickable = field.control.is_switch() || field.control.is_select();
    let base = div()
        .id(SharedString::from(format!(
            "settings-control-{}",
            field.path
        )))
        .debug_selector({
            let path = field.path.clone();
            move || format!("settings-control-{path}")
        })
        .flex_none()
        .when(clickable, |s| {
            s.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.settings_cycle_field(index, 1, cx);
                }),
            )
        });
    match field.control {
        ControlKind::Switch => {
            let on = value
                .as_ref()
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            base.w(px(34.0))
                .h(px(20.0))
                .p(px(2.0))
                .flex()
                .items_center()
                .when(on, |s| s.justify_end())
                .rounded_full()
                .bg(if on { theme.accent } else { theme.active_bg })
                .child(
                    div()
                        .size(px(16.0))
                        .rounded_full()
                        .bg(gpui::rgb(0xffffff))
                        .shadow_sm(),
                )
                .into_any_element()
        }
        ControlKind::Select | ControlKind::ThemePicker => base
            .w(px(170.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px(px(8.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg)
            .shadow_sm()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .child(field.option_text(value.as_ref().and_then(serde_json::Value::as_str))),
            )
            .child(icons::chevron_icon(true, theme.muted))
            .into_any_element(),
        _ => base
            .w(px(170.0))
            .min_h(px(28.0))
            .flex()
            .items_center()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(if editing.is_some() {
                theme.accent
            } else {
                theme.border
            })
            .bg(theme.bg)
            .text_size(px(12.0))
            .overflow_hidden()
            .child(SharedString::from(
                editing.unwrap_or_else(|| settings::display_value(value.as_ref())),
            ))
            .into_any_element(),
    }
}

fn settings_footer(app: &GpuiApp, theme: &Theme, cx: &mut gpui::Context<GpuiApp>) -> AnyElement {
    let pending = app.settings_discard_pending();
    let count = app
        .settings_value("/projects")
        .and_then(|value| value.as_array().map(Vec::len))
        .unwrap_or(0);
    let summary = app.settings_error.clone().unwrap_or_else(|| {
        if pending {
            tr(TextKey::SettingsDiscardUnsavedChanges)
        } else if app.settings_dirty() {
            tr(TextKey::SettingsUnsaved)
        } else {
            tr_args(
                TextKey::SettingsSyncedWithCore,
                &[("count", &count.to_string())],
            )
        }
    });
    let mut buttons = Vec::new();
    if pending {
        buttons.push(button(
            "settings-keep-editing",
            tr(TextKey::SettingsKeepEditing),
            false,
            theme,
            cx,
            |app, cx| app.settings_keep_editing(cx),
        ));
        buttons.push(button(
            "settings-discard",
            tr(TextKey::SettingsDiscard),
            true,
            theme,
            cx,
            |app, cx| app.settings_confirm_discard(cx),
        ));
    } else {
        buttons.push(button(
            "settings-cancel",
            tr(TextKey::SettingsCancel),
            false,
            theme,
            cx,
            |app, cx| app.settings_cancel(cx),
        ));
        buttons.push(button(
            "settings-apply",
            tr(TextKey::SettingsApply),
            true,
            theme,
            cx,
            |app, cx| app.settings_apply_and_close(cx),
        ));
    }
    div()
        .id("settings-footer")
        .debug_selector(|| "settings-footer".into())
        .flex_none()
        .h(px(FOOTER_HEIGHT))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .px(px(24.0))
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.sidebar_bg)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_size(px(11.0))
                .text_color(if app.settings_error.is_some() {
                    theme.danger
                } else {
                    theme.muted
                })
                .child(summary),
        )
        .child(div().flex_none().flex().gap(px(8.0)).children(buttons))
        .into_any_element()
}

pub(super) fn button(
    id: &'static str,
    label: String,
    primary: bool,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
    action: impl Fn(&mut GpuiApp, &mut gpui::Context<GpuiApp>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex_none()
        .h(px(28.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(if primary { theme.accent } else { theme.border })
        .bg(if primary { theme.accent } else { theme.bg })
        .shadow_sm()
        .text_size(px(13.0))
        .text_color(if primary {
            gpui::rgb(0xffffff)
        } else {
            theme.fg
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                action(app, cx);
            }),
        )
        .child(label)
        .into_any_element()
}
