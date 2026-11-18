//! 用户报障回归：通过真实 GpuiApp 的输入、菜单与命令队列验收。
#![cfg(feature = "gpui")]

use gpui::{px, size, Entity, TestAppContext, VisualTestContext};
use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
use muxterm::frontend::gpui::model::existing::ExistingRuntime;
use muxterm::frontend::gpui::model::panel::PanelTab;
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
