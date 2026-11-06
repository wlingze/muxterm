//! 无头 chrome 验收：挂载真实 `GpuiApp`，点行、按快捷键、断言状态。
//!
//! 对应 macOS `MuxtermAppUITests` 里「点侧栏切工作区 / Cmd-Ctrl-数字 / Cmd-B」
//! 这几条路径；无头窗口没有真实 AppKit 控件，所以断言落在状态与命令队列上。

#![cfg(feature = "gpui")]

use gpui::{px, size, TestAppContext};
use muxterm::frontend::command_queue::ClientCommand;
use muxterm::frontend::gpui::chrome::palette_view::PaletteAction;
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
    // 对照截图：固定聚合（Shells/Agents）+ Open 在前，然后是已打开的 workspace。
    let titles: Vec<String> = rows.iter().map(|row| row.title()).collect();
    assert_eq!(
        titles,
        vec!["Shells", "Agents", "Open", "dev · local", "ops · local"]
    );
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
    // 用动作而不是标题断言：目录文案会随语言变，动作不会。
    let actions: Vec<PaletteAction> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_items()
                .into_iter()
                .map(|item| item.action)
                .collect()
        })
    });
    assert!(actions.contains(&PaletteAction::KeyAction(
        muxterm::frontend::gpui::model::shortcuts::KeyAction::SwitchTab(1)
    )));
    assert!(actions.contains(&PaletteAction::KeyAction(
        muxterm::frontend::gpui::model::shortcuts::KeyAction::NewTab
    )));

    cx.update(|_, cx| view.update(cx, |app, cx| app.palette_push("new tab", cx)));
    let visible: Vec<PaletteAction> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_visible()
                .into_iter()
                .map(|item| item.action)
                .collect()
        })
    });
    assert_eq!(
        visible,
        vec![PaletteAction::KeyAction(
            muxterm::frontend::gpui::model::shortcuts::KeyAction::NewTab
        )]
    );
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
                .map(|item| item.action)
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(
        visible,
        vec![PaletteAction::KeyAction(
            muxterm::frontend::gpui::model::shortcuts::KeyAction::NewTab
        )]
    );
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
fn chrome_metrics_match_the_swift_layout(_cx: &mut TestAppContext) {
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

/// Core 的 `PaneHistory` 回填要进 Surface 的历史，滚轮才滚得出 attach 前的输出。
#[gpui::test]
fn pane_history_events_land_in_the_scrollback(cx: &mut TestAppContext) {
    let mut store = store_with_two_workspaces();
    store.push_render_event(
        "local@@dev@tmux@dev",
        muxterm::frontend::utils::corebridge::ClientEvent {
            type_: muxterm::protocol::ffi::types::STATE_PANE_HISTORY,
            pane_id: 10,
            tab_id: 1,
            window_id: 0,
            data: b"older-one\nolder-two".to_vec(),
            name: String::new(),
        },
    );
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            assert!(app.drain_render_events(), "历史事件必须让视图变脏");
        })
    });
    let depth = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.history_depth_for_tests("local@@dev@tmux@dev", 10)
        })
    });
    assert_eq!(depth, 2);
}

/// 滚轮入口要往上翻历史，并且只在有历史时动。
///
/// 命中测试由 `gpui_surface_events.rs` 的 PaneProbe 覆盖；这里验证
/// `GpuiApp` 的状态与视口接线（pane 元素只在真实布局投影存在时才渲染）。
#[gpui::test]
fn wheel_over_a_pane_scrolls_its_history(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::state::WheelInput;
    let mut store = store_with_two_workspaces();
    store.push_render_event(
        "local@@dev@tmux@dev",
        muxterm::frontend::utils::corebridge::ClientEvent {
            type_: muxterm::protocol::ffi::types::STATE_PANE_HISTORY,
            pane_id: 10,
            tab_id: 1,
            window_id: 0,
            data: b"h1\nh2\nh3".to_vec(),
            name: String::new(),
        },
    );
    let (view, cx) = mount(store, cx);
    let offset = cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            assert!(app.drain_render_events(), "历史事件必须让视图变脏");
            assert_eq!(app.history_offset_for_tests("local@@dev@tmux@dev", 10), 0);
            app.handle_wheel(
                WheelInput {
                    pane_id: 10,
                    delta_y: 2.0,
                    precise: false,
                    shift: false,
                    col: 0,
                    row: 0,
                },
                cx,
            );
            app.history_offset_for_tests("local@@dev@tmux@dev", 10)
        })
    });
    assert_eq!(offset, 2, "上滚两格要进入历史");
}

