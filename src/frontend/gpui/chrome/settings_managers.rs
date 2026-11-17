//! 全宽 Projects / Shortcuts 管理器。不能塞进普通表单的固定宽度控件列。
use super::settings::button;
use crate::frontend::gpui::{
    model::{settings, shortcut_editor, workspace_picker::ProjectList},
    state::GpuiApp,
    theme::Theme,
};
use gpui::{div, prelude::*, px, AnyElement, FontWeight, MouseButton, SharedString};

pub(super) fn projects(
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let projects = ProjectList::new(
        &app.settings_value("/projects")
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .configs();
    let rows = projects
        .into_iter()
        .enumerate()
        .map(|(index, config)| {
            let edit = config.clone();
            let delete = config.clone();
            div()
                .id(SharedString::from(format!("settings-project-{index}")))
                .debug_selector(move || format!("settings-project-{index}"))
                .flex()
                .items_center()
                .gap(px(12.0))
                .min_h(px(58.0))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(9.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar_bg)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.settings_edit_project(Some(&edit), cx);
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
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(config.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme.muted)
                                .overflow_hidden()
                                .child(format!(
                                    "{} · {} · {}",
                                    config.runtime.label().to_lowercase(),
                                    config.transport.label(),
                                    config.path
                                )),
                        ),
                )
                .child(row_button(
                    format!("settings-project-delete-{index}"),
                    settings::localized("settings_project_delete", "Delete"),
                    theme,
                    cx,
                    move |app, cx| app.settings_delete_project(&delete, cx),
                ))
                .into_any_element()
        })
        .collect::<Vec<_>>();
    div().id("settings-control-projects").debug_selector(||"settings-control-projects".into()).w_full().flex().flex_col().gap(px(8.0)).p(px(18.0))
        .child(div().flex().items_center().justify_between().gap(px(12.0)).mb(px(6.0))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(5.0)).child(div().font_weight(FontWeight::SEMIBOLD).child(settings::localized("settings_projects","Projects")))
                .child(div().text_size(px(11.5)).line_height(px(16.0)).text_color(theme.muted).child(settings::localized("settings_project_hint","Reusable launch profiles shared by Open. Click a row to edit; Apply saves your changes."))))
            .child(button("settings-project-new",settings::localized("settings_new_project","New project…"),false,theme,cx,|app,cx|app.settings_edit_project(None,cx))))
        .when(rows.is_empty(),|element|element.child(div().py(px(28.0)).text_color(theme.muted).child(settings::localized("settings_projects_empty","No projects yet. Create a launch profile to get started."))))
        .children(rows).into_any_element()
}

