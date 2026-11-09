//! 设置浮层：左侧分类栏 + 右侧字段列表 + 底部 Cancel / Apply。
//!
//! 分类与字段全部来自 Core manifest（`model/settings.rs` 解析），这里不写死
//! 哪一项存在。布局与 macOS `SettingsWindow.swift` 对齐：680×680、左栏 200pt、
//! 底部 52pt 操作条（摘要 + Cancel + Apply），Esc = 取消、⏎ = 应用。
//!
//! 之前这里没有键盘分支也没有按钮，Esc 关不掉、按键还会漏进终端；现在浮层
//! 独占按键，关闭一定经过「有改动先确认」这一条路。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::model::settings::{self, SettingsCategory, SettingsField};
use crate::frontend::gpui::state::{GpuiApp, SettingsFocus};
use crate::frontend::utils::i18n::{tr, tr_args, TextKey};

/// 与 Swift `NSWindow(contentRect: 680x680)` 一致。
const SETTINGS_WIDTH: f32 = 680.0;
const SETTINGS_HEIGHT: f32 = 680.0;
/// Swift `sidebarView.widthAnchor = 200`。
const RAIL_WIDTH: f32 = 200.0;
/// Swift `footer.heightAnchor = 52`。
const FOOTER_HEIGHT: f32 = 52.0;

impl GpuiApp {
    /// 设置浮层；没打开时返回 `None`。
    pub(crate) fn render_settings(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
        // 窗口比 680×680 小时面板要跟着缩，否则底部 Cancel / Apply 会掉到窗口外，
        // 点不到（无头测试和 600pt 高的小窗口都会命中这个坑）。
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height);
        let panel_width = SETTINGS_WIDTH.min((width - 40.0).max(320.0));
        let panel_height = SETTINGS_HEIGHT.min((height - 40.0).max(240.0));
        let theme = self.theme().clone();
        let categories: Vec<AnyElement> = self
            .settings_categories()
            .into_iter()
            .map(|category| category_row(category, self, &theme, cx))
            .collect();

        let page = match self.settings_selected() {
            Some(category) => category_page(&category, self, &theme, cx),
            None => div()
                .flex_1()
                .p_6()
                .text_sm()
                .text_color(theme.muted)
                .child(SharedString::from(tr(TextKey::SettingsSearchPreferences)))
                .into_any_element(),
        };

        let footer = settings_footer(self, &theme, cx);
        let search = settings_search_box(self, &theme, cx);

        Some(
            div()
                .id("settings-overlay")
                .debug_selector(|| "settings-overlay".into())
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                // 遮罩是面板的兄弟节点而不是父节点：点面板里的按钮不会被
                // 遮罩的「点击取消」抢先处理（之前 Cancel 会因此直接关窗口）。
                .child(
                    div()
                        .id("settings-scrim")
                        .debug_selector(|| "settings-scrim".into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h_full()
                        .bg(gpui::rgba(0x00000099))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                // 点遮罩 = 取消（有改动先确认），与关闭按钮同一条路。
                                this.settings_cancel(cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .id("settings-panel")
                        .debug_selector(|| "settings-panel".into())
                        .flex()
                        .flex_row()
                        .w(px(panel_width))
                        .h(px(panel_height))
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg)
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .w(px(RAIL_WIDTH))
                                .h_full()
                                .py_3()
                                .gap_1()
                                .bg(theme.chrome_bg)
                                .border_r_1()
                                .border_color(theme.border)
                                .child(search)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .px_2()
                                        .py_1()
                                        .flex_1()
                                        .overflow_hidden()
                                        .children(categories),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                // 不加 min_w(0) 的话底部按钮条会按内容撑宽，
                                // 把 Cancel / Apply 挤出面板（点不到）。
                                .min_w(px(0.0))
                                .h_full()
                                .child(page)
                                .child(footer),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// 左栏搜索框（Swift 的 `NSSearchField`）。
fn settings_search_box(
    app: &GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let focused = app.settings_focus() == SettingsFocus::Search;
    let query = app.settings_query().to_string();
    let text = if query.is_empty() && !focused {
        tr(TextKey::SettingsSearchBySettingNameOrKeyword).to_string()
    } else {
        format!("{query}{}", if focused { "▎" } else { "" })
    };
    let color = if query.is_empty() && !focused {
        theme.muted
    } else {
        theme.fg
    };
    div()
        .id("settings-search")
        .debug_selector(|| "settings-search".into())
        .mx_3()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(if focused { theme.accent } else { theme.border })
        .bg(theme.bg)
        .text_xs()
        .text_color(color)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.settings_focus_search(cx);
            }),
        )
        .child(SharedString::from(text))
        .into_any_element()
}

fn category_row(
    category: SettingsCategory,
    app: &GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let active = app.settings_draft.selected() == Some(category.id.as_str());
    let id = category.id.clone();
    div()
        .id(SharedString::from(format!(
            "settings-category-{}",
            category.id
        )))
        .debug_selector({
            let id = category.id.clone();
            move || format!("settings-category-{id}")
        })
        .flex()
        .flex_col()
        .gap_0p5()
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .bg(if active {
            theme.active_bg
        } else {
            theme.chrome_bg
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.settings_select(&id, cx);
                this.settings_focus_field(0, cx);
            }),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.fg)
                .child(SharedString::from(category.title)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .child(SharedString::from(category.hint)),
        )
        .into_any_element()
}