// ---------------------------------------------------------------------------
// Cmd-P：现有 runtime（Core discovery）只在这里出现，激活走 Catalog attach
// ---------------------------------------------------------------------------

fn existing_runtime(
    name: &str,
    windows: u32,
) -> muxterm::frontend::gpui::model::existing::ExistingRuntime {
    use muxterm::frontend::gpui::model::existing::ExistingRuntime;
    ExistingRuntime {
        name: name.to_string(),
        runtime: "tmux".to_string(),
        transport: "local".to_string(),
        target: String::new(),
        session: name.to_string(),
        socket: None,
        windows,
        attached: false,
        workspace_id: None,
        path: String::new(),
    }
}

/// 面板的 Workspaces tab 同时列已打开的 workspace 和 discovery 到的现有 runtime。
#[gpui::test]
fn panel_lists_existing_runtimes_after_the_workspaces(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_existing_for_tests(vec![
                existing_runtime("alpha", 2),
                existing_runtime("beta", 1),
            ]);
            app.show_panel(
                muxterm::frontend::gpui::model::panel::PanelTab::Workspaces,
                cx,
            );
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    let workspaces = rows
        .iter()
        .filter(|row| matches!(row, PanelRow::Workspace { .. }))
        .count();
    let existing: Vec<String> = rows
        .iter()
        .filter_map(|row| match row {
            PanelRow::Existing { title, detail, .. } => Some(format!("{title}|{detail}")),
            _ => None,
        })
        .collect();
    assert_eq!(workspaces, 2, "已打开的 workspace 仍在前");
    assert_eq!(
        existing,
        vec![
            "alpha|tmux @ local · 2 windows".to_string(),
            "beta|tmux @ local · 1 windows".to_string(),
        ]
    );
}

/// Enter 在现有 runtime 行上只发 attach（AttachOnly），不新建 session。
#[gpui::test]
fn panel_enter_on_existing_attaches_through_the_catalog(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::utils::corebridge::ClientOpenIntent;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let command = cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_existing_for_tests(vec![existing_runtime("alpha", 2)]);
            app.show_panel(
                muxterm::frontend::gpui::model::panel::PanelTab::Workspaces,
                cx,
            );
            // 现有 runtime 排在所有 workspace 之后。
            let last = app.panel_rows().len() - 1;
            app.move_panel_selection(last as i32, cx);
            assert_eq!(app.panel_selected(), last);
            app.activate_panel_row(cx);
            app.panel_open()
        })
    });
    assert!(command, "异步 attach 完成前保留面板，失败时显示错误");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let request = commands
        .iter()
        .find_map(|command| match command {
            ClientCommand::OpenCandidate { request } => Some(request),
            _ => None,
        })
        .expect("必须发完整 Candidate 身份");
    assert_eq!(request.intent, ClientOpenIntent::AttachOnly);
    let muxterm::frontend::utils::corebridge::ClientCandidateRef::Existing { identity } =
        &request.candidate
    else {
        panic!("Existing ref");
    };
    assert_eq!(identity.session.as_deref(), Some("alpha"));
    assert_eq!(identity.runtime_id, "tmux");
}

/// 查询串收窄现有 runtime 行，并且不把按键漏进终端。
#[gpui::test]
fn panel_query_filters_existing_and_swallows_typing(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_existing_for_tests(vec![
                existing_runtime("alpha", 1),
                existing_runtime("beta", 1),
            ]);
            app.show_panel(
                muxterm::frontend::gpui::model::panel::PanelTab::Workspaces,
                cx,
            );
            app.panel_push_query("bet", cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    let names: Vec<String> = rows
        .iter()
        .filter_map(|row| match row {
            PanelRow::Existing { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["beta".to_string()], "查询只留下匹配行");
}

/// 原生菜单栏的点击 → `KeyAction` → `run_action` 这条链路。
///
/// 平台层只负责把 NSMenuItem 翻成 `KeyAction`；这里验证收到动作后
/// gpui 真的执行了（新 tab 会发 Core task）。
#[gpui::test]
fn menu_actions_run_through_the_action_table(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::gpui::model::shortcuts::KeyAction;
    use muxterm::frontend::utils::corebridge::ClientTask;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let (tx, rx) = futures::channel::mpsc::unbounded();
    cx.update(|_, cx| view.update(cx, |app, cx| app.watch_menu(rx, cx)));
    tx.unbounded_send(KeyAction::NewTab).expect("菜单动作");
    cx.run_until_parked();
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::NewTab,
                ..
            }
        )),
        "菜单里的 New Tab 必须发出 ClientTask::NewTab"
    );
}

