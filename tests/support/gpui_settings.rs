use super::*;
use muxterm::frontend::gpui::{
    chrome::settings_window::SettingsWindow,
    model::{
        shortcut_editor,
        shortcuts::{KeyAction, KeyChord},
        workspace_picker::{config_from_project, ProjectList},
    },
};

fn user_commands(app: &mut GpuiApp) -> Vec<ClientCommand> {
    app.take_commands_for_tests()
        .into_iter()
        .filter(|command| !matches!(command, ClientCommand::Resize { .. }))
        .collect()
}

fn project_values() -> serde_json::Value {
    serde_json::json!({"projects":[{"id":"remote","name":"Remote project","path":"~/Developer/work","runtime":{"id":"herdr","session":"dev","socket":"/tmp/named.sock"},"transport":{"id":"ssh","target":"ryzen"},"env":{"KEEP":"1"},"worktrees":[]}],"shortcuts":{"preset":"colemak","primary_key":"auto","overrides":[]}})
}

/// 对照 `testSettingsControlsFillAvailablePageWidth` / 用户两张报障截图。
#[gpui::test]
fn projects_fill_the_settings_page_not_the_control_column(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            app.settings_select("projects", cx);
        })
    });
    cx.run_until_parked();
    let page = cx.debug_bounds("settings-page").unwrap();
    let projects = cx.debug_bounds("settings-control-projects").unwrap();
    assert!(
        projects.size.width > px(400.0),
        "不能再挤进 180pt 的控件列：{projects:?}"
    );
    assert!(page.contains(&projects.origin));
    assert!(cx.debug_bounds("settings-project-new").is_some());
    assert!(cx.debug_bounds("settings-project-delete-0").is_some());
}

/// 对照 `testProjectsPageEditsAndPersistsProjectFromGUI` / `testProjectEditorPreservesSSHHostWhenEditing`。
#[gpui::test]
fn editing_a_settings_project_preserves_metadata_and_never_opens_a_workspace(
    cx: &mut TestAppContext,
) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            let config = config_from_project(&project_values()["projects"][0]).unwrap();
            app.settings_edit_project(Some(&config), cx);
            app.update_target_editor(|editor| editor.set_name("Renamed"), cx);
            app.save_target_editor(cx);
            let value = app.settings_value("/projects").unwrap();
            assert_eq!(value[0]["name"], "Renamed");
            assert_eq!(value[0]["transport"]["target"], "ryzen");
            assert_eq!(value[0]["env"]["KEEP"], "1");
            assert!(app.settings_dirty());
            assert!(user_commands(app).is_empty(), "只改设置草稿，不打开工作区");
            app.settings_discard(cx);
            assert_eq!(
                app.settings_value("/projects").unwrap()[0]["name"],
                "Remote project"
            );
        })
    });
}

/// 对照 `testProjectsPageCreatesProjectAndRefreshesList`。
#[gpui::test]
fn projects_can_be_created_and_deleted_in_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            app.settings_edit_project(None, cx);
            app.update_target_editor(
                |editor| {
                    editor.set_name("Local project");
                    editor.set_path("/tmp/work");
                },
                cx,
            );
            app.save_target_editor(cx);
            let projects = ProjectList::new(&app.settings_value("/projects").unwrap()).configs();
            assert_eq!(projects.len(), 2);
            let created = projects
                .iter()
                .find(|project| project.name == "Local project")
                .unwrap();
            app.settings_delete_project(created, cx);
            assert_eq!(
                app.settings_value("/projects")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert!(user_commands(app).is_empty());
        })
    });
}

/// 对照 `testShortcutDialogSearchesDisablesAndSavesWithoutChangingCancel`。
#[gpui::test]
fn shortcut_record_disable_restore_and_cancel_share_one_draft(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            app.settings_select("shortcuts", cx);
            app.settings_disable_shortcut(KeyAction::NewTab, cx);
            assert_eq!(
                shortcut_editor::ShortcutMap::from_values(&app.settings_value("/").unwrap())
                    .resolve(&KeyChord::command("t")),
                None
            );
            app.settings_restore_shortcut(KeyAction::NewTab, cx);
            assert_eq!(
                app.settings_value("/shortcuts/overrides").unwrap(),
                serde_json::json!([])
            );
            app.settings_start_recording(KeyAction::NewTab, cx);
            app.settings_record_shortcut(KeyChord::command("h"), cx);
            assert_eq!(
                app.settings_value("/shortcuts/overrides").unwrap()[0]["bindings"][0]["key"],
                "h"
            );
            assert!(user_commands(app).is_empty());
            app.settings_confirm_discard(cx);
            assert_eq!(
                app.settings_value("/shortcuts/overrides").unwrap(),
                serde_json::json!([])
            );
        })
    });
}

#[gpui::test]
fn shortcut_manager_is_visible_and_lists_both_layout_labels(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            app.settings_select("shortcuts", cx);
        })
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-category-shortcuts").is_some());
    assert!(cx.debug_bounds("settings-shortcut-manager").is_some());
    assert!(cx.debug_bounds("settings-shortcut-search").is_some());
    assert!(cx
        .debug_bounds("settings-shortcut-record-new_tab")
        .is_some());
    assert_eq!(shortcut_editor::label(&KeyChord::command("t"), false), "⌘T");
    assert_eq!(shortcut_editor::label(&KeyChord::command("t"), true), "⌘F");
}

#[gpui::test]
fn settings_reject_conflicting_recordings_without_executing_them(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            app.settings_start_recording(KeyAction::NewTab, cx);
            app.settings_record_shortcut(KeyChord::command("d"), cx);
            assert_eq!(
                app.settings_value("/shortcuts/overrides").unwrap(),
                serde_json::json!([])
            );
            assert!(user_commands(app).is_empty());
        })
    });
}

#[gpui::test]
fn rejected_settings_apply_keeps_the_window_and_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({"font":{"size":18.0}}),
                cx,
            );
            app.settings_set("/font/size", serde_json::json!(-1), cx);
            app.settings_apply_and_close(cx);
            assert!(app.settings_open);
            app.complete_settings_apply(false, cx);
            assert!(app.settings_open);
            assert!(app.settings_dirty());
            assert_eq!(
                app.settings_value("/font/size"),
                Some(serde_json::json!(-1))
            );
        })
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-apply").is_some());
}

/// 对照 Swift 设置是独立 NSWindow：关设置不会关主窗口，也不会建第二个 Core。
#[gpui::test]
fn settings_are_a_separate_reusable_window(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &project_values(), cx);
            app.detach_settings_window(cx).unwrap();
        })
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.windows().len()), 2);
    assert!(
        cx.debug_bounds("settings-overlay").is_none(),
        "主终端不再有遮罩"
    );
    cx.update(|_, cx| view.update(cx, |app, cx| app.detach_settings_window(cx).unwrap()));
    assert_eq!(
        cx.update(|_, cx| cx.windows().len()),
        2,
        "重复打开只聚焦既有设置窗口"
    );
    let handle = cx.update(|_, cx| {
        cx.windows()
            .into_iter()
            .find(|handle| handle.downcast::<SettingsWindow>().is_some())
            .unwrap()
    });
    let _ = handle;
    cx.update(|_, cx| view.update(cx, |app, cx| app.settings_cancel(cx)));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| cx.windows().len()),
        1,
        "只关设置，主窗口仍在"
    );
}
