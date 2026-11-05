//! 无头 chrome 验收：挂载真实 `GpuiApp`，点行、按快捷键、断言状态。
//!
//! 对应 macOS `MuxtermAppUITests` 里「点侧栏切工作区 / Cmd-Ctrl-数字 / Cmd-B」
//! 这几条路径；无头窗口没有真实 AppKit 控件，所以断言落在状态与命令队列上。

#![cfg(feature = "gpui")]

use gpui::{px, size, TestAppContext};
use muxterm::frontend::command_queue::ClientCommand;
use muxterm::frontend::gpui::chrome::sidebar_view::{CommandKey, RowTarget, Section};
use muxterm::frontend::gpui::chrome::tab_bar_view::TabBarPosition;
use muxterm::frontend::gpui::model::attention::{
    AttentionSnapshot, PaneAttention, PaneAttentionStatus, WorkspaceAttention,
};
use muxterm::frontend::gpui::model::panel::PanelTab;
use muxterm::frontend::gpui::model::search::SearchSnapshot;
use muxterm::frontend::gpui::model::sidebar::backend_status;
use muxterm::frontend::gpui::model::sidebar::Indicator;
use muxterm::frontend::gpui::model::target::TargetTransport;
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
    let commands: Vec<ClientCommand> = commands
        .into_iter()
        .filter(|command| !matches!(command, ClientCommand::Resize { .. }))
        .collect();
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
    let commands: Vec<ClientCommand> = commands
        .into_iter()
        .filter(|command| !matches!(command, ClientCommand::Resize { .. }))
        .collect();
    assert!(matches!(
        commands.first(),
        Some(ClientCommand::Task {
            task: ClientTask::SwitchTab { tab_id: 7 },
            ..
        })
    ));
}

/// tab 栏：活动 pane 点亮它所属的 tab，其余 tab 保持安静。
#[gpui::test]
fn tab_bar_lights_the_busy_tab(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    store.replace_topology(
        workspace("ws", "dev", true),
        vec![
            ClientTab {
                id: 1,
                name: "quiet".to_string(),
                is_active: true,
            },
            ClientTab {
                id: 2,
                name: "busy".to_string(),
                is_active: false,
            },
        ],
        vec![
            (
                1,
                vec![ClientPane {
                    id: 10,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "quiet".to_string(),
                }],
            ),
            (
                2,
                vec![ClientPane {
                    id: 20,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "busy".to_string(),
                }],
            ),
        ],
    );
    store.ensure_workspace("ws").backend_status = Some(backend_status::CONNECTED);

    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_attention(Some(AttentionSnapshot {
                blocked_count: 0,
                workspaces: vec![WorkspaceAttention {
                    workspace_id: "ws".to_string(),
                    name: "dev".to_string(),
                    transport: "local".to_string(),
                    ..WorkspaceAttention::new(
                        "ws",
                        vec![PaneAttention {
                            pane_id: 20,
                            status: PaneAttentionStatus::Working,
                            process_name: Some("cargo".to_string()),
                            last_line: "compiling".to_string(),
                            seq: 5,
                            ..PaneAttention::default()
                        }],
                    )
                }],
            }));
        })
    });
    let items = cx.update(|_, cx| view.update(cx, |app, _| app.tab_items()));
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "quiet");
    assert_eq!(items[0].activity, None, "没活动的 tab 不点亮");
    assert_eq!(items[1].title, "busy");
    assert_eq!(items[1].activity, Some(Indicator::Working));
    // tab 标题是「编号 + 两个空格 + 名字」：对照 `StatusBarTabTitle.display`。
    assert_eq!(items[1].label(), "2  busy");
}