/// 面板根列表的行动作是 Open（Swift `panelOpen`），不是 "New project"。
#[gpui::test]
fn panel_primary_action_is_open(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel::PANEL_OPEN_LABEL;
    assert_eq!(PANEL_OPEN_LABEL, "+ Open");
    assert!(!PANEL_OPEN_LABEL.contains("New project"));

    // 回归上一轮错误：Open 必须进入机器选择，不得再打开 TargetEditor 表单。
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.begin_panel_open(cx);
            assert!(matches!(
                app.panel_stage(),
                muxterm::frontend::gpui::model::workspace_picker::PickerStage::Hosts
            ));
            assert!(app.target_editor().is_none());
        })
    });
}

/// 命令面板打开时独占按键：查询串收字，绝不漏进 PTY。
#[gpui::test]
fn palette_owns_keystrokes_and_never_reaches_the_shell(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.palette_open())));
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("i n f o");
    let (query, serial) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.palette_query_for_tests(), app.take_commands_for_tests())
        })
    });
    assert_eq!(query, "info", "按键必须进命令面板的查询串");
    assert!(
        !serial
            .iter()
            .any(|command| matches!(command, ClientCommand::Input { .. })),
        "命令面板打字不能漏进终端：{serial:?}"
    );
}

/// 光标在提示符后面的空格上：不能因为「裁剪尾部空白」把光标一起裁掉。
#[gpui::test]
fn cursor_is_rendered_on_a_trailing_blank_cell(cx: &mut TestAppContext) {
    let mut store = store_with_two_workspaces();
    // 有 layout 才会渲染 pane；`replace_topology` 只给拓扑。
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![(
            1,
            muxterm::frontend::utils::corebridge::ClientLayout::Leaf { pane_id: 10 },
        )],
    );
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            // 一行 `❯ ` + 光标停在最后那个空格（shell 提示符的常态）。
            app.feed_pane_for_tests("local@@dev@tmux@dev", 10, 80, 24, "❯ ".as_bytes(), cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("pane-cursor").is_some(),
        "光标必须画出来，即使它落在一个尾部空白格子上（回归：之前尾部空白裁剪会把光标剪掉）"
    );
}

/// Muxterm Info：命令面板能打开它，浮层有版本/commit/日志路径，Esc 关掉，
/// 打开期间打字不漏进终端。
#[gpui::test]
fn muxterm_info_opens_from_the_palette_and_owns_keystrokes(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientBuildInfo;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    // 走真实入口：Cmd-Shift-P → 输入 info → 回车。
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_keystrokes("i n f o");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let opened = cx.update(|_, cx| view.update(cx, |app, _| app.info().is_some()));
    assert!(opened, "Muxterm Info 必须能打开（之前是灰掉的占位项）");
    assert!(
        cx.debug_bounds("info-panel").is_some(),
        "浮层必须真的画出来"
    );

    // 打开期间打字：不进终端。
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("x y z");
    let serial = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !serial
            .iter()
            .any(|command| matches!(command, ClientCommand::Input { .. })),
        "Info 浮层打开时不能往 PTY 写字符：{serial:?}"
    );

    cx.simulate_keystrokes("escape");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.info().is_none())));

    // 日志没开时仍然给出目录，用户不用猜路径。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_info_for_tests(ClientBuildInfo {
                version: "0.1.0".into(),
                commit: "abc1234".into(),
                log_file: None,
            });
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("info-panel").is_some());
}

// ---------------------------------------------------------------------------
// 命令面板：不再有「列出来但点不动」的条目
// ---------------------------------------------------------------------------

/// 打开命令面板，按动作选中一条命令，再用真实回车执行。
///
/// 按「动作」而不是标题定位：目录文案会随语言变，动作是稳定的。
fn run_palette_action(
    view: &gpui::Entity<GpuiApp>,
    cx: &mut gpui::VisualTestContext,
    action: muxterm::frontend::gpui::chrome::palette_view::PaletteAction,
) {
    cx.simulate_keystrokes("cmd-shift-p");
    let found =
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_select_action_for_tests(action)));
    assert!(found, "命令目录里找不到 {action:?}");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

