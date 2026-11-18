//! 用户报障回归：通过真实 GpuiApp 的输入、菜单与命令队列验收。
#![cfg(feature = "gpui")]

use gpui::{px, size, Entity, TestAppContext, VisualTestContext};
use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
use muxterm::frontend::gpui::model::existing::ExistingRuntime;
use muxterm::frontend::gpui::model::machine_inventory::MachineInventory;
use muxterm::frontend::gpui::model::panel::PanelTab;
use muxterm::frontend::gpui::model::target::TargetTransport;
use muxterm::frontend::gpui::state::GpuiApp;
use muxterm::frontend::utils::corebridge::{ClientPane, ClientTab, ClientWorkspace};
use muxterm::frontend::view_store::ViewStore;

fn topology(runtime: &str, id: &str, resolved: Option<serde_json::Value>) -> ViewStore {
    let mut store = ViewStore::default();
    store.replace_topology(
        ClientWorkspace {
            id: id.into(),
            name: "muxterm".into(),
            runtime: runtime.into(),
            active: true,
            resolved_target: resolved,
        },
        vec![ClientTab {
            id: 1,
            name: "main".into(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 10,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "zsh".into(),
            }],
        )],
    );
    store
}

fn local_muxterm() -> ExistingRuntime {
    ExistingRuntime {
        name: "muxterm".into(),
        runtime: "tmux".into(),
        transport: "local".into(),
        target: String::new(),
        session: "muxterm".into(),
        socket: None,
        windows: 2,
        attached: true,
        workspace_id: None,
        path: String::new(),
    }
}

fn mount(store: ViewStore, cx: &mut TestAppContext) -> (Entity<GpuiApp>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let app = GpuiApp::new_headless(store, cx);
        window.focus(&app.focus_handle());
        app
    });
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    (view, cx)
}