fn category_page(
    category: &SettingsCategory,
    app: &GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let focused_field = match app.settings_focus() {
        SettingsFocus::Field(index) => Some(index),
        SettingsFocus::Search => None,
    };
    let rows: Vec<AnyElement> = category
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| field_row(field, index, focused_field == Some(index), app, theme, cx))
        .collect();
    div()
        .id("settings-page")
        .debug_selector(|| "settings-page".into())
        .flex()
        .flex_col()
        .gap_3()
        .flex_1()
        .min_w(px(0.0))
        .p_6()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .text_color(theme.fg)
                        .child(SharedString::from(category.section.to_string())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(SharedString::from(category.description.to_string())),
                ),
        )
        .children(rows)
        .into_any_element()
}

fn field_row(
    field: &SettingsField,
    index: usize,
    focused: bool,
    app: &GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
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
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(if focused { theme.accent } else { theme.border })
        .bg(theme.chrome_bg)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.settings_focus_field(index, cx);
            }),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.fg)
                                .child(SharedString::from(field.title())),
                        )
                        .when(!field.description().is_empty(), |element| {
                            element.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted)
                                    .child(SharedString::from(field.description())),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(SharedString::from(field.apply.label())),
                )
                .child(control),
        )
        .into_any_element()
}

/// 右侧控件：开关直接翻转，枚举循环，文本显示编辑缓冲。
fn field_control(
    field: &SettingsField,
    index: usize,
    app: &GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    use settings::ControlKind;
    let value = app.settings_value(&field.path);
    let editing = app
        .settings_edit()
        .filter(|edit| edit.path == field.path)
        .map(|edit| edit.buffer.clone());

    // Projects 不再显示原始 JSON：一行一个已保存项目。
    if field.path == "/projects" {
        return projects_control(value.as_ref(), theme);
    }

    let text = match &field.control {
        ControlKind::Switch => {
            let on = value
                .as_ref()
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if on { "On" } else { "Off" }.to_string()
        }
        ControlKind::Select | ControlKind::ThemePicker => {
            let current = value.as_ref().and_then(serde_json::Value::as_str);
            format!("{} ⌄", field.option_text(current))
        }
        _ => editing
            .or_else(|| Some(settings::display_value(value.as_ref())))
            .unwrap_or_default(),
    };
    let clickable = field.control.is_switch() || field.control.is_select();
    let muted = matches!(field.control, ControlKind::Other(_));
    div()
        .id(SharedString::from(format!(
            "settings-control-{}",
            field.path
        )))
        .debug_selector({
            let path = field.path.clone();
            move || format!("settings-control-{path}")
        })
        .flex_none()
        .min_w(px(120.0))
        .px_2()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg)
        .text_sm()
        .text_color(if muted { theme.muted } else { theme.fg })
        .when(clickable, |element| {
            element.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.settings_focus_field(index, cx);
                    this.settings_cycle_field(index, 1, cx);
                }),
            )
        })
        .child(SharedString::from(text))
        .into_any_element()
}