/// 状态栏徽标只数主导色的条目，不是所有未读之和。
#[gpui::test]
fn status_badge_counts_the_dominant_colour(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_attention(Some(AttentionSnapshot {
                blocked_count: 0,
                workspaces: vec![WorkspaceAttention {
                    workspace_id: "local@@dev@tmux@dev".to_string(),
                    name: "dev".to_string(),
                    transport: "local".to_string(),
                    ..WorkspaceAttention::new(
                        "local@@dev@tmux@dev",
                        vec![
                            PaneAttention {
                                pane_id: 10,
                                status: PaneAttentionStatus::Blocked,
                                seq: 3,
                                ..PaneAttention::default()
                            },
                            PaneAttention {
                                pane_id: 11,
                                status: PaneAttentionStatus::Blocked,
                                seq: 4,
                                ..PaneAttention::default()
                            },
                        ],
                    )
                }],
            }));
        })
    });
    let attention = cx.update(|_, cx| view.update(cx, |app, _| app.status_attention()));
    assert_eq!(attention.count, 2, "两个 blocked 都是主导色");
    assert_eq!(attention.indicator, Some(Indicator::Blocked));
}

/// tab 栏位置来自 `[ui] tab_bar_position`，默认顶部。
#[gpui::test]
fn tab_bar_position_defaults_to_bottom(cx: &mut TestAppContext) {
    // 对照 macOS `ContentView.position = .bottom`。
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let position = cx.update(|_, cx| view.update(cx, |app, _| app.tab_bar_position()));
    assert_eq!(position, TabBarPosition::Bottom);
}

/// Cmd-P（QuickConnect）打开统一面板并停在 Workspaces。
#[gpui::test]
fn cmd_p_opens_the_panel_on_workspaces(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.panel_open())));
    cx.simulate_keystrokes("cmd-p");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.panel_open())));
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_tab())),
        PanelTab::Workspaces
    );
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title(), "dev · local");
}

/// Tab 循环三个 tab，Shift-Tab 反向；Escape 关闭。
#[gpui::test]
fn tab_cycles_panel_tabs_and_escape_closes(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.simulate_keystrokes("tab");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_tab())),
        PanelTab::Attention
    );
    cx.simulate_keystrokes("shift-tab");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_tab())),
        PanelTab::Workspaces
    );
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.panel_open())));
}

/// 面板打开时 Tab 不进终端：键先被面板吃掉。
#[gpui::test]
fn panel_swallows_tab_instead_of_sending_it(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("cmd-p");
    cx.simulate_keystrokes("tab");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::Input { .. })),
        "面板里的 Tab 不该写进 PTY：{commands:?}"
    );
}

/// Cmd-F 直接落在 Search tab，搜索命中按 query 过滤。
#[gpui::test]
fn cmd_f_opens_the_search_tab(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_search_snapshot(Some(
                SearchSnapshot::decode(
                    br#"{"ok":true,"hits":[
                        {"workspace_id":"local@@dev@tmux@dev","tab_id":1,"pane_id":10,"seq":4,"line":"cargo test ok"},
                        {"workspace_id":"local@@dev@tmux@dev","tab_id":1,"pane_id":11,"seq":5,"line":"vim notes"}
                    ]}"#,
                )
                .expect("snapshot"),
            ));
        })
    });
    cx.simulate_keystrokes("cmd-f");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_tab())),
        PanelTab::Search
    );
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert_eq!(rows.len(), 2);
}

/// Cmd-I 打开输入框，徽章显示目标去向。
#[gpui::test]
fn cmd_i_opens_the_composer_with_its_target(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.composer_open)));
    cx.simulate_keystrokes("cmd-i");
    let (open, badge, page_title) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.composer_open,
                app.composer_page()
                    .and_then(|page| page.target.as_ref().map(|target| target.badge())),
                app.composer_page().map(|page| page.title()),
            )
        })
    });
    assert!(open, "Cmd-I 要打开输入框");
    assert_eq!(badge, Some("Local"));
    assert_eq!(page_title.as_deref(), Some("dev #10"));
}