/// `Local` / `SSH` 直接把面板导航到会话列表 / 机器列表。
#[gpui::test]
fn palette_local_and_ssh_open_the_picker_on_the_right_stage(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::model::workspace_picker::PickerStage;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    run_palette_action(&view, cx, PaletteAction::OpenLocal);
    let stage = cx.update(|_, cx| view.update(cx, |app, _| app.panel_stage().clone()));
    assert!(
        matches!(
            stage,
            PickerStage::Sessions(muxterm::frontend::gpui::model::target::TargetTransport::Local)
        ),
        "Local 要直接进本机会话列表，实际 {stage:?}"
    );
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.panel_open())));

    let (view, cx) = mount(store_with_two_workspaces(), cx);
    run_palette_action(&view, cx, PaletteAction::OpenSsh);
    let stage = cx.update(|_, cx| view.update(cx, |app, _| app.panel_stage().clone()));
    assert!(matches!(stage, PickerStage::Hosts), "SSH 要先选机器");
}

/// Rename Tab…：浮层带出当前名字，回车发 RenameTab；Esc 什么都不发。
#[gpui::test]
fn rename_tab_prompt_seeds_the_name_and_sends_the_task(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    run_palette_action(&view, cx, PaletteAction::RenameTab);
    let seeded = cx
        .update(|_, cx| view.update(cx, |app, _| app.prompt().map(|prompt| prompt.value.clone())));
    assert_eq!(seeded.as_deref(), Some("dev"), "浮层要带出当前 tab 名字");
    assert!(
        cx.debug_bounds("prompt-panel").is_some(),
        "浮层要真的画出来"
    );

    // 清掉旧名字再输入新名字：Cmd-A 在浮层里不做全选，这里用退格。
    for _ in 0..8 {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_keystrokes("l o g s");
    cx.simulate_keystrokes("enter");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::RenameTab { name, .. } if name == "logs"
        )),
        "回车必须发 RenameTab：{commands:?}"
    );
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.prompt().is_none())));

    // Esc 路径：不发命令。
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    run_palette_action(&view, cx, PaletteAction::RenameWorkspace);
    cx.simulate_keystrokes("escape");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(!commands
        .iter()
        .any(|command| matches!(command, ClientCommand::RenameWorkspace { .. })));
}

/// Rename Workspace…：空名字留在浮层，不把空名字发给 Core。
#[gpui::test]
fn rename_workspace_rejects_an_empty_name(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    run_palette_action(&view, cx, PaletteAction::RenameWorkspace);
    for _ in 0..8 {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_keystrokes("enter");
    let (open, error) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.prompt().is_some(),
                app.prompt().and_then(|prompt| prompt.error.clone()),
            )
        })
    });
    assert!(open, "空名字不能提交");
    assert!(error.is_some(), "要给出错误提示");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(!commands
        .iter()
        .any(|command| matches!(command, ClientCommand::RenameWorkspace { .. })));
}

/// Move Tab Left/Right：按相邻 tab 发 Core 的 move_tab。
#[gpui::test]
fn move_tab_sends_the_neighbour_target(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::utils::corebridge::{ClientLayout, ClientPane, ClientTab};
    let mut store = store_with_two_workspaces();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![
            ClientTab {
                id: 1,
                name: "one".to_string(),
                is_active: true,
            },
            ClientTab {
                id: 2,
                name: "two".to_string(),
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
                    title: "one".to_string(),
                }],
            ),
            (
                2,
                vec![ClientPane {
                    id: 11,
                    cols: 80,
                    rows: 24,
                    is_active: false,
                    title: "two".to_string(),
                }],
            ),
        ],
    );
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![(1, ClientLayout::Leaf { pane_id: 10 })],
    );
    let (view, cx) = mount(store, cx);

    run_palette_action(&view, cx, PaletteAction::MoveTabRight);
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::MoveTab {
                from: 1,
                target: 2,
                before: false,
                ..
            }
        )),
        "右移要发给右侧邻居：{commands:?}"
    );

    // 已经在最左：左移不再发命令。
    run_palette_action(&view, cx, PaletteAction::MoveTabLeft);
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(!commands
        .iter()
        .any(|command| matches!(command, ClientCommand::MoveTab { .. })));
}