/// 对照 `UnifiedPanelExistingConnectionsE2ETests`：Shells 的同名本机 shell
/// 不是本机 tmux session，不能把后者从 discovery 结果中过滤掉。
#[gpui::test]
fn local_tmux_survives_a_same_named_shell(cx: &mut TestAppContext) {
    let (view, cx) = mount(topology("shell", "local-shell", None), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_existing_for_tests(vec![local_muxterm()]);
            app.show_panel(PanelTab::Workspaces, cx);
            app.panel_push_query("muxterm", cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert!(
        rows.iter()
            .any(|row| matches!(row, PanelRow::Existing { config, .. }
        if config.name == "muxterm" && config.transport.label() == "local")),
        "同名 shell 不能吞掉本机 tmux: {rows:?}"
    );
}

/// 对照 `UnifiedPanelExistingConnectionsE2ETests`：同名、同 session 的 SSH
/// workspace 不等于本机 tmux，不能跨 transport 去重。
#[gpui::test]
fn local_tmux_survives_a_same_named_remote_session(cx: &mut TestAppContext) {
    let (view, cx) = mount(
        topology(
            "tmux",
            "remote-muxterm",
            Some(serde_json::json!({
                "canonical": { "runtime":"tmux", "transport":"ssh", "target":"ryzen", "session":"muxterm" },
                "spec": { "runtime":"tmux", "transport":"ssh", "target":"ryzen", "session":"muxterm", "path":"~" }
            })),
        ),
        cx,
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_existing_for_tests(vec![local_muxterm()]);
            app.show_panel(PanelTab::Workspaces, cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert!(
        rows.iter()
            .any(|row| matches!(row, PanelRow::Existing { config, .. }
        if config.transport.label() == "local")),
        "远程同名 session 不能吞掉本机 tmux: {rows:?}"
    );
}

fn remote(machine: &str, session: &str) -> ExistingRuntime {
    ExistingRuntime {
        name: session.into(),
        runtime: "tmux".into(),
        transport: "ssh".into(),
        target: machine.into(),
        session: session.into(),
        socket: None,
        windows: 1,
        attached: false,
        workspace_id: None,
        path: String::new(),
    }
}

fn ryzen() -> TargetTransport {
    TargetTransport::Ssh {
        name: "ryzen".into(),
    }
}

/// 报障回归：搜索时慢机器还在路上，界面先显示「没有匹配结果」，过一会结果
/// 又出现。已解析完的机器结果必须立即显示；没回来的机器只能标「加载中」，
/// 不能当成「没有匹配」。
#[gpui::test]
fn a_slow_machine_is_loading_not_an_empty_result(cx: &mut TestAppContext) {
    // 本机回包了（没有命中），远端还在路上。
    let mut inventory = MachineInventory::default();
    inventory.begin(&[TargetTransport::Local, ryzen()]);
    inventory.finish(&TargetTransport::Local, vec![local_muxterm()]);
    let (view, cx) = mount(topology("tmux", "local-muxterm", None), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_machine_inventory_for_tests(inventory);
            app.show_panel(PanelTab::Workspaces, cx);
            app.set_panel_query("zzz", cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-empty").is_none(),
        "远端还没回包时不能报「没有匹配结果」"
    );
    // 标题行要真的画出来，并带上「正在检查」的状态。
    assert!(
        cx.debug_bounds("panel-heading-status-0").is_some(),
        "还在加载的机器必须有标题和加载状态"
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            assert!(
                matches!(app.panel_rows().first(), Some(PanelRow::Heading { .. })),
                "慢机器要留下标题行：{:?}",
                app.panel_rows()
            );
            // 标题行不是动作行：Enter 不能触发任何命令（先丢掉首帧 Resize）。
            let _ = app.take_commands_for_tests();
            app.activate_panel_row(cx);
            let commands = app.take_commands_for_tests();
            assert!(
                commands.is_empty(),
                "Enter 落在机器标题上不能打开任何东西：{commands:?}"
            );
        })
    });

    // 一台都还没回：同样是「加载中」，不是空结果。
    let mut pending = MachineInventory::default();
    pending.begin(&[TargetTransport::Local, ryzen()]);
    let (view, cx) = mount(topology("tmux", "local-muxterm", None), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_machine_inventory_for_tests(pending);
            app.show_panel(PanelTab::Workspaces, cx);
            app.set_panel_query("zzz", cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-empty").is_none(),
        "所有机器都在路上时不能报「没有匹配结果」"
    );
}

/// 报障回归：迟到的机器结果不能抢走用户已经选中的行。
#[gpui::test]
fn a_late_machine_result_keeps_the_selected_row(cx: &mut TestAppContext) {
    let mut inventory = MachineInventory::default();
    inventory.begin(&[TargetTransport::Local, ryzen()]);
    inventory.finish(
        &TargetTransport::Local,
        vec![remote("local", "alpha"), remote("local", "beta")],
    );
    let (view, cx) = mount(topology("tmux", "local-muxterm", None), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_machine_inventory_for_tests(inventory);
            app.show_panel(PanelTab::Workspaces, cx);
            let rows = app.panel_rows();
            let target = rows
                .iter()
                .position(|row| row.title() == "alpha")
                .expect("本机结果必须立即显示");
            while app.panel_selected() < target {
                app.move_panel_selection(1, cx);
            }
            assert_eq!(app.panel_rows()[app.panel_selected()].title(), "alpha");
        })
    });

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.finish_machine_for_tests(&ryzen(), vec![remote("ryzen", "api")], cx);
        })
    });
    let selected =
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()[app.panel_selected()].title()));
    assert_eq!(selected, "alpha", "远端回包不能把选中行顶掉");
}

/// 报障回归：普通 zsh 里滚轮完全无效，看不到历史记录。
///
/// Herdr 这类 Runtime 的历史在服务端，前端根本没有本地 scrollback；
/// 滚轮必须发 `ScrollPane` 交给 Runtime，不能落进「本地历史」这条空路。
/// 能力从 Core 的 provider 元数据问，不按 runtime 名字分支。
#[gpui::test]
fn the_wheel_reaches_a_server_scroll_runtime(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::gpui::state::WheelInput;
    use muxterm::frontend::utils::corebridge::{ClientRuntimeInfo, ClientTask};

    const WORKSPACE: &str = "remote@@ryzen@herdr@wH";
    let (view, cx) = mount(topology("herdr", WORKSPACE, None), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_runtime_info_for_tests(vec![ClientRuntimeInfo {
                id: "herdr".into(),
                name: "Herdr".into(),
                support: vec!["ServerScroll".into()],
                accepted_transports: vec!["ssh".into()],
            }]);
            app.feed_pane_for_tests(WORKSPACE, 10, 80, 24, b"plain zsh\r\n", cx);
            app.handle_wheel(
                WheelInput {
                    pane_id: 10,
                    delta_y: 3.0,
                    precise: false,
                    shift: false,
                    col: 0,
                    row: 0,
                },
                cx,
            );
        })
    });
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::ScrollPane {
                    pane_id: 10,
                    lines: 3
                },
                ..
            }
        )),
        "服务端持历史的 pane 必须收到 ScrollPane：{commands:?}"
    );
}