/// 打字只写草稿，不写 PTY；Esc 收起后草稿还在。
#[gpui::test]
fn typing_stays_in_the_draft_and_survives_closing(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("cmd-i");
    cx.simulate_keystrokes("h i");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.is_empty(),
        "输入框里的字不能直接进终端：{commands:?}"
    );
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("cmd-i");
    let text = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.composer_page()
                .map(|page| page.text.clone())
                .unwrap_or_default()
        })
    });
    assert_eq!(text, "hi", "重新打开要接上原草稿");
}

/// Cmd-Enter 发送一次并清空这一页；再按一次只报「没有内容」。
#[gpui::test]
fn cmd_enter_sends_the_draft_once(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("cmd-i");
    cx.simulate_keystrokes("e c h o");
    cx.simulate_keystrokes("cmd-enter");

    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let inputs: Vec<&ClientCommand> = commands
        .iter()
        .filter(|command| matches!(command, ClientCommand::Input { .. }))
        .collect();
    assert_eq!(inputs.len(), 1, "一次 Cmd-Enter 只写一次：{commands:?}");
    let ClientCommand::Input { data, pane_id, .. } = inputs[0] else {
        unreachable!()
    };
    assert_eq!(data, b"echo");
    assert_eq!(*pane_id, 10);

    let (cleared, status) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.composer_page().map(|page| page.text.clone()),
                app.composer_status.clone(),
            )
        })
    });
    assert_eq!(cleared.as_deref(), Some(""), "发送后这一页要清空");
    assert_eq!(status.as_deref(), Some("sent"));

    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("cmd-enter");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(commands.is_empty(), "空草稿不发任何字节：{commands:?}");
    let status = cx.update(|_, cx| view.update(cx, |app, _| app.composer_status.clone()));
    assert_eq!(status.as_deref(), Some("nothing to send"));
}

/// 每个 pane 一份草稿：切走再回来内容还在。
#[gpui::test]
fn composer_drafts_are_per_pane(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-i");
    cx.simulate_keystrokes("a b");
    cx.simulate_keystrokes("escape");

    // 切到另一个 workspace（pane 20），打开输入框应是空白页。
    cx.simulate_keystrokes("cmd-ctrl-2");
    cx.simulate_keystrokes("cmd-i");
    let second_text = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.composer_page()
                .map(|page| page.text.clone())
                .unwrap_or_default()
        })
    });
    assert_eq!(second_text, "");

    cx.simulate_keystrokes("c d");
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("cmd-ctrl-1");
    cx.simulate_keystrokes("cmd-i");
    let first_text = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.composer_page()
                .map(|page| page.text.clone())
                .unwrap_or_default()
        })
    });
    assert_eq!(first_text, "ab", "回到第一个 pane 草稿要还原");
}

fn settings_manifest() -> serde_json::Value {
    muxterm::config::ConfigDocument::manifest_json()
}

/// Cmd-, 打开设置；注入 Core 快照后侧栏由 manifest 驱动。
#[gpui::test]
fn cmd_comma_opens_settings_and_the_manifest_drives_the_sidebar(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-,");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)));

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "font": { "size": 13.0 } }),
                cx,
            )
        })
    });
    let (categories, selected) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.settings_categories()
                    .into_iter()
                    .map(|category| category.id)
                    .collect::<Vec<_>>(),
                app.settings_selected().map(|category| category.id),
            )
        })
    });
    assert!(categories.contains(&"appearance".to_string()));
    assert!(categories.contains(&"ui".to_string()));
    assert_eq!(selected.as_deref(), Some("appearance"), "默认选中第一条");
}

