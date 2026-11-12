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
        .h(px(28.0))
        .flex()
        .flex_row()
        .items_center()
        .gap_1p5()
        .px_2()
        .rounded(px(6.0))
        .border_1()
        .border_color(if focused { theme.accent } else { theme.border })
        .bg(theme.bg)
        .text_size(px(11.5))
        .text_color(color)
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.muted)
                .child("⌕"),
        )
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
        .py(px(7.0))
        .rounded(px(6.0))
        .cursor_pointer()
        // 选中项用强调色淡底 + 强调色标题（Swift 的 source list 选中态）。
        .bg(if active {
            gpui::Rgba {
                a: 0.14,
                ..theme.accent
            }
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
                .text_size(px(13.0))
                .text_color(if active { theme.accent } else { theme.label() })
                .font_weight(if active {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .child(SharedString::from(category.title)),
        )
        .child(
            div()
                .text_size(px(11.0))
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
    // 对照 Swift：一页 = 标题 + 说明 + 一整张分组的卡片（行之间用分隔线，不再
    // 每行套一个独立边框盒子）。卡片底是 controlBackgroundColor 的白。
    div()
        .id("settings-page")
        .debug_selector(|| "settings-page".into())
        .flex()
        .flex_col()
        .gap(px(14.0))
        .flex_1()
        .min_w(px(0.0))
        .px(px(16.0))
        .py(px(18.0))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.label())
                        .child(SharedString::from(category.section.to_string())),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.muted)
                        .child(SharedString::from(category.description.to_string())),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar_bg)
                .overflow_hidden()
                .children(rows),
        )
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
    // 对照 `SettingsWindow.makeRow`：左侧文案 18pt 起点、右侧控件固定 200pt、
    // 控件右对齐到 18pt 内边距。之前左侧没有 `min_w(0)`，长描述把控件挤出
    // 卡片外，Apply 状态文字和下拉框整个跑到面板外面。
    div()
        .id(SharedString::from(format!("settings-field-{}", field.path)))
        .debug_selector({
            let path = field.path.clone();
            move || format!("settings-field-{path}")
        })
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(16.0))
        .py(px(12.0))
        .border_b_1()
        .border_color(theme.border)
        .when(focused, |element| {
            element.bg(gpui::Rgba {
                a: 0.06,
                ..theme.accent
            })
        })
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
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.label())
                        .child(SharedString::from(field.title())),
                )
                .when(!field.description().is_empty(), |element| {
                    element.child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .text_color(theme.muted)
                            .child(SharedString::from(field.description())),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .flex_none()
                .w(px(180.0))
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
        .when(clickable, |element| {
            element.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.settings_focus_field(index, cx);
                    this.settings_cycle_field(index, 1, cx);
                }),
            )
        });

    match &field.control {
        // 开关 = 圆角胶囊 + 滑块（Swift 是 `NSSwitch`）；纯文字 On/Off
        // 既看不出当前状态，也没法一眼判断能不能点。
        ControlKind::Switch => {
            let on = value
                .as_ref()
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            base.w(px(38.0))
                .h(px(22.0))
                .p(px(2.0))
                .flex()
                .items_center()
                .when(on, |element| element.justify_end())
                .rounded_full()
                .bg(if on { theme.accent } else { theme.active_bg })
                .border_1()
                .border_color(if on { theme.accent } else { theme.border })
                .child(
                    div()
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_full()
                        .bg(if on { theme.bg } else { theme.muted })
                        .shadow_sm(),
                )
                .into_any_element()
        }
        // 下拉 = 控件样式的按钮：左边当前值，右边 chevron（Swift `NSPopUpButton`）。
        ControlKind::Select | ControlKind::ThemePicker => {
            let current = value.as_ref().and_then(serde_json::Value::as_str);
            base.w(px(180.0))
                .h(px(26.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .px(px(8.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.bg)
                .text_size(px(12.0))
                .text_color(theme.label())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(SharedString::from(field.option_text(current))),
                )
                .child(crate::frontend::gpui::chrome::icons::chevron_icon(
                    true,
                    theme.muted,
                ))
                .into_any_element()
        }
        // 其余（文本 / 数字 / 文件 / 多行）= 输入框外观，正在编辑时显示缓冲。
        _ => {
            let is_editing = editing.is_some();
            let text = editing
                .or_else(|| Some(settings::display_value(value.as_ref())))
                .unwrap_or_default();
            let muted = matches!(field.control, ControlKind::Other(_));
            base.w(px(180.0))
                .min_h(px(26.0))
                .flex()
                .items_center()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(if is_editing {
                    theme.accent
                } else {
                    theme.border
                })
                .bg(theme.bg)
                .text_size(px(12.0))
                .text_color(if muted { theme.muted } else { theme.label() })
                .overflow_hidden()
                .child(SharedString::from(text))
                .into_any_element()
        }
    }
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