/// `/projects`：列出名字与路径，原始 JSON 留在草稿里不动。
fn projects_control(
    value: Option<&serde_json::Value>,
    theme: &crate::frontend::gpui::theme::Theme,
) -> AnyElement {
    let empty = Vec::new();
    let projects = value
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);
    let mut rows: Vec<AnyElement> = Vec::new();
    for project in projects {
        let name = project
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let path = project
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .text_xs()
                .child(
                    div()
                        .text_color(theme.fg)
                        .child(SharedString::from(name.to_string())),
                )
                .child(
                    div()
                        .text_color(theme.muted)
                        .child(SharedString::from(path.to_string())),
                )
                .into_any_element(),
        );
    }
    if rows.is_empty() {
        rows.push(
            div()
                .text_xs()
                .text_color(theme.muted)
                .child(SharedString::from(tr(
                    TextKey::SettingsSavedWorkspaceProfilesForQuickConnect,
                )))
                .into_any_element(),
        );
    }
    div()
        .id("settings-control-projects")
        .debug_selector(|| "settings-control-projects".into())
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .max_w(px(280.0))
        .overflow_hidden()
        .children(rows)
        .into_any_element()
}

/// 底部操作条：摘要 + Cancel / Apply；有改动要放弃时换成确认。
fn settings_footer(
    app: &GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let dirty = app.settings_dirty();
    let pending = app.settings_discard_pending();
    let projects = app
        .settings_value("/projects")
        .and_then(|value| value.as_array().map(Vec::len))
        .unwrap_or(0);
    let summary = if pending {
        tr(TextKey::SettingsDiscardUnsavedChanges).to_string()
    } else if dirty {
        tr(TextKey::SettingsUnsaved).to_string()
    } else {
        tr_args(
            TextKey::SettingsSyncedWithCore,
            &[("count", &projects.to_string())],
        )
    };
    let summary_color = if dirty || pending {
        theme.warning
    } else {
        theme.muted
    };

    let mut buttons: Vec<AnyElement> = Vec::new();
    if pending {
        buttons.push(button(
            "settings-keep-editing",
            tr(TextKey::SettingsKeepEditing),
            false,
            theme,
            cx,
            |this, cx| this.settings_keep_editing(cx),
        ));
        buttons.push(button(
            "settings-discard",
            tr(TextKey::SettingsDiscard),
            true,
            theme,
            cx,
            |this, cx| this.settings_confirm_discard(cx),
        ));
    } else {
        buttons.push(button(
            "settings-cancel",
            tr(TextKey::SettingsCancel),
            false,
            theme,
            cx,
            |this, cx| this.settings_cancel(cx),
        ));
        buttons.push(button(
            "settings-apply",
            tr(TextKey::SettingsApply),
            true,
            theme,
            cx,
            |this, cx| this.settings_apply_and_close(cx),
        ));
    }

    div()
        .id("settings-footer")
        .debug_selector(|| "settings-footer".into())
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(FOOTER_HEIGHT))
        .w_full()
        .px_6()
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.chrome_bg)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_xs()
                .text_color(summary_color)
                .child(SharedString::from(summary)),
        )
        .children(buttons)
        .into_any_element()
}

fn button(
    id: &'static str,
    label: String,
    primary: bool,
    theme: &crate::frontend::gpui::theme::Theme,
    cx: &mut gpui::Context<GpuiApp>,
    action: fn(&mut GpuiApp, &mut gpui::Context<GpuiApp>),
) -> AnyElement {
    div()
        .id(id)
        .debug_selector({
            let id = id.to_string();
            move || id.clone()
        })
        .flex_none()
        .px_3()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(if primary { theme.accent } else { theme.border })
        .bg(if primary { theme.accent } else { theme.bg })
        .text_sm()
        .text_color(if primary { theme.bg } else { theme.fg })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                action(this, cx);
            }),
        )
        .child(SharedString::from(label))
        .into_any_element()
}