pub(super) fn shortcuts(
    app: &GpuiApp,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
) -> AnyElement {
    let map = shortcut_editor::ShortcutMap::from_values(app.settings_draft.values());
    let overrides = app
        .settings_value("/shortcuts/overrides")
        .unwrap_or_else(|| serde_json::json!([]));
    let rows = app
        .shortcut_rows()
        .into_iter()
        .map(|row| {
            let action = row.action;
            let id = row.id.clone();
            let recording = app.shortcut_editor.recording == Some(action);
            let custom = overrides
                .as_array()
                .is_some_and(|rows| rows.iter().any(|item| item["action"].as_str() == Some(&id)));
            let chords = map.chords(action);
            let labels = |qwerty| {
                let labels = chords
                    .iter()
                    .map(|key| shortcut_editor::label(key, qwerty))
                    .collect::<Vec<_>>();
                if labels.is_empty() {
                    settings::localized("settings_shortcut_disabled", "Disabled")
                } else {
                    labels.join("  ")
                }
            };
            div()
                .id(SharedString::from(format!("settings-shortcut-{id}")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("settings-shortcut-{id}")
                })
                .flex()
                .flex_col()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(12.0))
                .border_b_1()
                .border_color(theme.border)
                .when(recording, |element| {
                    element.bg(gpui::Rgba {
                        a: 0.06,
                        ..theme.accent
                    })
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(row.title),
                        )
                        .child(
                            div()
                                .w(px(112.0))
                                .flex_none()
                                .text_size(px(12.0))
                                .text_color(theme.fg)
                                .child(labels(false)),
                        )
                        .child(
                            div()
                                .w(px(112.0))
                                .flex_none()
                                .text_size(px(12.0))
                                .text_color(theme.muted)
                                .child(labels(true)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(6.0))
                        .when(recording, |element| {
                            element.child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(theme.accent)
                                    .child(settings::localized(
                                        "settings_shortcut_recording",
                                        "Press a shortcut. Esc cancels.",
                                    )),
                            )
                        })
                        .child(row_button(
                            format!("settings-shortcut-record-{id}"),
                            settings::localized("settings_shortcut_record", "Record"),
                            theme,
                            cx,
                            move |app, cx| app.settings_start_recording(action, cx),
                        ))
                        .child(row_button(
                            format!("settings-shortcut-disable-{id}"),
                            settings::localized("settings_shortcut_disable", "Disable"),
                            theme,
                            cx,
                            move |app, cx| app.settings_disable_shortcut(action, cx),
                        ))
                        .when(custom, |element| {
                            element.child(row_button(
                                format!("settings-shortcut-restore-{id}"),
                                settings::localized("settings_shortcut_restore", "Restore"),
                                theme,
                                cx,
                                move |app, cx| app.settings_restore_shortcut(action, cx),
                            ))
                        }),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();
    div().id("settings-shortcut-manager").debug_selector(||"settings-shortcut-manager".into()).flex().flex_col().gap(px(12.0)).p(px(18.0))
        .child(div().text_size(px(11.5)).line_height(px(17.0)).text_color(theme.muted).child(settings::localized("settings_shortcut_hand_position_hint","All default shortcuts are anchored to Colemak. QWERTY uses the same physical positions: ⌘D / ⌘S / ⌘T become ⌘G / ⌘D / ⌘F. Typing is unchanged.")))
        .child(div().id("settings-shortcut-search").debug_selector(||"settings-shortcut-search".into()).h(px(28.0)).flex().items_center().px(px(8.0)).rounded(px(6.0)).border_1().border_color(if app.shortcut_editor.searching{theme.accent}else{theme.border}).bg(theme.sidebar_bg).cursor_text()
            .text_size(px(12.0)).text_color(if app.shortcut_editor.query.is_empty(){theme.muted}else{theme.fg})
            .child(if app.shortcut_editor.query.is_empty(){settings::localized("settings_shortcut_search","Search actions or shortcuts")}else{app.shortcut_editor.query.clone()})
            .on_mouse_down(MouseButton::Left,cx.listener(|app,_,_,cx|{cx.stop_propagation();app.shortcut_editor.searching=true;app.shortcut_editor.recording=None;cx.notify();})))
        .when_some(app.shortcut_editor.error.clone(),|element,error|element.child(div().text_size(px(11.5)).text_color(theme.danger).child(error)))
        .child(div().flex().gap(px(12.0)).px(px(12.0)).text_size(px(11.0)).text_color(theme.muted)
            .child(div().flex_1().child(settings::localized("settings_shortcut_action","Action")))
            .child(div().w(px(112.0)).flex_none().child("Colemak"))
            .child(div().w(px(112.0)).flex_none().child("QWERTY")))
        .when(rows.is_empty(),|element|element.child(div().py_4().text_color(theme.muted).child(settings::localized("settings_no_results","No matching actions"))))
        .children(rows).into_any_element()
}

fn row_button(
    id: String,
    label: String,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
    action: impl Fn(&mut GpuiApp, &mut gpui::Context<GpuiApp>) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .flex_none()
        .h(px(24.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg)
        .text_size(px(11.5))
        .cursor_pointer()
        .hover(|s| s.bg(theme.active_bg))
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                action(app, cx);
            }),
        )
        .into_any_element()
}