/// Apply 只提交改过的路径；再按一次没有 patch。
#[gpui::test]
fn settings_apply_queues_only_changed_paths(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({
                    "font": { "size": 13.0 },
                    "ui": { "tab_bar_position": "top" }
                }),
                cx,
            )
        })
    });
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())));

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.settings_set("/ui/tab_bar_position", serde_json::json!("bottom"), cx);
            app.settings_set("/font/size", serde_json::json!(15.0), cx);
        })
    });
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())));

    let applied = cx.update(|_, cx| view.update(cx, |app, cx| app.settings_apply(cx)));
    assert_eq!(applied, 2);
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let patches: Vec<(String, serde_json::Value)> = commands
        .iter()
        .filter_map(|command| match command {
            ClientCommand::ConfigApply { path, value } => Some((path.clone(), value.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        patches,
        vec![
            ("/font/size".to_string(), serde_json::json!(15.0)),
            (
                "/ui/tab_bar_position".to_string(),
                serde_json::json!("bottom")
            ),
        ],
        "顺序跟随 manifest，只报改动过的路径"
    );

    let second = cx.update(|_, cx| view.update(cx, |app, cx| app.settings_apply(cx)));
    assert_eq!(second, 0, "Apply 之后 baseline 已更新");
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())));
}

/// 丢弃修改回到打开时的值，选择与搜索词保留。
#[gpui::test]
fn settings_discard_reverts_but_keeps_the_selection(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "ui": { "tab_bar_position": "top" } }),
                cx,
            );
            app.settings_select("ui", cx);
            app.settings_set("/ui/tab_bar_position", serde_json::json!("bottom"), cx);
        })
    });
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())));
    cx.update(|_, cx| view.update(cx, |app, cx| app.settings_discard(cx)));
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())));
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_value("/ui/tab_bar_position"))),
        Some(serde_json::json!("top"))
    );
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app
            .settings_selected()
            .map(|category| category.id))),
        Some("ui".to_string())
    );
}

/// 搜索过滤侧栏但不丢草稿；选中项被过滤掉时切到第一条可见分类。
#[gpui::test]
fn settings_search_filters_without_losing_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "font": { "size": 13.0 } }),
                cx,
            );
            app.settings_set("/font/size", serde_json::json!(17.0), cx);
            app.settings_set_query("ssh", cx);
        })
    });
    let (categories, selected, value) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.settings_categories()
                    .into_iter()
                    .map(|category| category.id)
                    .collect::<Vec<_>>(),
                app.settings_selected().map(|category| category.id),
                app.settings_value("/font/size"),
            )
        })
    });
    assert_eq!(categories, vec!["ssh".to_string()]);
    assert_eq!(selected.as_deref(), Some("ssh"), "选中项不可见时切到第一条");
    assert_eq!(value, Some(serde_json::json!(17.0)), "草稿不因搜索丢");
}

/// 新建项目：路径自动带出 name，保存后交给 Catalog 打开。
#[gpui::test]
fn new_project_saves_through_the_catalog(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.open_target_editor(None, cx)));
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.target_editor().is_some())));

    // 斜杠不是可打印键名，路径直接设；名字用真实按键打进当前焦点字段。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.update_target_editor(|editor| editor.set_path("/srv"), cx)
        })
    });
    // set_path 已经自动把 name 带成 srv；Tab 之后的按键只往 Name 追加。
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("x");
    let (path, name) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let editor = app.target_editor().expect("editor");
            (editor.path_text().to_string(), editor.name().to_string())
        })
    });
    assert_eq!(path, "/srv", "打字不能改路径字段");
    assert_eq!(name, "srvx", "按键写进当前焦点字段");

    cx.simulate_keystrokes("enter");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let opens: Vec<(String, String, String, Option<String>)> = commands
        .iter()
        .filter_map(|command| match command {
            ClientCommand::OpenTarget { target, .. } => Some((
                target.name.clone(),
                target.runtime.clone(),
                target.path.clone(),
                target.target.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(opens.len(), 1, "保存只发一次 OpenTarget：{commands:?}");
    assert_eq!(opens[0].0, "srvx");
    assert_eq!(opens[0].1, "tmux");
    assert_eq!(opens[0].3, None, "本地传输没有 target");
    assert_eq!(opens[0].2, "/srv", "路径原样交给 Catalog");
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.target_editor().is_none())),
        "保存后窗口关闭"
    );
}

