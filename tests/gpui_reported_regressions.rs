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
