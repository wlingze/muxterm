//! 回归 Swift 的真实 Quick Panel 操作链；不把改文案当成迁移完成。
#![cfg(feature = "gpui")]
#[path = "support/gpui_keys.rs"]
mod keys;
use gpui::{px, size, Entity, Modifiers, TestAppContext, VisualTestContext};
use keys::keys;
use muxterm::frontend::gpui::{
    chrome::panel_view::{AggregateKind, PanelRow, RowAction},
    model::{
        existing::ExistingRuntime, panel::PanelTab, target::TargetRuntime,
        workspace_picker::PickerStage,
    },
    state::GpuiApp,
};
use muxterm::frontend::utils::corebridge::{
    ClientCandidateRef, ClientOpenIntent, ClientPane, ClientTab, ClientWorkspace,
};
use muxterm::frontend::{command_queue::ClientCommand, view_store::ViewStore};
use serde_json::{json, Value};

fn store() -> ViewStore {
    let mut store = ViewStore::default();
    for (id, runtime, active) in [
        ("shell", "shell", false),
        ("one", "tmux", true),
        ("two", "herdr", false),
    ] {
        store.replace_topology(
            ClientWorkspace {
                id: id.into(),
                name: id.into(),
                runtime: runtime.into(),
                active,
                resolved_target: None,
            },
            vec![ClientTab {
                id: 1,
                name: "terminal".into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 10,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "terminal".into(),
                }],
            )],
        );
    }
    store
}
fn mount(cx: &mut TestAppContext) -> (Entity<GpuiApp>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let app = GpuiApp::new_headless(store(), cx);
        window.focus(&app.focus_handle());
        app
    });
    cx.simulate_resize(size(px(1200.), px(800.)));
    (view, cx)
}
fn projects() -> Value {
    json!([{"id":"saved","name":"saved","path":"~/dev/saved","runtime":{"id":"tmux","session":"saved"},"transport":{"id":"ssh","target":"ryzen"},"template":"monitor"}])
}
fn existing(name: &str) -> ExistingRuntime {
    ExistingRuntime {
        name: name.into(),
        runtime: "herdr".into(),
        transport: "ssh".into(),
        target: "ryzen".into(),
        session: "default".into(),
        socket: Some("/run/herdr.sock".into()),
        workspace_id: Some("w2".into()),
        path: String::new(),
        windows: 1,
        attached: false,
    }
}
fn activate(view: &Entity<GpuiApp>, cx: &mut VisualTestContext, title: &str) {
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            let rows = app.panel_rows();
            let index = rows
                .iter()
                .position(|row| row.title() == title)
                .unwrap_or_else(|| panic!("missing {title}: {rows:?}"));
            app.select_panel_row(index, cx);
        })
    });
}
fn titles(view: &Entity<GpuiApp>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.panel_rows().iter().map(PanelRow::title).collect()
        })
    })
}
fn open(view: &Entity<GpuiApp>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_picker_data_for_tests(projects(), vec!["ryzen".into(), "archmini".into()]);
            app.set_existing_for_tests(vec![existing("discovered")]);
            app.show_panel(PanelTab::Workspaces, cx);
            app.take_commands_for_tests();
        })
    });
}

/// 对照 `testOpenWorkspaceRowsStayInShortcutOrderWhenAttachCountsDiffer` 与截图一、二。
#[gpui::test]
fn root_pins_shells_agents_open_then_numbered_workspaces_projects_and_discovery(
    cx: &mut TestAppContext,
) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    assert_eq!(
        titles(&view, cx),
        [
            "Shells",
            "Agents",
            "Open",
            "one · local",
            "two · local",
            "saved",
            "discovered"
        ]
    );
    let numbers = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.panel_rows()
                .iter()
                .filter_map(|row| match row {
                    PanelRow::Workspace { shortcut, .. } => Some(*shortcut),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(
        numbers,
        vec![Some(1), Some(2)],
        "shell 聚合源不能占用 workspace 数字编号"
    );
}

/// 对照 `testOpenFiltersMachinesThenListsSessionsOnThatMachine`，Open 不再打开旧表单。
#[gpui::test]
fn open_is_machine_then_runtime_inventory_not_the_target_editor(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "Open");
    assert_eq!(titles(&view, cx), ["‹ Back", "local", "ryzen", "archmini"]);
    cx.update(|_, cx| view.update(cx, |app, _| assert!(app.target_editor().is_none())));
    activate(&view, cx, "ryzen");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_host_sessions_for_tests(vec![existing("found")])
        })
    });
    assert_eq!(titles(&view, cx), ["‹ Back", "+ Open New Path", "found"]);
}