/// Theme：切换写 `[theme] name`，方向由当前底色决定。
#[gpui::test]
fn palette_theme_toggle_persists_the_opposite_theme(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let dark = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let bg = app.theme().bg;
            0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b < 0.5
        })
    });
    run_palette_action(&view, cx, PaletteAction::ToggleTheme);
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let value = commands
        .iter()
        .find_map(|command| match command {
            ClientCommand::ConfigApply { path, value } if path == "theme.name" => {
                Some(value.as_str().unwrap_or_default().to_string())
            }
            _ => None,
        })
        .expect("主题切换必须写配置");
    assert_eq!(value, if dark { "white" } else { "black" });
}

/// Status Bar Mode：写 `[statusbar] mode`，tmux 模式显示 tmux 自己的左右段。
#[gpui::test]
fn status_bar_mode_toggles_and_tmux_mode_shows_tmux_segments(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::gpui::model::status_bar::StatusBarSnapshot;
    use muxterm::frontend::gpui::state::StatusBarMode;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_status_snapshot_for_tests(StatusBarSnapshot {
                enabled: true,
                left: "tmux-left".into(),
                right: "tmux-right".into(),
                ..StatusBarSnapshot::default()
            });
            app.set_status_bar_mode_for_tests(StatusBarMode::Tmux);
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tmux-status-left").is_some(),
        "tmux 模式要画出 tmux 的 status-left"
    );

    run_palette_action(&view, cx, PaletteAction::ToggleStatusBarMode);
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let value = commands
        .iter()
        .find_map(|command| match command {
            ClientCommand::ConfigApply { path, value } if path == "statusbar.mode" => {
                Some(value.as_str().unwrap_or_default().to_string())
            }
            _ => None,
        })
        .expect("必须写 statusbar.mode");
    assert_eq!(value, "theme");
    cx.run_until_parked();
    let (mode, segments) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.status_bar_mode(), app.tmux_status_segments())
        })
    });
    assert_eq!(mode, StatusBarMode::Theme);
    assert!(
        segments.is_none(),
        "theme 模式不再取 tmux 段（渲染由它门控）"
    );
}

/// Language：二级列表选中文后立刻改文案并写 `[ui] language`，Esc 只退回目录。
#[gpui::test]
fn language_menu_switches_the_catalog_and_persists(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::utils::i18n::Language;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    assert!(cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_select_action_for_tests(PaletteAction::Languages)
        })
    }));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.palette_menu().is_some())));

    // Esc 先退回目录，面板不关。
    cx.simulate_keystrokes("escape");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.palette_open())));
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.palette_menu().is_none())));

    // 再进一次语言子菜单（Esc 只退回目录），选中文。
    let reopened = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_select_action_for_tests(PaletteAction::Languages)
        })
    });
    assert!(reopened, "目录里必须有语言项");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let selected = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_select_action_for_tests(PaletteAction::SetLanguage(
                muxterm::frontend::utils::i18n::Language::SimplifiedChinese,
            ))
        })
    });
    assert!(selected, "语言子菜单必须有中文选项");
    cx.simulate_keystrokes("enter");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::ConfigApply { path, value }
                if path == "ui.language" && value.as_str() == Some("zh-CN")
        )),
        "语言选择必须持久化：{commands:?}"
    );
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.language())),
        Language::SimplifiedChinese
    );
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_items()[0].title.clone())),
        "本地",
        "目录文案要立刻变成中文"
    );
    // 收尾：把全局语言恢复成英文，别影响其它测试。
    cx.update(|_, _| muxterm::frontend::utils::i18n::set_language(Language::English));
}

// ---------------------------------------------------------------------------
// 快捷键与焦点：Cmd-D/Cmd-S 分屏、Cmd-[ / Cmd-]、切 tab / 新建 pane 后焦点
// ---------------------------------------------------------------------------

fn two_tab_store() -> ViewStore {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let mut store = store_with_two_workspaces();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![
            ClientTab {
                id: 1,
                name: "one".to_string(),
                is_active: true,
            },
            ClientTab {
                id: 2,
                name: "two".to_string(),
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
                    title: "one".to_string(),
                }],
            ),
            (
                2,
                vec![ClientPane {
                    id: 11,
                    cols: 80,
                    rows: 24,
                    is_active: false,
                    title: "two".to_string(),
                }],
            ),
        ],
    );
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![
            (1, ClientLayout::Leaf { pane_id: 10 }),
            (2, ClientLayout::Leaf { pane_id: 11 }),
        ],
    );
    store
}