/// SSH 传输没填 Host 时给出错误，不发命令。
#[gpui::test]
fn ssh_project_without_a_host_shows_an_error(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_target_editor(None, cx);
            app.update_target_editor(
                |editor| {
                    editor.select_transport(TargetTransport::Ssh {
                        name: String::new(),
                    })
                },
                cx,
            );
            app.take_commands_for_tests();
        })
    });
    cx.simulate_keystrokes("enter");
    let (error, still_open, commands) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.target_editor_error.clone(),
                app.target_editor().is_some(),
                app.take_commands_for_tests(),
            )
        })
    });
    assert_eq!(
        error.as_deref(),
        Some("Select or enter an SSH Host before saving.")
    );
    assert!(still_open, "校验失败不能关窗口");
    assert!(commands.is_empty(), "校验失败不发命令：{commands:?}");
}

// ---------------------------------------------------------------------------
// Cmd-Shift-P 命令面板
// ---------------------------------------------------------------------------

/// Cmd-Shift-P 打开命令面板；再按一次 Esc 收起。
///
/// 对照 macOS `AppE2ETests`：`openCommandPalette` → `commandPalette.testIsPresented()`，
/// 取消走 `dismiss()`。
#[gpui::test]
fn cmd_shift_p_opens_the_command_palette(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_open())),
        "Cmd-Shift-P 必须打开命令面板"
    );
    cx.simulate_keystrokes("escape");
    assert!(
        !cx.update(|_, cx| view.update(cx, |app, _| app.palette_open())),
        "Esc 必须收起命令面板"
    );
}

/// 面板目录带 tab 切换项，过滤是整串包含。
#[gpui::test]
fn palette_lists_tabs_and_filters_by_query(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.open_palette(cx)));
    let titles: Vec<String> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_items()
                .into_iter()
                .map(|item| item.title)
                .collect()
        })
    });
    assert!(titles.iter().any(|title| title == "Switch to Tab 1"));
    assert!(titles.iter().any(|title| title == "New Tab"));

    cx.update(|_, cx| view.update(cx, |app, cx| app.palette_push("new tab", cx)));
    let visible: Vec<String> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_visible()
                .into_iter()
                .map(|item| item.title)
                .collect()
        })
    });
    assert_eq!(visible, vec!["New Tab".to_string()]);
}

/// 回车执行选中项：New Tab 走 `ClientTask::NewTab`，面板同时收起。
#[gpui::test]
fn palette_enter_runs_the_selected_command(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    // 用查询把目标收窄到唯一一条；空格的按键模拟不通用，直接写查询串。
    cx.update(|_, cx| view.update(cx, |app, cx| app.palette_push("new tab", cx)));
    let visible = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_visible()
                .into_iter()
                .map(|item| item.title)
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(visible, vec!["New Tab".to_string()]);
    cx.simulate_keystrokes("enter");
    let (open, commands) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.palette_open(), app.take_commands_for_tests())
        })
    });
    assert!(!open, "执行后面板必须收起");
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::NewTab,
                ..
            }
        )),
        "New Tab 必须排进命令队列，实际: {commands:?}"
    );
}

/// 没有命中时回车只关面板，不执行任何命令。
#[gpui::test]
fn palette_enter_without_matches_only_dismisses(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.update(|_, cx| view.update(cx, |app, cx| app.palette_push("zzzz", cx)));
    cx.simulate_keystrokes("enter");
    let (open, commands) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.palette_open(), app.take_commands_for_tests())
        })
    });
    let commands: Vec<ClientCommand> = commands
        .into_iter()
        .filter(|command| !matches!(command, ClientCommand::Resize { .. }))
        .collect();
    assert!(!open);
    assert!(commands.is_empty(), "没有命中时不得执行命令: {commands:?}");
}

/// 面板打开时上下键移动选中项，不会漏进终端。
#[gpui::test]
fn palette_arrow_keys_move_selection(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("down");
    let selected = cx.update(|_, cx| view.update(cx, |app, _| app.palette_selected()));
    assert_eq!(selected, 2);
    cx.simulate_keystrokes("up");
    let selected = cx.update(|_, cx| view.update(cx, |app, _| app.palette_selected()));
    assert_eq!(selected, 1);
}