/// 对照 `testOpenNewPathCompletesDirectoriesAndConfirms`；新增末端确认而非开表单。
#[gpui::test]
fn path_picker_waits_for_runtime_and_editable_name_before_creating(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "Open");
    activate(&view, cx, "ryzen");
    activate(&view, cx, "+ Open New Path");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_panel_query("~/dev/", cx);
            assert!(app.apply_picker_directories_for_tests(vec!["muxterm".into(), "notes".into()]));
        })
    });
    activate(&view, cx, "muxterm");
    activate(&view, cx, "Open This Directory");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            let PickerStage::Confirm(draft) = app.panel_stage() else {
                panic!("must confirm")
            };
            assert_eq!(draft.runtime, TargetRuntime::Herdr);
            assert_eq!(draft.name, "muxterm");
            assert_eq!(draft.path, "~/dev/muxterm");
            assert!(
                app.take_commands_for_tests().iter().all(|c| !matches!(
                    c,
                    ClientCommand::OpenTarget { .. } | ClientCommand::OpenCandidate { .. }
                )),
                "确认之前不能创建"
            );
            app.choose_panel_runtime(TargetRuntime::Tmux, cx);
            app.set_panel_query("custom", cx);
        })
    });
    activate(&view, cx, "Create Workspace");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let commands = app.take_commands_for_tests();
            let (target, intent) = commands
                .iter()
                .find_map(|c| match c {
                    ClientCommand::OpenTarget { target, intent } => Some((target, intent)),
                    _ => None,
                })
                .expect("CreateIfMissing");
            assert_eq!(*intent, ClientOpenIntent::CreateIfMissing);
            assert_eq!(target.runtime, "tmux");
            assert_eq!(target.name, "custom");
            assert_eq!(target.target.as_deref(), Some("ryzen"));
            assert_eq!(target.path, "~/dev/muxterm");
            assert_eq!(target.workspace_id, None);
        })
    });
}

#[gpui::test]
fn empty_name_does_not_create_or_close_the_panel(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "Open");
    activate(&view, cx, "local");
    activate(&view, cx, "+ Open New Path");
    activate(&view, cx, "Open This Directory");
    cx.update(|_, cx| view.update(cx, |app, cx| app.set_panel_query(" ", cx)));
    activate(&view, cx, "Create Workspace");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            assert!(app.panel_open());
            assert!(app
                .panel_rows()
                .iter()
                .any(|row| matches!(row, PanelRow::Status(_))));
            assert!(!app.take_commands_for_tests().iter().any(|c| matches!(
                c,
                ClientCommand::OpenTarget { .. } | ClientCommand::OpenCandidate { .. }
            )));
        })
    });
}

/// 对照 `testQuickActionsStayInABottomRightPopover`，不触发全局 Shells。
#[gpui::test]
fn cmd_k_opens_context_actions_and_escape_restores_query_and_tab(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.set_panel_query("discovered", cx)));
    cx.simulate_keystrokes(&keys("cmd-k"));
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            assert_eq!(app.aggregate_kind(), None);
            assert_eq!(
                app.panel_actions().unwrap().rows(),
                [RowAction::Attach, RowAction::Save]
            );
        })
    });
    let panel = cx.debug_bounds("quick-panel").expect("panel");
    let menu = cx.debug_bounds("panel-actions").expect("popover");
    assert!(menu.origin.x > panel.origin.x);
    assert!(menu.origin.y > panel.origin.y);
    cx.simulate_keystrokes("s a v e");
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            assert!(app.panel_open());
            assert!(app.panel_actions().is_none());
            assert_eq!(app.panel_query(), "discovered");
            assert_eq!(app.panel_tab(), PanelTab::Workspaces);
        })
    });
}

