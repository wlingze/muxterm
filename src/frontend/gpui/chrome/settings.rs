//! 设置窗口：左侧固定宽度分类栏 + 右侧字段列表 + 底部 Apply。
//!
//! 分类与字段全部来自 Core manifest（`model/settings.rs` 解析），这里不写死
//! 哪一项存在。控件按 `ControlKind` 选择：开关画成 On/Off 按钮，其余显示当前值，
//! 数字与文本由键盘改写。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::model::settings::{self, SettingsCategory, SettingsField};
use crate::frontend::gpui::state::GpuiApp;

impl GpuiApp {
    /// 设置窗口浮层；没打开时返回 `None`。
    pub(crate) fn render_settings(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
        let theme = self.theme().clone();
        let categories: Vec<AnyElement> = self
            .settings_categories()
            .into_iter()
            .map(|category| category_row(category, self, &theme, cx))
            .collect();

        let page = match self.settings_selected() {
            Some(category) => category_page(&category, self, &theme),
            None => div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(theme.muted)
                .child(SharedString::from("search for a setting"))
                .into_any_element(),
        };

        let dirty = self.settings_draft.is_dirty();

        Some(
            div()
                .id("settings-overlay")
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h_full()
                .bg(gpui::rgba(0x00000099))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .w(px(960.0))
                        .h(px(620.0))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg)
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .w(px(settings::SIDEBAR_WIDTH))
                                .h_full()
                                .p_2()
                                .bg(theme.chrome_bg)
                                .border_r_1()
                                .border_color(theme.border)
                                .children(categories),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .h_full()
                                .p_4()
                                .gap_3()
                                .child(page)
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .text_xs()
                                        .text_color(theme.muted)
                                        .child(SharedString::from(if dirty {
                                            "unsaved changes · ⏎ apply · esc discard"
                                        } else {
                                            "esc close"
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
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
            cx.listener(move |this, _, _, cx| this.settings_select(&id, cx)),
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
) -> AnyElement {
    let rows: Vec<AnyElement> = category
        .fields
        .iter()
        .map(|field| {
            let value = app.settings_draft.get(&field.path);
            field_row(field, value, theme)
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .flex_1()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_base()
                        .child(SharedString::from(category.section.to_string())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(SharedString::from(category.description.to_string())),
                ),
        )
        .children(rows)
        .into_any_element()
}

fn field_row(
    field: &SettingsField,
    value: Option<&serde_json::Value>,
    theme: &crate::frontend::gpui::theme::Theme,
) -> AnyElement {
    let control = match field.control {
        settings::ControlKind::Switch => {
            let on = value.and_then(serde_json::Value::as_bool).unwrap_or(false);
            if on { "On" } else { "Off" }.to_string()
        }
        settings::ControlKind::ThemePicker => value
            .and_then(serde_json::Value::as_str)
            .map(|value| settings::option_label(&field.path, value))
            .unwrap_or_default(),
        _ => value.map(display_value).unwrap_or_else(|| "—".to_string()),
    };
    div()
        .id(SharedString::from(format!("settings-field-{}", field.path)))
        .flex()
        .flex_col()
        .gap_0p5()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(theme.chrome_bg)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .child(SharedString::from(field.title())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(SharedString::from(field.apply.label())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.fg)
                        .child(SharedString::from(control)),
                ),
        )
        .when(!field.description().is_empty(), |element| {
            element.child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(SharedString::from(field.description())),
            )
        })
        .into_any_element()
}

/// 字段当前值的一行文本；数组与对象按 JSON 展开，避免吞掉内容。
fn display_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Null => "—".to_string(),
        other => other.to_string(),
    }
}