/// Cmd-D 左右分屏、Cmd-S 上下分屏（对照 `KeyBindingsTests`）。
#[gpui::test]
fn cmd_d_and_cmd_s_split_the_focused_pane(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.simulate_keystrokes("cmd-d");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SplitPane {
                    pane_id: 10,
                    horizontal: true
                },
                ..
            }
        )),
        "Cmd-D 必须左右分屏：{commands:?}"
    );

    cx.simulate_keystrokes("cmd-s");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SplitPane {
                    pane_id: 10,
                    horizontal: false
                },
                ..
            }
        )),
        "Cmd-S 必须上下分屏：{commands:?}"
    );
}

/// Cmd-[ / Cmd-]：上一个 / 下一个 pane（对照 `PaneNavigationTests`）。
#[gpui::test]
fn cmd_brackets_walk_the_panes(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.simulate_keystrokes("cmd-]");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::NextPane,
                ..
            }
        )),
        "Cmd-] 必须是下一个 pane：{commands:?}"
    );
    cx.simulate_keystrokes("cmd-[");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::PreviousPane,
                ..
            }
        )),
        "Cmd-[ 必须是上一个 pane：{commands:?}"
    );
}

/// 切 tab 之后焦点必须跟着走：光标要画在新 tab 的活动 pane 上。
#[gpui::test]
fn switching_tab_moves_the_focus_and_the_cursor(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests("local@@dev@tmux@dev", 10, 80, 24, b"one", cx);
            app.feed_pane_for_tests("local@@dev@tmux@dev", 11, 80, 24, b"two", cx);
        })
    });
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane())),
        Some(10),
        "起始焦点在 tab 1"
    );

    // Core 回包：tab 2 变成 active，它的 pane 11 也是 active。
    let mut store = two_tab_store();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![
            ClientTab {
                id: 1,
                name: "one".to_string(),
                is_active: false,
            },
            ClientTab {
                id: 2,
                name: "two".to_string(),
                is_active: true,
            },
        ],
        vec![
            (
                1,
                vec![ClientPane {
                    id: 10,
                    cols: 80,
                    rows: 24,
                    is_active: false,
                    title: "one".to_string(),
                }],
            ),
            (
                2,
                vec![ClientPane {
                    id: 11,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "two".to_string(),
                }],
            ),
        ],
    );
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![
            (1, ClientLayout::Leaf { pane_id: 10 }),
            (2, ClientLayout::Leaf { pane_id: 11 }),
        ],
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.replace_store_for_tests(store);
            app.pump_once(cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane())),
        Some(11),
        "切 tab 后焦点必须在 tab 2 的 pane 上"
    );
    assert!(
        cx.debug_bounds("pane-cursor").is_some(),
        "焦点 pane 变了，光标必须画出来"
    );
}

/// 分屏后新 pane 要接管焦点（Core 回的拓扑里新 pane 是 active）。
#[gpui::test]
fn a_new_pane_takes_the_focus(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    let mut store = two_tab_store();
    // Core 完成分屏：tab 1 变成两个 pane，新的 12 是 active。
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![ClientTab {
            id: 1,
            name: "one".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![
                ClientPane {
                    id: 10,
                    cols: 40,
                    rows: 24,
                    is_active: false,
                    title: "one".to_string(),
                },
                ClientPane {
                    id: 12,
                    cols: 40,
                    rows: 24,
                    is_active: true,
                    title: "two".to_string(),
                },
            ],
        )],
    );
    // 分屏后的布局：两个叶子并排，新的在右边。
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![(
            1,
            muxterm::frontend::utils::corebridge::ClientLayout::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(muxterm::frontend::utils::corebridge::ClientLayout::Leaf {
                    pane_id: 10,
                }),
                second: Box::new(muxterm::frontend::utils::corebridge::ClientLayout::Leaf {
                    pane_id: 12,
                }),
            },
        )],
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.replace_store_for_tests(store);
            app.feed_pane_for_tests("local@@dev@tmux@dev", 12, 40, 24, b"new", cx);
            app.pump_once(cx);
        })
    });
    cx.run_until_parked();
    let focused = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
    assert_eq!(focused, Some(12), "新建的 pane 必须是焦点");
    assert!(cx.debug_bounds("pane-cursor").is_some());
}