/// Save 按钮点中必须只保存，不可同时 attach；新记录保留 w2/socket/session。
#[gpui::test]
fn discovered_save_button_persists_without_attaching(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.set_panel_query("discovered", cx)));
    cx.run_until_parked();
    // 搜索会按机器分组，标题行也占行号：按前缀找 Save 按钮，不写死 index。
    let save = (0..)
        .find_map(|index| {
            cx.debug_bounds(Box::leak(format!("panel-save-{index}").into_boxed_str()))
        })
        .expect("Save 按钮必须渲染");
    cx.simulate_click(save.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let commands = app.take_commands_for_tests();
            assert!(!commands.iter().any(|c| matches!(
                c,
                ClientCommand::OpenTarget { .. } | ClientCommand::OpenCandidate { .. }
            )));
            let value = commands
                .iter()
                .find_map(|c| match c {
                    ClientCommand::ConfigApply { path, value } if path == "projects" => Some(value),
                    _ => None,
                })
                .expect("projects config transaction");
            assert_eq!(value[0]["template"], "monitor");
            assert_eq!(value[1]["runtime"]["workspace_id"], "w2");
            assert_eq!(value[1]["runtime"]["socket"], "/run/herdr.sock");
            assert_eq!(value[1]["runtime"]["session"], "default");
        })
    });
}

#[gpui::test]
fn herdr_attach_keeps_its_canonical_workspace_identity(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "discovered");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let commands = app.take_commands_for_tests();
            let request = commands
                .iter()
                .find_map(|c| match c {
                    ClientCommand::OpenCandidate { request } => Some(request),
                    _ => None,
                })
                .unwrap();
            assert_eq!(request.intent, ClientOpenIntent::AttachOnly);
            let ClientCandidateRef::Existing { identity } = &request.candidate else {
                panic!("Existing")
            };
            assert_eq!(identity.workspace_id.as_deref(), Some("w2"));
            assert_eq!(identity.session.as_deref(), Some("default"));
            assert_eq!(identity.socket.as_deref(), Some("/run/herdr.sock"));
            assert_eq!(identity.target, "ryzen");
        })
    });
}

#[gpui::test]
fn stored_project_opens_by_stable_project_id(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "saved");
    cx.update(|_,cx|view.update(cx,|app,_|assert!(app.take_commands_for_tests().iter().any(|c|matches!(c,ClientCommand::OpenCandidate{request} if request.candidate==ClientCandidateRef::Project{project_id:"saved".into()})))));
}

#[gpui::test]
fn escape_walks_back_one_step_and_never_creates(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "Open");
    activate(&view, cx, "local");
    activate(&view, cx, "+ Open New Path");
    activate(&view, cx, "Open This Directory");
    for expected in ["path", "sessions", "hosts", "root"] {
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            view.update(cx, |app, _| {
                let stage = match app.panel_stage() {
                    PickerStage::Root => "root",
                    PickerStage::Hosts => "hosts",
                    PickerStage::Sessions(_) => "sessions",
                    PickerStage::Path(_) => "path",
                    _ => "confirm",
                };
                assert_eq!(stage, expected);
                assert!(app.panel_open());
            })
        });
    }
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| view.update(cx, |app, _| assert!(!app.panel_open())));
}