/// 无头默认主题是浅色、Menlo 18 —— 与 macOS `MuxtermTerminalColors/MuxtermTerminalFont` 一致。
#[gpui::test]
fn headless_theme_defaults_to_light_menlo_18(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let (dark, family, size, bg) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let theme = app.theme();
            (
                theme.is_dark,
                theme.font_family.to_string(),
                theme.font_size,
                theme.bg,
            )
        })
    });
    assert!(!dark, "默认必须是浅色主题");
    assert_eq!(family, "Menlo");
    assert_eq!(size, 18.0);
    assert_eq!(bg, gpui::rgb(0xffffff));
}

/// 无头布局用渲染期量出来的格子尺寸：宽高都必须为正，且高 > 宽。
#[gpui::test]
fn measured_cell_is_tall_enough_for_the_font(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let (width, height) = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    assert!(width > 1.0 && height > 1.0);
    assert!(
        height > width,
        "等宽字体的行高必须大于字宽，实际 ({width}, {height})"
    );
}

// ---------------------------------------------------------------------------
// 布局：tab 栏 / 状态栏 / 侧栏的真实像素高度
// ---------------------------------------------------------------------------

/// tab 栏 24pt、状态栏 18pt、侧栏 240pt —— 与 macOS `FlatChrome` / split 一致。
#[gpui::test]
fn chrome_metrics_match_the_swift_layout(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::sidebar::SIDEBAR_WIDTH;
    use muxterm::frontend::gpui::chrome::status_bar::STATUS_BAR_HEIGHT;
    use muxterm::frontend::gpui::chrome::tab_bar::TAB_BAR_HEIGHT;
    // 对照 `FlatChrome.tabBarHeight`：一条 bar 同时承担 tab 与状态。
    assert_eq!(TAB_BAR_HEIGHT, 24.0);
    assert_eq!(STATUS_BAR_HEIGHT, 24.0);
    // 对照 `MainWindow` 里 `splitView.setPosition(240, ofDividerAt: 0)`。
    assert_eq!(SIDEBAR_WIDTH, 240.0);
}

/// 真实布局：侧栏 240pt、tab 栏 24pt、状态栏 18pt。
#[gpui::test]
fn chrome_bounds_match_the_swift_metrics(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let sidebar = cx.debug_bounds("sidebar").expect("侧栏必须渲染");
    let status_bar = cx.debug_bounds("status-bar").expect("状态栏必须渲染");
    assert_eq!(f32::from(sidebar.size.width), 240.0);
    // 一条 24pt 的 bar 同时装 tab 与状态；不再有第二条 18pt。
    assert_eq!(f32::from(status_bar.size.height), 24.0);
    let window_height = cx.update(|window, _| f32::from(window.viewport_size().height));
    assert!((f32::from(status_bar.origin.y) + 24.0 - window_height).abs() < 1.0);
    let _ = view;
}

/// 命令面板浮层：600×360，居中偏上，盖住 pane 区域。
#[gpui::test]
fn palette_renders_a_600x360_overlay(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.open_palette(cx)));
    cx.run_until_parked();
    let overlay = cx.debug_bounds("palette-overlay").expect("浮层必须渲染");
    let panel = cx.debug_bounds("palette-panel").expect("面板必须渲染");
    assert_eq!(f32::from(panel.size.width), 600.0);
    assert_eq!(f32::from(panel.size.height), 360.0);
    let window_width = cx.update(|window, _| f32::from(window.viewport_size().width));
    // 浮层只盖 pane 区域（不含侧栏），在 pane 区域内水平居中。
    let main_width = window_width - 240.0;
    let left = f32::from(panel.origin.x) - 240.0;
    assert!(
        (left - (main_width - 600.0) / 2.0).abs() <= 1.0,
        "panel left={left} main_width={main_width}"
    );
    assert!((f32::from(overlay.size.width) - main_width).abs() <= 1.0);
}
