//! 无头 chrome 验收：挂载真实 `GpuiApp`，点行、按快捷键、断言状态。
//!
//! 对应 macOS `MuxtermAppUITests` 里「点侧栏切工作区 / Cmd-Ctrl-数字 / Cmd-B」
//! 这几条路径；无头窗口没有真实 AppKit 控件，所以断言落在状态与命令队列上。

#![cfg(feature = "gpui")]

use gpui::{px, size, TestAppContext};
use muxterm::frontend::command_queue::ClientCommand;
use muxterm::frontend::gpui::chrome::sidebar_view::{CommandKey, RowTarget, Section};
use muxterm::frontend::gpui::model::attention::{
    AttentionSnapshot, PaneAttention, PaneAttentionStatus, WorkspaceAttention,
};
use muxterm::frontend::gpui::model::sidebar::backend_status;
use muxterm::frontend::gpui::state::GpuiApp;
use muxterm::frontend::utils::corebridge::ClientTask;
use muxterm::frontend::utils::corebridge::{ClientPane, ClientTab, ClientWorkspace};
use muxterm::frontend::view_store::ViewStore;

fn workspace(id: &str, name: &str, active: bool) -> ClientWorkspace {
    ClientWorkspace {
        id: id.to_string(),
        name: name.to_string(),
        runtime: "tmux".to_string(),
        active,
        resolved_target: None,
    }
}

fn store_with_two_workspaces() -> ViewStore {
    let mut store = ViewStore::default();
    for (id, name, active) in [
        ("local@@dev@tmux@dev", "dev", true),
        ("local@@ops@tmux@ops", "ops", false),
    ] {
        store.replace_topology(
            workspace(id, name, active),
            vec![ClientTab {
                id: if active { 1 } else { 2 },
                name: name.to_string(),
                is_active: true,
            }],
            vec![(
                if active { 1 } else { 2 },
                vec![ClientPane {
                    id: if active { 10 } else { 20 },
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: name.to_string(),
                }],
            )],
        );
        store.ensure_workspace(id).backend_status = Some(backend_status::CONNECTED);
    }
    store
}

/// 挂载真实 `GpuiApp`，返回视图与窗口上下文。
fn mount<'a>(
    store: ViewStore,
    cx: &'a mut TestAppContext,
) -> (gpui::Entity<GpuiApp>, &'a mut gpui::VisualTestContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let app = GpuiApp::new_headless(store, cx);
        // 与 `app.rs` 的生产路径一样：窗口一建好就把焦点交给根视图，
        // 否则按键到不了 `handle_key`。
        window.focus(&app.focus_handle());
        app
    });
    cx.simulate_resize(size(px(1_200.), px(800.)));
    (view, cx)
}

/// 侧栏行按打开顺序铺开，快捷键与详情来自投影。
#[gpui::test]
fn sidebar_rows_render_the_projection(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let model = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_model()));
    let workspaces = &model[0];
    assert_eq!(workspaces.section, Section::Workspaces);
    assert_eq!(workspaces.rows.len(), 2);
    assert_eq!(workspaces.rows[0].title, "dev");
    assert_eq!(workspaces.rows[0].shortcut.as_deref(), Some("1"));
    assert_eq!(workspaces.rows[1].title, "ops");
    assert_eq!(workspaces.rows[1].shortcut.as_deref(), Some("2"));
    assert!(workspaces.rows[0].active, "第一个工作区是当前活动");
    assert_eq!(workspaces.rows[0].detail, "tmux @ local");
}

/// 点第二行切工作区（`onWorkspaceActivate` 的等价路径）。
#[gpui::test]
fn clicking_a_sidebar_row_switches_workspace(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.activate_sidebar_row(RowTarget::Workspace("local@@ops@tmux@ops".to_string()), cx);
        })
    });
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(visible.as_deref(), Some("local@@ops@tmux@ops"));
}

/// Cmd-Ctrl-2 切到第二个工作区；Cmd-Ctrl-0 回到最后一个。
#[gpui::test]
fn workspace_shortcuts_follow_the_key_table(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-ctrl-2");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(visible.as_deref(), Some("local@@ops@tmux@ops"));

    cx.simulate_keystrokes("cmd-ctrl-0");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(
        visible.as_deref(),
        Some("local@@ops@tmux@ops"),
        "0 永远是最后一个，这里仍然是 ops"
    );

    cx.simulate_keystrokes("cmd-ctrl-1");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(visible.as_deref(), Some("local@@dev@tmux@dev"));
}

/// Cmd-B 切换侧栏，同时改变主区宽度。
#[gpui::test]
fn cmd_b_toggles_the_sidebar(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_visible())));
    cx.simulate_keystrokes("cmd-b");
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_visible())));
    cx.simulate_keystrokes("cmd-b");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_visible())));
}

/// 折叠段只影响绘制：模型里行还在，展开状态被记住。
#[gpui::test]
fn collapsing_a_section_is_remembered(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.toggle_sidebar_section(Section::Workspaces, cx)
        })
    });
    let model = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_model()));
    assert!(!model[0].expanded);
    assert_eq!(model[0].rows.len(), 2, "折叠不删行");
}

/// 命令可见性切换：同一行在两段之间移动，且只在前端生效。
#[gpui::test]
fn hiding_a_command_moves_it_between_sections(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let snapshot = AttentionSnapshot {
        blocked_count: 0,
        workspaces: vec![WorkspaceAttention {
            workspace_id: "local@@dev@tmux@dev".to_string(),
            name: "dev".to_string(),
            transport: "local".to_string(),
            ..WorkspaceAttention::new(
                "local@@dev@tmux@dev",
                vec![PaneAttention {
                    pane_id: 10,
                    status: PaneAttentionStatus::Working,
                    last_line: "cargo test".to_string(),
                    seq: 4,
                    process_name: Some("cargo".to_string()),
                    ..PaneAttention::default()
                }],
            )
        }],
    };
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_attention(Some(snapshot));
            app.toggle_command_visibility(
                CommandKey {
                    workspace_id: "local@@dev@tmux@dev".to_string(),
                    pane_id: 10,
                },
                cx,
            );
        })
    });
    let model = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_model()));
    let commands = model
        .iter()
        .find(|model| model.section == Section::Commands)
        .unwrap();
    let hidden = model
        .iter()
        .find(|model| model.section == Section::HiddenCommands)
        .unwrap();
    assert!(commands.rows.is_empty());
    assert_eq!(hidden.rows.len(), 1);
    assert_eq!(hidden.rows[0].trailing.as_deref(), Some("eye"));
}

/// Cmd-T 建新 tab：动作表命中后必须真的排进命令队列。
#[gpui::test]
fn cmd_t_creates_a_tab_through_the_command_queue(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-t");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::NewTab,
                ..
            }
        )),
        "commands were {commands:?}"
    );
}

/// 侧栏关闭按钮排队 `CloseWorkspace`：工作区的销毁由 Core 事件确认。
#[gpui::test]
fn closing_a_workspace_queues_the_close_command(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.close_workspace("local@@ops@tmux@ops".to_string(), cx)
        })
    });
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert_eq!(
        commands,
        vec![ClientCommand::CloseWorkspace(
            "local@@ops@tmux@ops".to_string()
        )]
    );
}

/// Cmd-1 切 tab 走 Core 往返：前端只排队，不自行改活动 tab。
#[gpui::test]
fn switching_a_tab_queues_the_tab_switch(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.select_tab(7, cx)));
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(matches!(
        commands.first(),
        Some(ClientCommand::Task {
            task: ClientTask::SwitchTab { tab_id: 7 },
            ..
        })
    ));
}