#[gpui::test]
fn fixed_aggregates_do_not_become_core_workspaces(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "Shells");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            assert_eq!(app.aggregate_kind(), Some(AggregateKind::Shells));
            assert_eq!(app.aggregate_pages().len(), 1);
            assert!(app.take_commands_for_tests().iter().all(
                |c| !matches!(c,ClientCommand::ActivateWorkspace(id) if id.contains("aggregate"))
            ));
        })
    });
}

#[gpui::test]
fn panel_owns_text_input_and_arrows_do_not_leak_to_pty(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    cx.simulate_keystrokes("a b c down up backspace");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.commit_text("中文", cx);
            assert!(app.panel_query().contains("中文"));
            assert!(!app
                .take_commands_for_tests()
                .iter()
                .any(|c| matches!(c, ClientCommand::Input { .. })));
        })
    });
}

/// 对照 `QuickPanelActions.contextual(for:)`：Cmd-K 的动作集按行类型给出，
/// 不是一张全局命令表。
#[gpui::test]
fn action_menu_matches_the_swift_row_contexts(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::{ActionMenu, AggregateKind, PanelRow};
    use muxterm::frontend::gpui::model::quick_connect::TargetConfig;
    let menu = |target| {
        ActionMenu {
            target,
            query: String::new(),
            selected: 0,
        }
        .rows()
    };
    let workspace = PanelRow::Workspace {
        workspace_id: "ws".into(),
        title: "dev".into(),
        detail: String::new(),
        active: true,
        shortcut: Some(1),
    };
    assert_eq!(
        menu(PanelRow::Aggregate(AggregateKind::Shells)),
        [RowAction::Open],
        "聚合行只有 Open"
    );
    assert_eq!(menu(workspace), [RowAction::Open, RowAction::Close]);
    let project = TargetConfig {
        name: "saved".into(),
        runtime: TargetRuntime::Tmux,
        transport: muxterm::frontend::gpui::model::target::TargetTransport::Local,
        path: "~/dev/saved".into(),
        socket: None,
        session: None,
        workspace_id: None,
        persisted_id: Some("saved".into()),
    };
    assert_eq!(
        menu(PanelRow::Target {
            config: project.clone()
        }),
        [RowAction::Connect, RowAction::Edit, RowAction::Delete]
    );
    let attached = PanelRow::Existing {
        index: 0,
        title: "found".into(),
        detail: String::new(),
        config: existing("found").config(),
        saved: true,
    };
    assert_eq!(
        menu(attached),
        [RowAction::Connect, RowAction::Edit, RowAction::Delete],
        "已保存的现有 runtime 是 target 上下文"
    );
    let discovered = PanelRow::Existing {
        index: 0,
        title: "found".into(),
        detail: String::new(),
        config: existing("found").config(),
        saved: false,
    };
    assert_eq!(menu(discovered), [RowAction::Attach, RowAction::Save]);
    // Swift `.open` 行没有行内动作。
    assert!(menu(PanelRow::Action(
        muxterm::frontend::gpui::chrome::panel_view::PickerAction::Open
    ))
    .is_empty());
}

/// Confirm 步骤必须真的画出 runtime 切换与名字，点切换会改 draft。
#[gpui::test]
fn confirm_step_renders_runtime_toggle_and_takes_clicks(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    open(&view, cx);
    activate(&view, cx, "Open");
    activate(&view, cx, "local");
    activate(&view, cx, "+ Open New Path");
    activate(&view, cx, "Open This Directory");
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            assert!(matches!(app.panel_stage(), PickerStage::Confirm(_)));
        })
    });
    let herdr = cx.debug_bounds("panel-runtime-Herdr").expect("Herdr 选项");
    let tmux = cx.debug_bounds("panel-runtime-Tmux").expect("tmux 选项");
    assert!(herdr.origin.x < tmux.origin.x, "herdr 在 tmux 左边");
    cx.simulate_click(tmux.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let PickerStage::Confirm(draft) = app.panel_stage() else {
                panic!("仍在确认页")
            };
            assert_eq!(draft.runtime, TargetRuntime::Tmux, "点击必须改 runtime");
        })
    });
}
