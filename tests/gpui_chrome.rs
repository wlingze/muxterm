//! 无头 chrome 验收：挂载真实 `GpuiApp`，点行、按快捷键、断言状态。
//!
//! 对应 macOS `MuxtermAppUITests` 里「点侧栏切工作区 / Cmd-Ctrl-数字 / Cmd-B」
//! 这几条路径；无头窗口没有真实 AppKit 控件，所以断言落在状态与命令队列上。

#![cfg(feature = "gpui")]

use gpui::{px, size, TestAppContext};
#[path = "support/gpui_settings.rs"]
mod settings_regressions;
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
    // 对照 Swift `sidebarItems()`：两个固定聚合槽永远在最前面。
    assert_eq!(workspaces.rows.len(), 4, "两个聚合槽 + 两个工作区");
    assert_eq!(workspaces.rows[0].title, "Shells");
    assert_eq!(workspaces.rows[0].shortcut.as_deref(), Some("S"));
    assert_eq!(workspaces.rows[0].trailing, None, "聚合槽不可关闭");
    assert_eq!(workspaces.rows[0].detail, "aggregate @ local + ssh");
    assert_eq!(workspaces.rows[1].title, "Agents");
    assert_eq!(workspaces.rows[1].shortcut.as_deref(), Some("A"));
    assert_eq!(workspaces.rows[1].detail, "aggregate @ all workspaces");
    assert_eq!(workspaces.rows[2].title, "dev");
    assert_eq!(workspaces.rows[2].shortcut.as_deref(), Some("1"));
    assert_eq!(workspaces.rows[3].title, "ops");
    assert_eq!(workspaces.rows[3].shortcut.as_deref(), Some("2"));
    assert!(workspaces.rows[2].active, "第一个工作区是当前活动");
    assert_eq!(workspaces.rows[2].detail, "tmux @ local");
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

/// 侧栏长名字必须单行截断。
///
/// 用户报障（截图）：`feature-ssa2llvm-support_ci_and_mustpass_test` 在
/// Workspaces 段和 Agents 段都换行成两行 —— 第二行压住副标题，活动行的
/// 尾部关闭按钮还被顶出侧栏裁掉一半。Swift 的 `NSTextField` 是
/// `byTruncatingTail`，这里必须一致：标题/副标题单行、行高固定。
#[gpui::test]
fn long_sidebar_rows_stay_single_line(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::{
        ClientActivitySnapshot, ClientAttentionPane, ClientWorkspaceAttention,
    };
    const NAME: &str = "feature-ssa2llvm-support_ci_and_mustpass_test-with-an-extremely-long-branch-suffix-for-overflow";
    const ID: &str = "local@@long@tmux@long";
    let mut store = ViewStore::default();
    store.replace_topology(
        workspace(ID, NAME, true),
        vec![ClientTab {
            id: 1,
            name: "main".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 10,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "zsh".to_string(),
            }],
        )],
    );
    store.ensure_workspace(ID).backend_status = Some(backend_status::CONNECTED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_activity_snapshot_for_tests(&ClientActivitySnapshot {
                blocked_count: 0,
                workspaces: vec![ClientWorkspaceAttention {
                    workspace_id: ID.to_string(),
                    path: "/home/u/long".to_string(),
                    blocked: 0,
                    done: 1,
                    working: 0,
                    panes: vec![ClientAttentionPane {
                        muted: false,
                        workspace_id: ID.to_string(),
                        pane_id: 10,
                        status: "done".to_string(),
                        acknowledged: false,
                        last_line: "done".to_string(),
                        seq: 3,
                        process_name: Some("pi".to_string()),
                        process_is_agent: true,
                        agent_name: Some("pi".to_string()),
                        shell_name: None,
                    }],
                }],
            });
            // 快照 setter 故意不 notify（真实路径由 pump 统一置脏）。
            cx.notify();
        });
    });
    cx.run_until_parked();

    let titles: Vec<String> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.sidebar_model()
                .into_iter()
                .flat_map(|section| section.rows.into_iter().map(|row| row.title))
                .filter(|title| title.contains(NAME))
                .collect()
        })
    });
    assert_eq!(
        titles.len(),
        2,
        "长名字要同时出现在 Workspaces 与 Agents：{titles:?}"
    );

    for title in &titles {
        let name = cx
            .debug_bounds(Box::leak(format!("sidebar-name-{title}").into_boxed_str()))
            .unwrap_or_else(|| panic!("标题必须渲染：{title}"));
        let detail = cx
            .debug_bounds(Box::leak(
                format!("sidebar-detail-{title}").into_boxed_str(),
            ))
            .unwrap_or_else(|| panic!("副标题必须渲染：{title}"));
        assert!(
            (f32::from(name.size.height) - 15.0).abs() < 0.6,
            "标题必须单行（换行 = 撑破行高）：{title} {name:?}"
        );
        assert!(
            (f32::from(detail.size.height) - 12.0).abs() < 0.6,
            "副标题必须单行：{title} {detail:?}"
        );
        assert!(
            f32::from(detail.origin.y) + 0.6 >= f32::from(name.origin.y) + 15.0,
            "副标题不能被标题压住：{title} {name:?} {detail:?}"
        );
    }

    // 活动 workspace 行的关闭按钮必须完整落在侧栏里。
    let trailing = cx
        .debug_bounds(Box::leak(
            format!("sidebar-trailing-{NAME}").into_boxed_str(),
        ))
        .expect("活动行必须有尾部按钮");
    let sidebar = cx.debug_bounds("sidebar").expect("侧栏必须渲染");
    assert!(
        f32::from(trailing.origin.x) + f32::from(trailing.size.width)
            <= f32::from(sidebar.origin.x) + f32::from(sidebar.size.width) - 5.0,
        "关闭按钮不能被裁掉：{trailing:?} / {sidebar:?}"
    );
}

/// 快速面板的行也是单行：长 workspace 名不许把行撑高。
#[gpui::test]
fn quick_panel_rows_stay_one_line_with_long_names(cx: &mut TestAppContext) {
    const NAME: &str = "feature-ssa2llvm-support_ci_and_mustpass_test-with-an-extremely-long-branch-suffix-for-overflow";
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            // discovery 行用真实 workspace 名：长名字以前会换行、把行撑高。
            app.set_existing_for_tests(vec![existing_runtime(NAME, 2)]);
            cx.notify();
        })
    });
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();

    let titles: Vec<String> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.panel_rows().iter().map(|row| row.title()).collect()
        })
    });
    assert!(
        titles.iter().any(|title| title.contains(NAME)),
        "面板必须列出长名字的 discovery 行：{titles:?}"
    );

    let mut title_heights: Vec<f32> = Vec::new();
    let mut row = 0usize;
    while let Some(title) =
        cx.debug_bounds(Box::leak(format!("panel-title-{row}").into_boxed_str()))
    {
        let row_bounds = cx
            .debug_bounds(Box::leak(format!("panel-row-{row}").into_boxed_str()))
            .unwrap_or_else(|| panic!("第 {row} 行必须渲染"));
        assert!(
            f32::from(title.origin.x) + f32::from(title.size.width)
                <= f32::from(row_bounds.origin.x) + f32::from(row_bounds.size.width) - 4.0,
            "第 {row} 行标题溢出面板行（长名字必须省略号收尾）：{title:?} / {row_bounds:?}"
        );
        title_heights.push(f32::from(title.size.height));
        row += 1;
    }
    assert_eq!(row, titles.len(), "面板行必须全部渲染：{titles:?}");
    assert_eq!(row, titles.len(), "面板行必须全部渲染：{titles:?}");
    let tallest = title_heights.iter().copied().fold(0.0_f32, f32::max);
    let shortest = title_heights.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        tallest - shortest < 0.6,
        "面板行标题必须等高（换行会把长名字那行撑高）：{title_heights:?}"
    );
}

/// tab 标题同样单行截断，不能盖住关闭按钮。
#[gpui::test]
fn a_long_tab_name_truncates_instead_of_wrapping(cx: &mut TestAppContext) {
    const NAME: &str = "feature-ssa2llvm-support_ci_and_mustpass_test-with-an-extremely-long-branch-suffix-for-overflow";
    let mut store = ViewStore::default();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![ClientTab {
            id: 1,
            name: NAME.to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 10,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "zsh".to_string(),
            }],
        )],
    );
    let (view, cx) = mount(store, cx);
    cx.run_until_parked();
    let label = cx.debug_bounds("tab-label-1").expect("tab 标题必须渲染");
    let tab = cx.debug_bounds("tab-1").expect("tab 必须渲染");
    assert!(
        (f32::from(label.size.height) - 14.0).abs() < 0.6,
        "tab 标题必须单行：{label:?}"
    );
    assert!(
        f32::from(label.origin.x) + f32::from(label.size.width)
            <= f32::from(tab.origin.x) + f32::from(tab.size.width) - 2.0,
        "标题不能盖住关闭按钮：{label:?} / {tab:?}"
    );
    let _ = view;
}

/// 侧栏的 Shells / Agents 行点进去 = 固定聚合投影，绝不能当成 Core workspace。
#[gpui::test]
fn clicking_an_aggregate_row_switches_the_projection(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::AggregateKind;
    use muxterm::frontend::gpui::model::sidebar::{AGGREGATE_AGENTS, AGGREGATE_SHELLS};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.activate_sidebar_row(RowTarget::Workspace(AGGREGATE_SHELLS.to_string()), cx)
        })
    });
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.aggregate_kind())),
        Some(AggregateKind::Shells)
    );
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)))
            .as_deref(),
        Some("local@@dev@tmux@dev"),
        "聚合槽不拥有 Surface，不能把 visible 改成聚合 id"
    );

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.activate_sidebar_row(RowTarget::Workspace(AGGREGATE_AGENTS.to_string()), cx)
        })
    });
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.aggregate_kind())),
        Some(AggregateKind::Agents)
    );
}

/// 本机 shell 的原始 workspace 不占侧栏行：它的 tab 只经 Shells 投影出现。
///
/// 对照 Swift `sidebarItems()`：`regularItems` 过滤 `runtime == shell`。
#[gpui::test]
fn sidebar_hides_the_raw_local_shell_workspace(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    store.replace_topology(
        ClientWorkspace {
            id: "local//shell/tmux/".to_string(),
            name: "shell".to_string(),
            runtime: "shell".to_string(),
            active: true,
            resolved_target: None,
        },
        vec![ClientTab {
            id: 1,
            name: "shell".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 5,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "shell".to_string(),
            }],
        )],
    );
    store.ensure_workspace("local//shell/tmux/").backend_status = Some(backend_status::EXITED);
    let (view, cx) = mount(store, cx);
    let items = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_items()));
    assert_eq!(items.len(), 2, "只剩两个聚合槽：{items:?}");
    assert_eq!(
        items[0].workspace_id,
        muxterm::frontend::gpui::model::sidebar::AGGREGATE_SHELLS
    );
    assert_eq!(
        items[1].workspace_id,
        muxterm::frontend::gpui::model::sidebar::AGGREGATE_AGENTS
    );
    assert!(!items.iter().any(|item| item.runtime == "shell"));
    assert!(items[0].is_active, "启动就是本机 shell 时 Shells 高亮");
}

/// Shells 里没有任何本机 shell 时，点聚合行进 Catalog 开一个。
///
/// 对照 Swift `activateShells` → `openLocalShellWorkspace()`：不能只把
/// presentation 切过去，那样主区是一片空白。
#[gpui::test]
fn clicking_shells_without_a_shell_opens_one(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::AggregateKind;
    use muxterm::frontend::gpui::model::sidebar::AGGREGATE_SHELLS;
    use muxterm::frontend::utils::corebridge::ClientOpenIntent;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.activate_sidebar_row(RowTarget::Workspace(AGGREGATE_SHELLS.to_string()), cx)
        })
    });
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.aggregate_kind())),
        Some(AggregateKind::Shells)
    );
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let (target, intent) = commands
        .iter()
        .find_map(|command| match command {
            ClientCommand::OpenTarget { target, intent } => Some((target.clone(), *intent)),
            _ => None,
        })
        .expect("Shells 里没有 shell 时必须经 Catalog 开一个：");
    assert_eq!(target.runtime, "shell");
    assert_eq!(target.transport, "local");
    assert_eq!(intent, ClientOpenIntent::CreateIfMissing);
    assert!(
        target.path.starts_with('/'),
        "本机路径要展开成绝对路径：{}",
        target.path
    );
}

/// Shells 投影里的 Cmd-T 落在真实本机 shell 上，不是当前 tmux workspace。
#[gpui::test]
fn cmd_t_in_the_shells_aggregate_targets_the_local_shell(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::AggregateKind;
    let mut store = ViewStore::default();
    store.replace_topology(
        ClientWorkspace {
            id: "local//shell/tmux/".to_string(),
            name: "shell".to_string(),
            runtime: "shell".to_string(),
            active: false,
            resolved_target: None,
        },
        vec![ClientTab {
            id: 1,
            name: "shell".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 5,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "shell".to_string(),
            }],
        )],
    );
    store.ensure_workspace("local//shell/tmux/").backend_status = Some(backend_status::CONNECTED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.select_aggregate(AggregateKind::Shells, cx);
            let _ = app.take_commands_for_tests();
        })
    });
    cx.simulate_keystrokes("cmd-t");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                workspace_id: Some(workspace_id),
                task: ClientTask::NewTab,
            } if workspace_id == "local//shell/tmux/"
        )),
        "Cmd-T 必须落在本机 shell 上：{commands:?}"
    );
}

/// 本机 shell 退出（没有 tab/pane）时补一个新 tab；只发一次。
///
/// 对照 Swift `maybeCloseIfSessionEnded()`：`shell` + `local` 空工作区 → `newTab`。
#[gpui::test]
fn an_empty_local_shell_asks_core_for_one_new_tab(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    store.replace_topology(
        ClientWorkspace {
            id: "local//shell/tmux/".to_string(),
            name: "shell".to_string(),
            runtime: "shell".to_string(),
            active: true,
            resolved_target: None,
        },
        vec![],
        vec![],
    );
    store.ensure_workspace("local//shell/tmux/").backend_status = Some(backend_status::EXITED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.pump_once(cx);
        })
    });
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(
                command,
                ClientCommand::Task {
                    task: ClientTask::NewTab,
                    ..
                }
            ))
            .count(),
        1,
        "空 shell 必须补一个新 tab：{commands:?}"
    );

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.pump_once(cx);
        })
    });
    let again = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !again.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::NewTab,
                ..
            }
        )),
        "Core 回包前不能每拍重发：{again:?}"
    );
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

/// 本机 shell 的原始 workspace 不占编号：`Cmd-Ctrl-1` 永远是侧栏第一个 workspace。
///
/// 回归：`SwitchWorkspace` 以前用的是未过滤的 store 顺序，shell 占着第 1 位，
/// 侧栏和面板却把它隐藏了，于是「按 1」跳到侧栏里写着「2」的行。
#[gpui::test]
fn workspace_numbers_skip_the_raw_local_shell(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    // shell 先进 store，所以“按打开顺序”它排第一；但它不该占编号。
    store.replace_topology(
        ClientWorkspace {
            id: "local//shell/tmux/".to_string(),
            name: "shell".to_string(),
            runtime: "shell".to_string(),
            active: true,
            resolved_target: None,
        },
        vec![ClientTab {
            id: 1,
            name: "shell".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 5,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "shell".to_string(),
            }],
        )],
    );
    store.ensure_workspace("local//shell/tmux/").backend_status = Some(backend_status::CONNECTED);
    for (id, name, tab) in [
        ("local@@dev@tmux@dev", "dev", 2),
        ("local@@ops@tmux@ops", "ops", 3),
    ] {
        store.replace_topology(
            workspace(id, name, false),
            vec![ClientTab {
                id: tab,
                name: name.to_string(),
                is_active: true,
            }],
            vec![(
                tab,
                vec![ClientPane {
                    id: tab * 10,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: name.to_string(),
                }],
            )],
        );
        store.ensure_workspace(id).backend_status = Some(backend_status::CONNECTED);
    }
    let (view, cx) = mount(store, cx);
    cx.simulate_keystrokes("cmd-ctrl-1");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(
        visible.as_deref(),
        Some("local@@dev@tmux@dev"),
        "1 必须是侧栏第一个 workspace，不是被隐藏的 shell"
    );
    cx.simulate_keystrokes("cmd-ctrl-2");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(visible.as_deref(), Some("local@@ops@tmux@ops"));
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
    assert_eq!(model[0].rows.len(), 4, "折叠不删行（含两个聚合槽）");
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
        .flat_map(|command| match command {
            ClientCommand::ConfigApplyTransaction { patches } => patches
                .iter()
                .map(|patch| (patch.path.clone(), patch.value.clone().unwrap()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, ClientCommand::ConfigApplyTransaction { .. }))
            .count(),
        1,
        "所有字段必须原子提交"
    );
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

    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())),
        "没有 Core 成功回执前保留草稿"
    );
    cx.update(|_, cx| view.update(cx, |app, cx| app.complete_settings_apply(true, cx)));
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

/// 已打开的 workspace 在面板里按打开顺序排，与侧栏编号、`Cmd-Ctrl-N` 一致。
///
/// 回归：之前在面板里按 Core 的 attach 频次重排，行首却还写着编号，
/// 于是「第 1 个打开的」跑到列表后面，按 `Cmd-Ctrl-1` 落到别的行。
#[gpui::test]
fn panel_keeps_opened_workspaces_in_numbered_order(cx: &mut TestAppContext) {
    let mut store = store_with_two_workspaces();
    // 第三个（非活跃）workspace：频次最高，但那不能把它排到前面。
    store.replace_topology(
        workspace("local@@web@tmux@web", "web", false),
        vec![ClientTab {
            id: 3,
            name: "web".to_string(),
            is_active: true,
        }],
        vec![(
            3,
            vec![ClientPane {
                id: 30,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "web".to_string(),
            }],
        )],
    );
    store.ensure_workspace("local@@web@tmux@web").backend_status = Some(backend_status::CONNECTED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_usage_for_tests(vec![
                muxterm::frontend::gpui::model::usage::UsageEntry {
                    identity: "local@@ops@tmux@ops".into(),
                    workspace_id: Some("local@@ops@tmux@ops".into()),
                    target_workspace_id: None,
                    project_id: None,
                    count: 4,
                    last_used_at: 10,
                },
                muxterm::frontend::gpui::model::usage::UsageEntry {
                    identity: "local@@web@tmux@web".into(),
                    workspace_id: Some("local@@web@tmux@web".into()),
                    target_workspace_id: None,
                    project_id: None,
                    count: 9,
                    last_used_at: 100,
                },
            ]);
            app.show_panel(
                muxterm::frontend::gpui::model::panel::PanelTab::Workspaces,
                cx,
            );
        })
    });
    let titles: Vec<String> = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.panel_rows()
                .iter()
                .filter(|row| {
                    matches!(
                        row,
                        muxterm::frontend::gpui::chrome::panel_view::PanelRow::Workspace { .. }
                    )
                })
                .map(|row| row.title())
                .collect()
        })
    });
    assert_eq!(
        titles,
        vec!["dev · local", "ops · local", "web · local"],
        "打开顺序就是侧栏编号顺序，频次和活跃度都不能重排：{titles:?}"
    );
}

/// 面板行首的编号与侧栏一致：1 永远是第一个打开的 workspace。
#[gpui::test]
fn panel_numbers_match_the_sidebar(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    use muxterm::frontend::gpui::model::sidebar::{AGGREGATE_AGENTS, AGGREGATE_SHELLS};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let (sidebar, panel) = cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.show_panel(
                muxterm::frontend::gpui::model::panel::PanelTab::Workspaces,
                cx,
            );
            let sidebar: Vec<(String, Option<usize>)> = app
                .sidebar_model()
                .into_iter()
                .flat_map(|section| section.rows)
                .filter_map(|row| match &row.target {
                    RowTarget::Workspace(id)
                        if id != AGGREGATE_SHELLS && id != AGGREGATE_AGENTS =>
                    {
                        Some((
                            id.clone(),
                            row.shortcut
                                .as_deref()
                                .and_then(|text| text.parse::<usize>().ok()),
                        ))
                    }
                    _ => None,
                })
                .collect();
            let panel: Vec<(String, Option<usize>)> = app
                .panel_rows()
                .iter()
                .filter_map(|row| match row {
                    PanelRow::Workspace {
                        workspace_id,
                        shortcut,
                        ..
                    } => Some((workspace_id.clone(), *shortcut)),
                    _ => None,
                })
                .collect();
            (sidebar, panel)
        })
    });
    assert_eq!(
        sidebar.len(),
        panel.len(),
        "面板与侧栏的 workspace 集合必须一致"
    );
    for (index, ((sidebar_id, sidebar_number), (panel_id, panel_number))) in
        sidebar.iter().zip(panel.iter()).enumerate()
    {
        assert_eq!(sidebar_number, panel_number, "同一行的编号必须一致");
        assert_eq!(
            panel_number,
            &Some(index + 1),
            "第 {} 行的编号必须是 {}：{panel_id}",
            index + 1,
            index + 1
        );
        assert_eq!(sidebar_id, panel_id, "面板顺序必须就是侧栏顺序");
    }
}

/// 面板在小窗口里必须缩到正文范围内：底部操作条和搜索框都不能被挤出去。
/// 对照 macOS `UnifiedPanelController` 里 640×420 的固定尺寸 + 屏幕内夹取。
#[gpui::test]
fn quick_panel_size_fits_the_content_area(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel;
    assert_eq!(panel::fitted_panel_size(960.0, 750.0), (640.0, 420.0));
    let (width, height) = panel::fitted_panel_size(420.0, 360.0);
    assert!(width < 420.0, "窄窗口里面板不能撑出正文：{width}");
    assert!(height < 360.0, "矮窗口里面板不能撑出正文：{height}");
    let _ = cx;
}

/// 面板列表的第一行必须贴在列表顶端：以前测试截图里 `Shells` 被画丢，
/// 用户打开 Cmd-P 第一行就少了。这里按几何锁定：行与行连续，行 0 在列表顶部。
#[gpui::test]
fn quick_panel_first_row_sits_at_the_list_top(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    let titles: Vec<_> = cx
        .update(|_, cx| view.update(cx, |app, _| app.panel_rows()))
        .iter()
        .map(|row| row.title())
        .collect();
    assert_eq!(titles.first().map(String::as_str), Some("Shells"));
    let list = cx.debug_bounds("panel-list").expect("panel list");
    let first = cx.debug_bounds("panel-row-0").expect("first row");
    assert_eq!(first.top(), list.top(), "第一行必须从列表顶部开始");
    let second = cx.debug_bounds("panel-row-1").expect("second row");
    assert_eq!(second.top(), first.bottom(), "行与行之间不能有缝或重叠");
}

/// 面板每次打开都从顶部开始：滚动偏移必须被显式清零。
/// `scroll_to_top_of_item(0)` 会拿上一帧的 child_bounds 反推偏移，面板尺寸一变
/// （搜索框加高）就会整行错位 —— 实测第一行 Shells 被顶出可视区。
#[gpui::test]
fn quick_panel_opens_scrolled_to_the_top(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    let offset = cx.update(|_, cx| view.update(cx, |app, _| app.panel_scroll_handle().offset()));
    assert_eq!(offset.y, gpui::px(0.), "面板必须从第一行开始");
}

/// 面板打开后必须同时画出搜索框、逐行和底部键盘提示：
/// 用户报障「面板看不出怎么操作」，所以提示条是回归项。
#[gpui::test]
fn quick_panel_renders_its_footer_hints(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-footer").is_some(),
        "面板底部要有 ↑↓ / ⏎ / ⌘K / Esc 提示"
    );
}

/// 查不到东西时要给出「无结果」而不是一片空白。
#[gpui::test]
fn quick_panel_reports_an_empty_search(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.panel_push_query("zz-no-such-workspace", cx)
        })
    });
    cx.run_until_parked();
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert!(rows.is_empty(), "不该有匹配行：{rows:?}");
    assert!(
        cx.debug_bounds("panel-empty").is_some(),
        "空结果要有占位说明"
    );
}

/// Esc 关设置。以前浮层没有按键分支：Esc 既关不掉，还会漏进终端。
#[gpui::test]
fn escape_closes_the_settings_overlay(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &serde_json::json!({}), cx)
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("settings-panel").is_some(),
        "设置必须画出来"
    );
    cx.simulate_keystrokes("escape");
    assert!(
        !cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)),
        "Esc 必须关掉设置"
    );
}

/// 有未保存修改时 Esc 先问；再按 Esc 回到编辑；点 Discard 才关，且值回滚。
#[gpui::test]
fn escape_with_unsaved_changes_asks_before_discarding(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "font": { "size": 13.0 } }),
                cx,
            );
            app.settings_set("/font/size", serde_json::json!(17.0), cx);
        })
    });

    cx.simulate_keystrokes("escape");
    let (open, pending) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.settings_open, app.settings_discard_pending())
        })
    });
    assert!(open, "有改动时 Esc 不能直接关");
    assert!(pending, "要先确认放弃修改");
    assert!(
        cx.debug_bounds("settings-discard").is_some(),
        "确认条必须画出 Discard"
    );

    cx.simulate_keystrokes("escape");
    let (open, pending) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.settings_open, app.settings_discard_pending())
        })
    });
    assert!(open, "再按 Esc 是继续编辑，不是关闭");
    assert!(!pending, "确认条要收起来");

    // 再按 Esc 回到确认条，然后点 Discard：关闭并回滚。
    cx.simulate_keystrokes("escape");
    let discard = cx.debug_bounds("settings-discard").expect("确认条要回来");
    cx.simulate_click(discard.center(), Default::default());
    let (open, dirty, value) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.settings_open,
                app.settings_dirty(),
                app.settings_value("/font/size"),
            )
        })
    });
    assert!(!open, "Discard 关闭设置");
    assert!(!dirty, "Discard 回滚草稿");
    assert_eq!(value, Some(serde_json::json!(13.0)), "回到打开时的值");
}

/// 底部按钮：Cancel 有改动先确认，Discard 关窗口并回滚；Apply 写 Core 后关闭。
#[gpui::test]
fn settings_footer_buttons_apply_discard_and_cancel(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "font": { "size": 13.0 } }),
                cx,
            )
        })
    });
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.settings_set("/font/size", serde_json::json!(17.0), cx)
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("settings-cancel").is_some(),
        "Cancel 必须画出来"
    );
    assert!(
        cx.debug_bounds("settings-apply").is_some(),
        "Apply 必须画出来"
    );

    let cancel = cx.debug_bounds("settings-cancel").unwrap();
    cx.simulate_click(cancel.center(), Default::default());
    let state = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.settings_open,
                app.settings_discard_pending(),
                app.settings_dirty(),
            )
        })
    });
    assert!(state.1, "Cancel 有改动时先确认（实际 {state:?}）");

    // Discard：关窗口 + 回滚。
    let discard = cx.debug_bounds("settings-discard").unwrap();
    cx.simulate_click(discard.center(), Default::default());
    let (is_open, dirty) =
        cx.update(|_, cx| view.update(cx, |app, _| (app.settings_open, app.settings_dirty())));
    assert!(!is_open, "Discard 关闭设置");
    assert!(!dirty, "Discard 回滚草稿");

    // Apply：写 Core 一条 patch 后关闭。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "font": { "size": 13.0 } }),
                cx,
            )
        })
    });
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.settings_set("/font/size", serde_json::json!(19.0), cx)
        })
    });
    let apply = cx.debug_bounds("settings-apply").unwrap();
    cx.simulate_click(apply.center(), Default::default());
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)),
        "等待 Core 回执"
    );
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::ConfigApplyTransaction { patches }
                if patches.len() == 1 && patches[0].path == "/font/size" && patches[0].value == Some(serde_json::json!(19.0))
        )),
        "Apply 必须写 Core：{commands:?}"
    );
    cx.update(|_, cx| view.update(cx, |app, cx| app.complete_settings_apply(true, cx)));
    assert!(!cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)));
}

/// 设置浮层独占键盘：可打印键只改搜索词，绝不进 PTY。
#[gpui::test]
fn settings_keys_never_reach_the_shell(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(&settings_manifest(), &serde_json::json!({}), cx)
        })
    });
    let before = cx.update(|_, cx| view.update(cx, |app, _| app.settings_query().to_string()));
    cx.simulate_keystrokes("a b");
    let after = cx.update(|_, cx| view.update(cx, |app, _| app.settings_query().to_string()));
    assert_eq!(before, "", "搜索词初始为空");
    assert_eq!(after, "ab", "可打印键写进搜索框");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::Input { .. })),
        "设置打开时按键不能漏进终端：{commands:?}"
    );
}

/// 数字字段：退格删字符、打字追加，都落进草稿。
#[gpui::test]
fn typing_into_a_number_field_updates_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "quick_panel": { "attach_history_days": 30 } }),
                cx,
            );
            app.settings_select("quick_panel", cx);
            app.settings_focus_field(0, cx);
        })
    });
    cx.simulate_keystrokes("backspace backspace 1 2");
    let value = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.settings_value("/quick_panel/attach_history_days")
        })
    });
    assert_eq!(value, Some(serde_json::json!(12.0)), "数字字段要能改");
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.settings_dirty())));
}

/// 开关控件：点一下在草稿里翻转，不写 Core。
#[gpui::test]
fn clicking_a_switch_toggles_it_in_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "attention": { "enabled": true } }),
                cx,
            );
            app.settings_select("attention", cx);
        })
    });
    cx.run_until_parked();
    let control = cx
        .debug_bounds("settings-control-/attention/enabled")
        .expect("开关控件必须画出来");
    cx.simulate_click(control.center(), Default::default());
    let value =
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_value("/attention/enabled")));
    assert_eq!(value, Some(serde_json::json!(false)), "点开关要翻转");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::ConfigApply { .. })),
        "草稿阶段不能直接写 Core：{commands:?}"
    );
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
    tx.unbounded_send(
        muxterm::frontend::gpui::model::shortcuts::ShortcutCommand::Action(KeyAction::NewTab),
    )
    .expect("菜单动作");
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

/// 一个 tab 里三个 pane，布局叶子顺序 = 屏幕顺序。
fn three_pane_store(active_pane: u32) -> ViewStore {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let mut store = ViewStore::default();
    let id = "local@@dev@tmux@dev";
    store.replace_topology(
        workspace(id, "dev", true),
        vec![ClientTab {
            id: 1,
            name: "dev".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            [10, 11, 12]
                .into_iter()
                .map(|pane_id| ClientPane {
                    id: pane_id,
                    cols: 80,
                    rows: 24,
                    is_active: pane_id == active_pane,
                    title: format!("pane {pane_id}"),
                })
                .collect(),
        )],
    );
    store.replace_layouts(
        id,
        vec![(
            1,
            ClientLayout::Split {
                horizontal: true,
                ratio: 50,
                first: Box::new(ClientLayout::Leaf { pane_id: 10 }),
                second: Box::new(ClientLayout::Split {
                    horizontal: false,
                    ratio: 50,
                    first: Box::new(ClientLayout::Leaf { pane_id: 11 }),
                    second: Box::new(ClientLayout::Leaf { pane_id: 12 }),
                }),
            },
        )],
    );
    store.ensure_workspace(id).backend_status = Some(backend_status::CONNECTED);
    store
}

/// Cmd-[ / Cmd-] 必须落到当前 tab 的具体 pane（对照 `PaneNavigationTests`
/// 与 Swift `MainWindow.movePane(offset:)`），不能交给 Core 的全局推断。
#[gpui::test]
fn cmd_brackets_walk_the_panes_of_the_current_tab(cx: &mut TestAppContext) {
    let (view, cx) = mount(three_pane_store(10), cx);
    cx.simulate_keystrokes("cmd-]");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SwitchPane { pane_id: 11 },
                ..
            }
        )),
        "Cmd-] 必须是布局里的下一个 pane：{commands:?}"
    );
    assert!(
        !commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::NextPane | ClientTask::PreviousPane,
                ..
            }
        )),
        "不能退化成让 Core 自己猜 active pane：{commands:?}"
    );

    // Core 确认后焦点跟着走，下一次 Cmd-] 从 11 继续。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.replace_store_for_tests(three_pane_store(11), cx);
            app.pump_once(cx);
        })
    });
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane())),
        Some(11)
    );
    cx.simulate_keystrokes("cmd-]");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SwitchPane { pane_id: 12 },
                ..
            }
        )),
        "Cmd-] 继续往后走：{commands:?}"
    );

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.replace_store_for_tests(three_pane_store(12), cx);
            app.pump_once(cx);
        })
    });
    cx.simulate_keystrokes("cmd-[");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SwitchPane { pane_id: 11 },
                ..
            }
        )),
        "Cmd-[ 回上一个：{commands:?}"
    );
}

/// tmux zoom：布局只剩一个叶子，导航仍要走完当前 tab 的全部 pane，
/// 并在切换后对目标 pane 重新全屏（Swift `movePane` 的 wasZoomed 分支）。
#[gpui::test]
fn cmd_brackets_leave_a_zoomed_pane_by_walking_the_snapshot(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let mut store = three_pane_store(11);
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![(1, ClientLayout::Leaf { pane_id: 11 })],
    );
    let (view, cx) = mount(store, cx);
    cx.simulate_keystrokes("cmd-]");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SwitchPane { pane_id: 12 },
                ..
            }
        )),
        "zoom 下仍要有下一个 pane：{commands:?}"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::TogglePaneFullscreen { pane_id: 12 },
                ..
            }
        )),
        "切换后对新 pane 重新全屏：{commands:?}"
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
            app.replace_store_for_tests(store, cx);
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
                ratio: 400,
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
            app.replace_store_for_tests(store, cx);
            app.feed_pane_for_tests("local@@dev@tmux@dev", 12, 40, 24, b"new", cx);
            app.pump_once(cx);
        })
    });
    cx.run_until_parked();
    let focused = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
    assert_eq!(focused, Some(12), "新建的 pane 必须是焦点");
    assert!(cx.debug_bounds("pane-cursor").is_some());
}

// ---------------------------------------------------------------------------
// 统一面板根列表：两个特殊 shell 在最前，已打开的不能再以 Project 重复出现
// ---------------------------------------------------------------------------

/// 根列表顺序：Shells → Agents → Open → 已打开的 workspace（Swift
/// `loadWorkspaceItems` 的 pinned + [.open] + opened）。
#[gpui::test]
fn panel_root_starts_with_the_two_special_shells(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::{AggregateKind, PickerAction};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.show_panel(PanelTab::Workspaces, cx)));
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert!(
        matches!(
            rows[0],
            muxterm::frontend::gpui::chrome::panel_view::PanelRow::Aggregate(AggregateKind::Shells)
        ),
        "第一行必须是 Shells：{rows:?}"
    );
    assert!(
        matches!(
            rows[1],
            muxterm::frontend::gpui::chrome::panel_view::PanelRow::Aggregate(AggregateKind::Agents)
        ),
        "第二行必须是 Agents：{rows:?}"
    );
    assert!(
        matches!(
            rows[2],
            muxterm::frontend::gpui::chrome::panel_view::PanelRow::Action(PickerAction::Open)
        ),
        "第三行必须是 Open：{rows:?}"
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-row-0").is_some(),
        "Shells 行必须真的画出来"
    );
    assert!(
        cx.debug_bounds("panel-row-1").is_some(),
        "Agents 行必须真的画出来"
    );
    // 只有 Open 一行，不再混入 New / Open Folder。
    let opens = rows
        .iter()
        .filter(|row| {
            matches!(
                row,
                muxterm::frontend::gpui::chrome::panel_view::PanelRow::Action(PickerAction::Open)
            )
        })
        .count();
    assert_eq!(opens, 1, "Open 只能出现一次");
}

/// 已经打开的 workspace 不能再以 Project / 现有 runtime 的身份重复出现。
#[gpui::test]
fn panel_dedupes_open_workspaces_against_saved_projects(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    let mut store = store_with_two_workspaces();
    // 打开的是 ssh 上的 tmux（`resolved_target` 可能还没回，按名字兜底去重）。
    store.replace_topology(
        ClientWorkspace {
            id: "ssh@@ryzen@yaklang-workspace@tmux@/home/u/yaklang-workspace".to_string(),
            name: "yaklang-workspace".to_string(),
            runtime: "tmux".to_string(),
            active: false,
            resolved_target: None,
        },
        vec![ClientTab {
            id: 3,
            name: "yaklang-workspace".to_string(),
            is_active: true,
        }],
        vec![(
            3,
            vec![ClientPane {
                id: 30,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "yaklang-workspace".to_string(),
            }],
        )],
    );
    store
        .ensure_workspace("ssh@@ryzen@yaklang-workspace@tmux@/home/u/yaklang-workspace")
        .backend_status = Some(backend_status::CONNECTED);

    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_picker_data_for_tests(
                serde_json::json!([{
                    "id": "project-1",
                    "name": "yaklang-workspace",
                    "path": "/home/u/yaklang-workspace",
                    "runtime": { "id": "tmux", "session": "yaklang-workspace" },
                    "transport": { "id": "ssh", "target": "ryzen" }
                }]),
                vec!["ryzen".to_string()],
            );
            app.show_panel(PanelTab::Workspaces, cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    let mentions = rows
        .iter()
        .filter(|row| {
            matches!(
                row,
                PanelRow::Workspace { .. } | PanelRow::Target { .. } | PanelRow::Existing { .. }
            ) && row.title().starts_with("yaklang-workspace")
        })
        .count();
    assert_eq!(
        mentions,
        1,
        "已打开的 workspace 只能出现一次：{:?}",
        rows.iter().map(|row| row.title()).collect::<Vec<_>>()
    );
}

/// 现有 runtime 里已经打开的那个 session 也不能再列一次。
#[gpui::test]
fn panel_dedupes_existing_sessions_that_are_already_open(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    use muxterm::frontend::gpui::model::existing::ExistingRuntime;
    let mut store = store_with_two_workspaces();
    store.replace_topology(
        ClientWorkspace {
            id: "local@@dev@tmux@dev".to_string(),
            name: "dev".to_string(),
            runtime: "tmux".to_string(),
            active: true,
            resolved_target: None,
        },
        vec![ClientTab {
            id: 1,
            name: "dev".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 10,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "dev".to_string(),
            }],
        )],
    );
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_existing_for_tests(vec![ExistingRuntime {
                name: "dev".to_string(),
                runtime: "tmux".to_string(),
                transport: "local".to_string(),
                target: String::new(),
                session: "dev".to_string(),
                socket: None,
                windows: 1,
                attached: false,
                workspace_id: None,
                path: "/home/u/dev".to_string(),
            }]);
            app.show_panel(PanelTab::Workspaces, cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    let dev_existing = rows
        .iter()
        .filter(|row| matches!(row, PanelRow::Existing { .. }) && row.title() == "dev")
        .count();
    assert_eq!(
        dev_existing,
        0,
        "已打开的 session 不能再列成现有 runtime：{:?}",
        rows.iter().map(|row| row.title()).collect::<Vec<_>>()
    );
}

/// 远端 Project 的连接身份来自 `canonical.target`（Core 字段名）。
///
/// 回归：gpui 曾误读 `canonical.alias`，SSH 主机变空串 → 已打开的远端
/// Project 又在 Project 列表里重复一次。
#[gpui::test]
fn panel_dedupes_ssh_projects_by_the_canonical_target(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    let mut store = store_with_two_workspaces();
    store.replace_topology(
        ClientWorkspace {
            id: "ssh/ryzen/dev/tmux//home/u/dev".to_string(),
            name: "dev-remote".to_string(),
            runtime: "tmux".to_string(),
            active: false,
            resolved_target: Some(serde_json::json!({
                "canonical": {
                    "name": "dev-remote",
                    "runtime": "tmux",
                    "transport": "ssh",
                    "target": "ryzen",
                    "path": "/home/u/dev",
                    "session": "dev",
                    "socket": null,
                    "workspace_id": null
                },
                "spec": {
                    "transport": "ssh",
                    "alias": "ryzen",
                    "session": "dev",
                    "runtime": "tmux",
                    "path": "/home/u/dev",
                    "socket": null,
                    "provenance": null,
                    "template": null
                }
            })),
        },
        vec![ClientTab {
            id: 4,
            name: "dev".to_string(),
            is_active: true,
        }],
        vec![(
            4,
            vec![ClientPane {
                id: 40,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "dev".to_string(),
            }],
        )],
    );

    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_picker_data_for_tests(
                serde_json::json!([{
                    "id": "project-dev",
                    "name": "dev-remote",
                    "path": "/home/u/dev",
                    "runtime": { "id": "tmux", "session": "dev" },
                    "transport": { "id": "ssh", "target": "ryzen" }
                }]),
                vec!["ryzen".to_string()],
            );
            app.show_panel(PanelTab::Workspaces, cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, PanelRow::Target { .. }))
            .count(),
        0,
        "已打开的远端 Project 不能再列一次：{:?}",
        rows.iter().map(|row| row.title()).collect::<Vec<_>>()
    );
}

/// 打开 Project 时 Core 会写 provenance；它是比 target 字段更稳的去重身份。
#[gpui::test]
fn panel_dedupes_projects_by_core_provenance(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    let mut store = store_with_two_workspaces();
    // canonical 与 Project 配置并不完全相等（Herdr 的 socket/session 由 Core
    // 解析后可能变形），但 provenance 指向同一个 project id。
    store.replace_topology(
        ClientWorkspace {
            id: "local//work/herdr/w2".to_string(),
            name: "legion".to_string(),
            runtime: "herdr".to_string(),
            active: false,
            resolved_target: Some(serde_json::json!({
                "canonical": {
                    "name": "legion",
                    "runtime": "herdr",
                    "transport": "local",
                    "target": "",
                    "path": "/work",
                    "session": "default",
                    "socket": "/tmp/herdr-forward.sock",
                    "workspace_id": "w2"
                },
                "spec": {
                    "transport": "local",
                    "alias": null,
                    "session": "default",
                    "runtime": "herdr",
                    "path": "/work",
                    "socket": "/tmp/herdr-forward.sock",
                    "provenance": { "project_id": "project-legion", "worktree_id": null },
                    "template": null
                }
            })),
        },
        vec![ClientTab {
            id: 5,
            name: "main".to_string(),
            is_active: true,
        }],
        vec![(
            5,
            vec![ClientPane {
                id: 50,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "main".to_string(),
            }],
        )],
    );

    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_picker_data_for_tests(
                serde_json::json!([{
                    "id": "project-legion",
                    "name": "legion",
                    "path": "~/Developer/work/legion",
                    "runtime": { "id": "herdr" },
                    "transport": { "id": "local" }
                }]),
                vec![],
            );
            app.show_panel(PanelTab::Workspaces, cx);
        })
    });
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, PanelRow::Target { .. }))
            .count(),
        0,
        "provenance 命中就不能再列 Project：{:?}",
        rows.iter().map(|row| row.title()).collect::<Vec<_>>()
    );
}

/// SGR 属性必须真的进到渲染树：粗体 / 斜体 / 下划线 / 删除线各有样式 run。
///
/// 对照 macOS SwiftTerm `getAttributes`：`bold` → bold 字体、`italic` → italic、
/// `underline`/`crossedOut` → 下划线/删除线。之前只画颜色，markdown 的标题和
/// 强调看起来和正文一模一样。
#[gpui::test]
fn terminal_renders_bold_italic_underline_and_strike(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                80,
                24,
                b"plain\r\n\x1b[1mBOLD\x1b[0m\r\n\x1b[3mitalic\x1b[0m\r\n\x1b[4munderline\x1b[0m\r\n\x1b[9mstrike\x1b[0m",
                cx,
            );
        })
    });
    cx.run_until_parked();
    for selector in ["pane-bold", "pane-italic", "pane-underline", "pane-strike"] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} 应该出现在渲染树里"
        );
    }
}

/// 没有 SGR 属性的普通文本不能带出这些样式（否则整屏都变粗）。
#[gpui::test]
fn plain_terminal_text_carries_no_style_runs(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests("local@@dev@tmux@dev", 10, 80, 24, b"plain text", cx);
        })
    });
    cx.run_until_parked();
    for selector in ["pane-bold", "pane-italic", "pane-underline", "pane-strike"] {
        assert!(
            cx.debug_bounds(selector).is_none(),
            "{selector} 不该出现在普通文本里"
        );
    }
    assert!(
        cx.debug_bounds("pane-cursor").is_some(),
        "普通文本仍然要比光标"
    );
}

// ---------------------------------------------------------------------------
// 活动接入：Core 的活动快照 → Agents / Commands / Attention
// ---------------------------------------------------------------------------

fn activity_snapshot_with_agent() -> muxterm::frontend::utils::corebridge::ClientActivitySnapshot {
    use muxterm::frontend::utils::corebridge::{
        ClientActivitySnapshot, ClientAttentionPane, ClientWorkspaceAttention,
    };
    let pane = |pane_id: u32, status: &str, process: &str, agent: bool| ClientAttentionPane {
        muted: false,
        workspace_id: "local@@dev@tmux@dev".to_string(),
        pane_id,
        status: status.to_string(),
        acknowledged: false,
        last_line: format!("{process} line"),
        seq: 11,
        process_name: Some(process.to_string()),
        process_is_agent: agent,
        agent_name: agent.then(|| process.to_string()),
        shell_name: None,
    };
    ClientActivitySnapshot {
        blocked_count: 0,
        workspaces: vec![ClientWorkspaceAttention {
            workspace_id: "local@@dev@tmux@dev".to_string(),
            path: "/home/u/dev".to_string(),
            blocked: 0,
            done: 0,
            working: 2,
            panes: vec![
                pane(10, "working", "pi", true),
                pane(20, "working", "cargo", false),
            ],
        }],
    }
}

/// `pi` 在前台时 Agents 段要有一行；非 agent 进程进 Commands。
#[gpui::test]
fn activity_snapshot_fills_the_agents_and_commands_sections(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let changed = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_activity_snapshot_for_tests(&activity_snapshot_with_agent())
        })
    });
    assert!(changed, "第一次注入必须算作变化");
    cx.run_until_parked();

    let model = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_model()));
    let agents = &model[1];
    assert_eq!(agents.section, Section::Agents);
    assert_eq!(agents.rows.len(), 1, "pi 必须出现在 Agents：{agents:?}");
    assert!(
        agents.rows[0].title.starts_with("dev"),
        "Agents 行标题来自 workspace：{}",
        agents.rows[0].title
    );
    assert!(
        agents.rows[0].detail.contains("pi"),
        "详情要带 agent 名字：{}",
        agents.rows[0].detail
    );

    let commands = &model[2];
    assert_eq!(commands.section, Section::Commands);
    assert_eq!(
        commands.rows.len(),
        1,
        "cargo 必须出现在 Commands：{commands:?}"
    );
    assert!(commands.rows[0].detail.contains("cargo"));

    // Attention tab 也能列出同样两条。
    cx.update(|_, cx| view.update(cx, |app, cx| app.show_panel(PanelTab::Attention, cx)));
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert!(
        rows.len() >= 2,
        "Attention 面板要有两条活动：{:?}",
        rows.iter().map(|row| row.title()).collect::<Vec<_>>()
    );
}

/// 快照没变就不算 dirty（别每帧都重绘侧栏）。
#[gpui::test]
fn repeated_activity_snapshots_are_not_dirty(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let snapshot = activity_snapshot_with_agent();
    let (first, second) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.set_activity_snapshot_for_tests(&snapshot),
                app.set_activity_snapshot_for_tests(&snapshot),
            )
        })
    });
    assert!(first);
    assert!(!second, "同一份快照不能重复触发重绘");
}

/// Info 浮层的复制按钮：日志没开也要能复制（复制目录），并且给出反馈。
#[gpui::test]
fn info_copy_buttons_always_put_something_on_the_clipboard(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientBuildInfo;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_info_for_tests(ClientBuildInfo {
                version: "0.1.0".to_string(),
                commit: "deadbee".to_string(),
                log_file: None,
            });
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("info-panel").is_some(), "Info 必须画出来");

    // 点「Copy log dir」：日志没开时复制目录，不能什么都不做。
    let bounds = cx.debug_bounds("info-copy-log-button").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    let copied = cx.update(|_, cx| {
        cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                _ => None,
            })
        })
    });
    assert!(
        copied
            .as_deref()
            .is_some_and(|text| text.contains("muxterm-logs")),
        "日志没开时也要复制日志目录，实际 {copied:?}"
    );
    let status = cx.update(|_, cx| view.update(cx, |app, _| app.info_status().map(str::to_string)));
    assert!(
        status.unwrap_or_default().contains("copied"),
        "要有复制反馈"
    );

    // 点「Copy commit」。
    let bounds = cx.debug_bounds("info-copy-commit-button").unwrap();
    cx.simulate_click(bounds.center(), Default::default());
    cx.run_until_parked();
    let copied = cx.update(|_, cx| {
        cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                gpui::ClipboardEntry::String(text) => Some(text.text().to_string()),
                _ => None,
            })
        })
    });
    assert_eq!(copied.as_deref(), Some("deadbee"));
}

/// 中英文占相同终端列时，后面的 SGR run 必须落在同一个 x。
/// 回归用户截图：`中` 的 fallback 字宽不能把同一列的竖线推歪。
///
/// 容差 0.75px：gpui 布局会按设备像素对齐（2x 屏上 0.5 逻辑像素），
/// 但一个字形宽度差就有 2px 级别，所以这个容差不会放过真正的错位。
const GRID_TOL: f32 = 0.75;

#[gpui::test]
fn terminal_mixed_width_runs_use_terminal_columns(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                80,
                24,
                "中\u{1b}[1m│\u{1b}[0m\r\nAA\u{1b}[3m│\u{1b}[0m".as_bytes(),
                cx,
            );
        })
    });
    cx.run_until_parked();
    let after_cjk = cx.debug_bounds("pane-bold").expect("中文之后的粗体格");
    let after_ascii = cx.debug_bounds("pane-italic").expect("ASCII 之后的斜体格");
    let (cell_width, _) = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    assert!(
        (f32::from(after_cjk.origin.x - after_ascii.origin.x)).abs() < GRID_TOL,
        "中文两格与 ASCII 两格后的竖线必须对齐：{after_cjk:?} / {after_ascii:?}"
    );
    assert!(
        (f32::from(after_cjk.size.width) - cell_width).abs() < GRID_TOL,
        "单格 run 宽度必须严格等于一列，不能由字形自然 advance 决定"
    );
}

/// 光标、背景与字形共用一套列坐标，不能各自计算自然字宽。
#[gpui::test]
fn terminal_cursor_after_cjk_uses_the_same_grid(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                80,
                24,
                "\u{1b}[1mX\u{1b}[0m中文ab".as_bytes(),
                cx,
            );
        })
    });
    cx.run_until_parked();
    let first = cx.debug_bounds("pane-bold").expect("第一列基准");
    let cursor = cx.debug_bounds("pane-cursor").expect("尾部光标");
    let (cell_width, _) = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    let actual = f32::from(cursor.origin.x - first.origin.x);
    assert!(
        (actual - 7.0 * cell_width).abs() < GRID_TOL,
        "X(1列)+中文(4列)+ab(2列)后的光标应在第7列：实际 {actual}, cell={cell_width}"
    );
    assert!(
        (f32::from(cursor.size.width) - cell_width).abs() < GRID_TOL,
        "空格光标必须填满一格"
    );
}

/// 用户报障：后台 pane 持续输出也让当前终端重画，CPU 长期接近一核。
/// 对照 Surface「常驻 Scene、隐藏 pane 仍收输出」：要更新 Surface，但不能请求前台重绘。
#[gpui::test]
fn background_workspace_output_does_not_dirty_the_terminal(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::{ClientEvent, ClientWorkspaceEvent};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.store_for_tests_event(ClientWorkspaceEvent {
                workspace_id: "local@@ops@tmux@ops".into(),
                event: ClientEvent {
                    type_: 0,
                    pane_id: 20,
                    tab_id: 2,
                    window_id: 0,
                    data: b"background output".to_vec(),
                    name: String::new(),
                },
            });
            assert!(!app.drain_render_events(), "后台输出不能让当前终端变脏");
            let text = app
                .surface("local@@ops@tmux@ops", 20)
                .unwrap()
                .grid()
                .screen_lines()
                .join("\n");
            assert!(
                text.contains("background output"),
                "后台 Surface 不能丢输出"
            );
        });
    });
}

/// 同一 workspace 的隐藏 tab 也只更新 Surface；切过去时仍能读到最新画面。
#[gpui::test]
fn background_tab_output_does_not_dirty_the_terminal(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::{ClientEvent, ClientWorkspaceEvent};
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.store_for_tests_event(ClientWorkspaceEvent {
                workspace_id: "local@@dev@tmux@dev".into(),
                event: ClientEvent {
                    type_: 0,
                    pane_id: 20,
                    tab_id: 2,
                    window_id: 0,
                    data: b"hidden tab output".to_vec(),
                    name: String::new(),
                },
            });
            assert!(!app.drain_render_events(), "隐藏 tab 输出不能要求重绘");
            let text = app
                .surface("local@@dev@tmux@dev", 20)
                .unwrap()
                .grid()
                .screen_lines()
                .join("\n");
            assert!(text.contains("hidden tab output"));
        });
    });
}

/// 空闲的 pump 不能报告 dirty：没有变化就不重绘（CPU 不做白工）。
#[gpui::test]
fn idle_pump_is_not_dirty(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    let dirty = cx.update(|_, cx| view.update(cx, |app, cx| app.pump_once(cx)));
    assert!(!dirty, "没有事件时 pump_once 不能要求重绘");
}

/// 一次 Cmd-D 只能分一次屏。
///
/// 回归：`handle_key` 没消费 `Command` 组合，macOS 会先把同一条 chord 交给视图
/// （performKeyEquivalent），再交给原生菜单 → 按一次命令执行两遍：Cmd-D 分两个
/// 窗格、Cmd-, 打开设置后又被菜单那条 `OpenSettings` 当取消关掉（一闪消失）。
#[gpui::test]
fn command_shortcuts_are_consumed_before_the_native_menu(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::utils::corebridge::ClientTask;
    let (view, cx) = mount(two_tab_store(), cx);
    cx.simulate_keystrokes("cmd-d");
    let (splits, consumed) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let splits = app
                .take_commands_for_tests()
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        ClientCommand::Task {
                            task: ClientTask::SplitPane { .. },
                            ..
                        }
                    )
                })
                .count();
            (splits, app.take_key_consumed_for_tests())
        })
    });
    assert_eq!(splits, 1, "一次按键只能分一次屏");
    assert!(
        consumed,
        "带 Command 的动作必须 stop_propagation，否则原生菜单会再执行一遍"
    );
}

/// Cmd-, 打开设置后必须一直开着（回归：菜单那条 OpenSettings 立刻把它关掉）。
#[gpui::test]
fn cmd_comma_opens_settings_and_keeps_them_open(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.simulate_keystrokes("cmd-,");
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)),
        "Cmd-, 要能打开设置"
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.pump_once(cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)),
        "打开的设置不能被自己关掉"
    );
}

/// 聚合投影里 Cmd-1..N 切的是**页面**，不是当前 workspace 的 tab id。
///
/// 回归：`SwitchTab(n)` 以前拿 `self.tabs()`（当前 workspace 的真实 tab id）去
/// 选页，Agents / Shells 里第 2 页之后按数字要么跳错、要么完全不动。
#[gpui::test]
fn aggregates_switch_pages_with_cmd_digits(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::AggregateKind;
    let mut store = ViewStore::default();
    for (id, name, tab, pane) in [
        ("local//shell/tmux/", "one", 1, 10),
        ("local//shell2/tmux/", "two", 2, 20),
    ] {
        store.replace_topology(
            ClientWorkspace {
                id: id.to_string(),
                name: name.to_string(),
                runtime: "shell".to_string(),
                active: tab == 1,
                resolved_target: None,
            },
            vec![ClientTab {
                id: tab,
                name: name.to_string(),
                is_active: true,
            }],
            vec![(
                tab,
                vec![ClientPane {
                    id: pane,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: name.to_string(),
                }],
            )],
        );
        store.ensure_workspace(id).backend_status = Some(backend_status::CONNECTED);
    }
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.select_aggregate(AggregateKind::Shells, cx);
        })
    });
    let pages = cx.update(|_, cx| view.update(cx, |app, _| app.aggregate_pages().len()));
    assert_eq!(pages, 2, "两个本机 shell 应该是两页");

    cx.simulate_keystrokes("cmd-2");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(
        visible.as_deref(),
        Some("local//shell2/tmux/"),
        "Cmd-2 必须切到第 2 页"
    );

    cx.simulate_keystrokes("cmd-1");
    let visible =
        cx.update(|_, cx| view.update(cx, |app, _| app.visible_workspace().map(str::to_string)));
    assert_eq!(visible.as_deref(), Some("local//shell/tmux/"));
}

/// 分屏后：活动 pane 正常，非活动 pane 压暗；单 pane 不压。
///
/// 对照 macOS `InactivePaneDimmingPolicy`（iTerm2 `SplitPaneDimmingAmount` = 0.15）。
#[gpui::test]
fn inactive_panes_are_dimmed_when_a_tab_is_split(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let mut store = store_with_two_workspaces();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![ClientTab {
            id: 1,
            name: "dev".to_string(),
            is_active: true,
        }],
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
                1,
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
        vec![(
            1,
            ClientLayout::Split {
                first: Box::new(ClientLayout::Leaf { pane_id: 10 }),
                second: Box::new(ClientLayout::Leaf { pane_id: 11 }),
                horizontal: true,
                ratio: 5,
            },
        )],
    );
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests("local@@dev@tmux@dev", 11, 80, 24, b"other", cx);
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("pane-dimmed").is_some(),
        "非活动 pane 必须压暗"
    );
}

/// 命令面板的查询框要有可见光标；`Cmd-A` 要真的全选（以前是清空）。
#[gpui::test]
fn palette_query_shows_a_caret_and_cmd_a_selects_all(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("input-caret").is_some(),
        "查询框必须有可见光标"
    );
    cx.simulate_keystrokes("a b c");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_query_for_tests())),
        "abc"
    );
    // Cmd-A 全选：查询串保留，选区非空。
    cx.simulate_keystrokes("cmd-a");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_query_for_tests())),
        "abc",
        "Cmd-A 是全选，不能清空查询"
    );
    assert!(
        cx.debug_bounds("input-selection").is_some(),
        "全选后要看见选区高亮"
    );
    // 直接打字替换整个选区。
    cx.simulate_keystrokes("x");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_query_for_tests())),
        "x"
    );
}

/// 统一面板（Cmd-P）的查询框同样有光标、Cmd-A、方向键。
#[gpui::test]
fn panel_query_supports_selection_and_caret(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("input-caret").is_some(),
        "面板查询框必须有可见光标"
    );
    cx.simulate_keystrokes("d e v");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_query())),
        "dev"
    );
    cx.simulate_keystrokes("left");
    cx.simulate_keystrokes("x");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_query())),
        "dexv",
        "方向键把光标移到中间再插入"
    );
    cx.simulate_keystrokes("cmd-a");
    cx.run_until_parked();
    assert!(cx.debug_bounds("input-selection").is_some());
    cx.simulate_keystrokes("backspace");
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_query())),
        "",
        "有选区时退格删掉整个选区"
    );
}

/// 用户报障（截图 2）：`Cmd-P` 的输入框光标跑到默认内容末尾。
///
/// 空输入框里光标必须在**行首**，占位文案浅色跟在它后面；光标不能跑到
/// 占位文字右边（那样看着像已经输入了一段内容）。
#[gpui::test]
fn the_panel_caret_sits_before_the_placeholder(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    let caret = cx.debug_bounds("input-caret").expect("查询框必须有光标");
    let placeholder = cx
        .debug_bounds("input-placeholder")
        .expect("空输入框必须有占位文案");
    assert!(
        f32::from(caret.origin.x) < f32::from(placeholder.origin.x),
        "光标必须在占位文案前面（左），实际 caret.x={} placeholder.x={}",
        f32::from(caret.origin.x),
        f32::from(placeholder.origin.x)
    );
    assert!(
        (f32::from(placeholder.origin.x) - f32::from(caret.origin.x) - 1.5).abs() < 2.0,
        "光标和占位文案之间不能有肉眼可见的空隙"
    );
    let _ = view;
}

/// 用户报障：「复制图片粘贴仍然没反应」。
///
/// 终端里的 `Cmd-V` 以前只读剪贴板文本，剪贴板里是截图时这次粘贴什么
/// 都不发生（Core 侧连一条日志都没有）。现在必须走图片通路。
#[gpui::test]
fn cmd_v_pastes_a_clipboard_image(cx: &mut TestAppContext) {
    use gpui::{ClipboardItem, Image, ImageFormat};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.run_until_parked();
    // 用真 PNG：`to_png` 对 PNG 原样透传，测试不依赖 AppKit 解码
    // （AppKit 解无效/畸形图会卡十几秒，把横幅的展示期耗光）。
    let image = Image::from_bytes(
        ImageFormat::Png,
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0],
    );
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_image(&image)));
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("image-paste-banner").is_some(),
        "Cmd-V 遇到图片必须走图片粘贴（横幅是用户唯一能看到的反馈）"
    );
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::Input { .. })),
        "图片粘贴不能顺手把字节写进终端：{commands:?}"
    );
}

/// 用户报障（截图 1）：侧栏宽度写死不能拖。
///
/// 分隔条必须存在、可拖、并有上下限；拖动后正文 origin 跟着变（否则点击
/// 命中和画出来的位置会错位）。
#[gpui::test]
fn dragging_the_sidebar_divider_resizes_the_sidebar(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::model::sidebar_layout::{SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.run_until_parked();
    let divider = cx.debug_bounds("sidebar-divider").expect("分隔条必须存在");
    let grab = divider.center();
    cx.simulate_mouse_down(grab, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(
        gpui::point(px(360.0), grab.y),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let width = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_width_for_tests()));
    assert!(
        (width - 360.0).abs() < 2.0,
        "拖到 360pt 时侧栏就该是 360pt，实际 {width}"
    );
    // 拖过上限也不能超出 Swift 的 480pt。
    cx.simulate_mouse_move(
        gpui::point(px(900.0), grab.y),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let width = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_width_for_tests()));
    assert_eq!(width, SIDEBAR_MAX_WIDTH);
    cx.simulate_mouse_up(
        gpui::point(px(900.0), grab.y),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let dragging = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_dragging_for_tests()));
    assert!(!dragging, "松手后必须结束拖拽");
    // 侧栏真的变宽了（正文 origin 跟着走）。
    let sidebar = cx.debug_bounds("sidebar").expect("侧栏必须渲染");
    assert_eq!(f32::from(sidebar.size.width), SIDEBAR_MAX_WIDTH);
    assert!(SIDEBAR_MIN_WIDTH < SIDEBAR_MAX_WIDTH);
}

/// 用户报障（截图 1）：四段挤在同一个滚动区。
///
/// 现在 Workspaces / Agents / Commands 各自成区，Hidden Commands 固定贴底。
#[gpui::test]
fn the_sidebar_keeps_its_sections_in_separate_bands(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.run_until_parked();
    let workspaces = cx
        .debug_bounds("sidebar-scroll-Workspaces")
        .expect("上半区必须渲染");
    let agents = cx
        .debug_bounds("sidebar-scroll-Agents")
        .expect("Agents 区必须渲染");
    let commands = cx
        .debug_bounds("sidebar-scroll-Commands")
        .expect("Commands 区必须渲染");
    assert!(
        f32::from(workspaces.origin.y) < f32::from(agents.origin.y),
        "Workspaces 在上半区"
    );
    assert!(
        f32::from(agents.origin.y) < f32::from(commands.origin.y),
        "Agents 在 Commands 之上"
    );
    assert!(
        f32::from(workspaces.size.height) > f32::from(agents.size.height),
        "Workspaces 要比 Agents 高"
    );
    assert!(
        f32::from(agents.size.height) > f32::from(commands.size.height),
        "Agents 要比 Commands 高（用户明确要求）"
    );
    let _ = view;
}

/// 用户报障（截图 3）：tab 栏应该默认等宽铺满，并用颜色区分运行/完成。
///
/// 对照 Swift `StatusBarView.swift` 的 `fillsViewport`：`equal_width` 时
/// `tabStack.distribution = .fillEqually` 且 `tabStackTrailingConstraint` 激活。
#[gpui::test]
fn tabs_fill_the_bar_equally_by_default(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::tab_bar_view::TabBarStyle;
    let (view, cx) = mount(two_tab_store(), cx);
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.tab_bar_style())),
        TabBarStyle::EqualWidth,
        "缺省必须是等宽铺满"
    );
    let mut ids = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.tab_items()
                .into_iter()
                .map(|item| item.tab_id)
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(ids.len(), 2, "这个 store 有两个 tab：{ids:?}");
    ids.sort_unstable();
    let _ = &ids;
    // `debug_bounds` 要求 `&'static str`：两个 id 只有几种可能，直接枚举。
    let first = cx.debug_bounds("tab-1").expect("第一个 tab");
    let second = cx.debug_bounds("tab-2").expect("第二个 tab");
    let strip = cx.debug_bounds("tab-strip").expect("tab 带必须渲染");
    // 等宽：两个 tab 宽度一致（允许亚像素差）。
    assert!(
        (f32::from(first.size.width) - f32::from(second.size.width)).abs() < 1.0,
        "等宽样式下每个 tab 应一样宽：{} vs {}",
        f32::from(first.size.width),
        f32::from(second.size.width)
    );
    // 铺满：两个 tab + 2pt 间距应接近整条带子的宽度。
    let used = f32::from(first.size.width) + f32::from(second.size.width) + 2.0;
    assert!(
        (f32::from(strip.size.width) - used).abs() < 3.0,
        "tab 必须铺满整条带子：strip={} used={used}",
        f32::from(strip.size.width)
    );
}

/// 用户报障（截图 3）：tab 要用颜色区分运行中 / 完成。
#[gpui::test]
fn tab_colours_follow_the_activity_state(_cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::tab_bar::tab_state_color;
    use muxterm::frontend::gpui::chrome::tab_bar_view::TabBarItem;
    use muxterm::frontend::gpui::theme::Theme;
    let theme = Theme::default();
    let base = TabBarItem {
        tab_id: 1,
        index: 1,
        title: "zsh".into(),
        active: false,
        pane_count: 1,
        activity: None,
        identity: None,
    };
    assert_eq!(tab_state_color(&base, &theme), theme.muted, "idle 不点亮");
    for (indicator, expected) in [
        (Indicator::Working, theme.working),
        (Indicator::Blocked, theme.blocked),
        (Indicator::Done, theme.done),
    ] {
        let item = TabBarItem {
            activity: Some(indicator),
            ..base.clone()
        };
        assert_eq!(
            tab_state_color(&item, &theme),
            expected,
            "{indicator:?} 必须用对应的状态色"
        );
        assert_ne!(expected, theme.muted, "状态色不能和 idle 一样");
    }
}

/// IME 提交要落到当前吃文字的那个输入框，而不是永远当成终端输入。
///
/// 回归：`commit_text` 只判断 `panel.open`，命令面板 / 设置里用中文输入法
/// 提交的字会直接写进 PTY。
#[gpui::test]
fn ime_commit_follows_the_focused_input(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));

    // 命令面板打开：提交的中文进查询串，不落 PTY。
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_marked_text("zhong", cx);
            app.commit_text("中文", cx);
        })
    });
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::Input { .. })),
        "命令面板里的 IME 提交不能写进终端：{commands:?}"
    );
    assert_eq!(
        cx.update(|_, cx| view.update(cx, |app, _| app.palette_query_for_tests())),
        "中文"
    );

    // 关掉面板：同样的提交回到终端。
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| view.update(cx, |app, cx| app.commit_text("中文", cx)));
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Input { data, .. } if data == "中文".as_bytes()
        )),
        "终端里的 IME 提交必须写进 PTY：{commands:?}"
    );
}

/// 回归（中文输入）：拼音首字母必须先交给平台输入法，再由提交路径进 PTY。
///
/// 旧实现把 `n` 直接编码进 PTY 并 `stop_propagation()`，macOS 的
/// `inputContext` 收不到这次 keyDown，输入法根本无法开始组词。
/// 对照 macOS `MuxTerminalView.keyDown` 的 `interpretKeyEvents`。
#[gpui::test]
fn pinyin_keys_defer_to_the_ime_and_commit_once(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    let take_inputs = |view: &gpui::Entity<GpuiApp>, cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.take_commands_for_tests()
                    .into_iter()
                    .filter_map(|command| match command {
                        ClientCommand::Input { data, .. } => Some(data),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
        })
    };
    let _ = take_inputs(&view, cx);

    // 未组词时的首字母：按键让给平台，但文本仍只提交一次。
    cx.simulate_keystrokes("n");
    let deferred = cx.update(|_, cx| view.update(cx, |app, _| app.take_ime_deferred_for_tests()));
    assert!(deferred, "拼音首字母必须让给输入法，否则组词起不来");
    assert_eq!(
        take_inputs(&view, cx),
        vec![b"n".to_vec()],
        "让给输入法之后仍然要经 `insertText` 提交一次，不能丢字也不能重复"
    );

    // 输入法提交中文：`insertText` → `replace_text_in_range` 的同一路径。
    cx.simulate_input("你");
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.take_ime_deferred_for_tests())),
        "提交的中文同样要经过输入法边界"
    );
    assert_eq!(take_inputs(&view, cx), vec!["你".as_bytes().to_vec()]);

    // 特殊键不能被让给输入法：回车直接编码。
    cx.simulate_keystrokes("enter");
    let deferred = cx.update(|_, cx| view.update(cx, |app, _| app.take_ime_deferred_for_tests()));
    assert!(!deferred, "回车必须直接编码，不能送进输入法");
    assert_eq!(take_inputs(&view, cx), vec![b"\r".to_vec()]);
}

/// 回归（shell 退出）：远端 workspace 的后端 `Exited` 后，冻住的 pane 必须收起，
/// 并切到邻近还活着的 workspace；不能停在最后一帧让键盘「卡住」。
///
/// 对照 Swift `maybeCloseIfSessionEnded()`：其它 workspace 还有 pane 时关掉当前格。
#[gpui::test]
fn an_ended_remote_workspace_switches_to_the_next_live_one(cx: &mut TestAppContext) {
    let mut store = store_with_two_workspaces();
    store.ensure_workspace("local@@dev@tmux@dev").backend_status = Some(backend_status::EXITED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.take_commands_for_tests();
            app.pump_once(cx);
        })
    });

    let (visible, sidebar, quit) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.visible_workspace().map(str::to_string),
                app.sidebar_items(),
                app.quit_requested(),
            )
        })
    });
    assert_eq!(
        visible.as_deref(),
        Some("local@@ops@tmux@ops"),
        "必须切到邻近还活着的 workspace"
    );
    assert!(!quit, "还有活 workspace 时不能退出窗口");
    assert_eq!(
        sidebar.len(),
        3,
        "死掉的 workspace 不能再占侧栏行：{:?}",
        sidebar.iter().map(|item| &item.name).collect::<Vec<_>>()
    );
    assert!(
        sidebar.iter().all(|item| item.name != "dev"),
        "结束的 workspace 必须从侧栏移除"
    );
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::ActivateWorkspace(id) if id == "local@@ops@tmux@ops"
        )),
        "切 workspace 必须告诉 Core：{commands:?}"
    );
}

/// 回归（shell 退出）：最后一个 workspace 也结束时，不能留一个空白、无响应的页；
/// 按 Swift `closeSessionWindow` 请求关窗口。
#[gpui::test]
fn the_last_ended_workspace_asks_the_window_to_close(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    store.replace_topology(
        workspace("local@@only@tmux@only", "only", true),
        vec![ClientTab {
            id: 1,
            name: "only".into(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 10,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "only".into(),
            }],
        )],
    );
    store
        .ensure_workspace("local@@only@tmux@only")
        .backend_status = Some(backend_status::EXITED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| view.update(cx, |app, cx| app.pump_once(cx)));
    let (visible, tabs, quit) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.visible_workspace().map(str::to_string),
                app.tab_items().len(),
                app.quit_requested(),
            )
        })
    });
    assert_eq!(visible, None, "死掉的 workspace 不能再当可见页");
    assert_eq!(tabs, 0, "僵尸 tab 必须退出");
    assert!(quit, "没有活 workspace 时要请求关窗口（不能卡在空白页）");
}

/// 回归（pane 关闭）：拓扑里没有的 pane，它的 Surface 与选区必须一起丢掉。
///
/// 否则关掉的 pane 还会拿着旧画面、旧选区，`Cmd-C` 也可能复制到已经不存在的 pane。
#[gpui::test]
fn closed_panes_drop_their_surface_and_selection(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests("local@@dev@tmux@dev", 10, 40, 6, b"hello", cx);
            app.begin_selection(10, 0, 0, 1, cx);
            app.extend_selection(10, 0, 4, cx);
        })
    });
    assert!(cx.update(|_, cx| view.update(cx, |app, _| app.selection_for(10).is_some())));

    // 拓扑更新：pane 10 被关掉，换成 pane 99。
    let mut next = store_with_two_workspaces();
    next.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![ClientTab {
            id: 1,
            name: "dev".into(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 99,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "dev".into(),
            }],
        )],
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.replace_store_for_tests(next, cx);
            app.pump_once(cx);
        })
    });
    let (selection, screen) =
        cx.update(|_, cx| view.update(cx, |app, _| (app.selection_for(10), app.pane_screen(10))));
    assert_eq!(selection, None, "关掉的 pane 不能留着选区");
    assert_eq!(screen, None, "关掉的 pane 不能留着 Surface");
}

/// 回归（关闭 workspace）：Core 不会为 close 发 topology 事件，前端必须像
/// Swift `closeWorkspace` 一样先本地收起并切到邻近 workspace，不能等一个永远
/// 不来的回包（用户看到的就是「关闭 workspace 失败」）。
#[gpui::test]
fn closing_the_visible_workspace_switches_before_core_replies(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.take_commands_for_tests();
            app.close_workspace("local@@dev@tmux@dev".into(), cx);
        })
    });
    let (visible, sidebar, quit) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.visible_workspace().map(str::to_string),
                app.sidebar_items(),
                app.quit_requested(),
            )
        })
    });
    assert_eq!(
        visible.as_deref(),
        Some("local@@ops@tmux@ops"),
        "关掉可见 workspace 后必须立刻切到邻近那个"
    );
    assert!(!quit, "还有活 workspace 时不能退出窗口");
    assert!(
        sidebar.iter().all(|item| item.name != "dev"),
        "关掉的 workspace 不能留在侧栏：{:?}",
        sidebar.iter().map(|item| &item.name).collect::<Vec<_>>()
    );
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::CloseWorkspace(id) if id == "local@@dev@tmux@dev"
        )),
        "本地收起后仍要告诉 Core 关掉它：{commands:?}"
    );
}

/// 回归（关闭 workspace）：关最后一个 workspace 走关窗，不留空白页。
#[gpui::test]
fn closing_the_last_workspace_requests_the_window_to_close(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    store.replace_topology(
        workspace("local@@only@tmux@only", "only", true),
        vec![ClientTab {
            id: 1,
            name: "only".into(),
            is_active: true,
        }],
        vec![(
            1,
            vec![ClientPane {
                id: 10,
                cols: 80,
                rows: 24,
                is_active: true,
                title: "only".into(),
            }],
        )],
    );
    store
        .ensure_workspace("local@@only@tmux@only")
        .backend_status = Some(backend_status::CONNECTED);
    let (view, cx) = mount(store, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.take_commands_for_tests();
            app.close_workspace("local@@only@tmux@only".into(), cx);
        })
    });
    let (visible, tabs, quit, commands) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.visible_workspace().map(str::to_string),
                app.tab_items().len(),
                app.quit_requested(),
                app.take_commands_for_tests(),
            )
        })
    });
    assert_eq!(visible, None, "关掉的 workspace 不能再是可见页");
    assert_eq!(tabs, 0, "僵尸 tab 必须退出");
    assert!(quit, "最后一个 workspace 关掉后要请求关窗");
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::CloseWorkspace(_))),
        "最后一个 workspace 走关窗路径，不再发 close：{commands:?}"
    );
}

/// 关闭后台 workspace 不能动当前可见页。
#[gpui::test]
fn closing_a_background_workspace_keeps_the_visible_one(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.take_commands_for_tests();
            app.close_workspace("local@@ops@tmux@ops".into(), cx);
        })
    });
    let (visible, rows) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.visible_workspace().map(str::to_string),
                app.sidebar_items().len(),
            )
        })
    });
    assert_eq!(
        visible.as_deref(),
        Some("local@@dev@tmux@dev"),
        "关后台 workspace 不能切走当前页"
    );
    assert_eq!(rows, 3, "只少一行：两个聚合槽 + dev");
}

/// 本机 shell 是聚合入口的锚点，永远不能被关闭。
#[gpui::test]
fn the_local_shell_workspace_is_not_closable(cx: &mut TestAppContext) {
    let mut store = ViewStore::default();
    for (id, name, runtime) in [("local///shell/", "Local Shell", "shell")] {
        store.replace_topology(
            ClientWorkspace {
                id: id.into(),
                name: name.into(),
                runtime: runtime.into(),
                active: true,
                resolved_target: None,
            },
            vec![ClientTab {
                id: 1,
                name: name.into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 10,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: name.into(),
                }],
            )],
        );
        store.ensure_workspace(id).backend_status = Some(backend_status::CONNECTED);
    }
    let (view, cx) = mount(store, cx);
    let (visible, rows, commands) = cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.take_commands_for_tests();
            app.close_workspace("local///shell/".into(), cx);
            (
                app.visible_workspace().map(str::to_string),
                app.sidebar_items().len(),
                app.take_commands_for_tests(),
            )
        })
    });
    assert_eq!(
        visible.as_deref(),
        Some("local///shell/"),
        "本机 shell 不能关"
    );
    assert_eq!(rows, 2, "侧栏仍只有两个聚合槽");
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, ClientCommand::CloseWorkspace(_))),
        "本机 shell 不能向 Core 发 close：{commands:?}"
    );
}

/// Cmd-D / Cmd-S 各分一次屏；两次按键 = 两次分割（不是一次按键分两个）。
#[gpui::test]
fn cmd_d_and_cmd_s_split_once_each(cx: &mut TestAppContext) {
    use muxterm::frontend::command_queue::ClientCommand;
    use muxterm::frontend::utils::corebridge::ClientTask;
    let (view, cx) = mount(two_tab_store(), cx);
    let take_splits =
        |view: &gpui::Entity<GpuiApp>, cx: &mut gpui::VisualTestContext| -> Vec<bool> {
            cx.update(|_, cx| {
                view.update(cx, |app, _| {
                    app.take_commands_for_tests()
                        .iter()
                        .filter_map(|command| match command {
                            ClientCommand::Task {
                                task: ClientTask::SplitPane { horizontal, .. },
                                ..
                            } => Some(*horizontal),
                            _ => None,
                        })
                        .collect()
                })
            })
        };
    cx.simulate_keystrokes("cmd-d");
    assert_eq!(
        take_splits(&view, cx),
        vec![true],
        "Cmd-D 一次只分一次（左右）"
    );
    cx.simulate_keystrokes("cmd-s");
    assert_eq!(
        take_splits(&view, cx),
        vec![false],
        "Cmd-S 一次只分一次（上下）"
    );
}

/// 分屏后每个 pane 上方出现标题条；单 pane 时不占位（Swift
/// `PaneTitleLayoutPolicy.showsTitleBar`）。
#[gpui::test]
fn pane_title_bar_appears_only_when_split(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("pane-title-10").is_none(),
        "单 pane 不画标题条"
    );

    let mut store = store_with_two_workspaces();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![ClientTab {
            id: 1,
            name: "dev".to_string(),
            is_active: true,
        }],
        // 同一个 tab 的 pane 必须放在一条记录里：重复的 tab id 会覆盖前一条，
        // pane 10 就不见了（旧夹具的坑）。
        vec![(
            1,
            vec![
                ClientPane {
                    id: 10,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "zsh".to_string(),
                },
                ClientPane {
                    id: 11,
                    cols: 80,
                    rows: 24,
                    is_active: false,
                    title: "server".to_string(),
                },
            ],
        )],
    );
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![(
            1,
            ClientLayout::Split {
                horizontal: true,
                // 千分比：一半一半。旧夹具写 5（=0.5%），左 pane 只有 5px 宽。
                ratio: 500,
                first: Box::new(ClientLayout::Leaf { pane_id: 10 }),
                second: Box::new(ClientLayout::Leaf { pane_id: 11 }),
            },
        )],
    );
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.replace_store_for_tests(store, cx);
        })
    });
    cx.run_until_parked();
    // 对照 Swift `updateTitleHover`：分屏后标题条仍然默认隐藏，
    // 鼠标停在 pane 顶部 22pt 才出现。
    assert!(
        cx.debug_bounds("pane-title-10").is_none(),
        "鼠标不在顶部时标题条隐藏"
    );
    // 侧栏 240pt，内容区从 x=240 开始；第一个 pane 的标题条带在 y<22。
    cx.simulate_mouse_move(
        gpui::point(px(300.0), px(6.0)),
        None,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("pane-title-10").is_some(),
        "分屏后悬停顶部就要有标题条"
    );
    assert!(
        cx.debug_bounds("pane-title-11").is_none(),
        "只显示鼠标所在 pane 的标题条"
    );
}

/// 用户报障：点 `⋯` 却新建了 pane。入口必须打开**该 pane** 的菜单，
/// 且菜单动作作用在这个 pane 上（即使焦点原本在另一个 pane）。
#[gpui::test]
fn the_pane_menu_opens_on_the_clicked_pane(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let (view, cx) = mount(split_store(), cx);
    cx.run_until_parked();
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));

    // 让 pane 11 的标题条出现（鼠标路径已在上一条测试里覆盖）。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| app.note_pane_hover(11, true, cx));
    });
    cx.run_until_parked();
    let actions = cx
        .debug_bounds("pane-actions-11")
        .expect("pane 11 action button");
    cx.simulate_mouse_down(
        actions.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    assert!(cx.debug_bounds("pane-menu").is_some(), "点 `⋯` 要打开菜单");
    let opened = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        !opened.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SplitPane { .. },
                ..
            }
        )),
        "打开菜单不能直接分屏：{opened:?}"
    );

    // 菜单里的 Split Horizontal 作用在 pane 11，而不是当前焦点的 pane 10。
    let split = cx
        .debug_bounds("pane-menu-split-horizontal")
        .expect("split horizontal row");
    cx.simulate_mouse_down(
        split.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let split_target = commands.iter().find_map(|command| match command {
        ClientCommand::Task {
            task:
                ClientTask::SplitPane {
                    pane_id,
                    horizontal,
                },
            ..
        } => Some((*pane_id, *horizontal)),
        _ => None,
    });
    assert_eq!(split_target, Some((11, true)), "命令：{commands:?}");
    assert!(cx.debug_bounds("pane-menu").is_none(), "点完动作要关菜单");
}

/// Esc 关菜单，且菜单开着时不把按键漏进终端。
#[gpui::test]
fn escape_closes_the_pane_menu(cx: &mut TestAppContext) {
    let (view, cx) = mount(split_store(), cx);
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| app.note_pane_hover(10, true, cx));
    });
    cx.run_until_parked();
    let actions = cx
        .debug_bounds("pane-actions-10")
        .expect("pane 10 action button");
    cx.simulate_mouse_down(
        actions.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(cx.debug_bounds("pane-menu").is_some());
    cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("pane-menu").is_none(), "Esc 关闭 pane 菜单");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(commands.is_empty(), "Esc 不能分发动作：{commands:?}");
}

/// 两个 pane（10 = active、11 = inactive）的水平分割工作区。
fn split_store() -> ViewStore {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    let mut store = store_with_two_workspaces();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![ClientTab {
            id: 1,
            name: "dev".to_string(),
            is_active: true,
        }],
        vec![(
            1,
            vec![
                ClientPane {
                    id: 10,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "zsh".to_string(),
                },
                ClientPane {
                    id: 11,
                    cols: 80,
                    rows: 24,
                    is_active: false,
                    title: "server".to_string(),
                },
            ],
        )],
    );
    store.replace_layouts(
        "local@@dev@tmux@dev",
        vec![(
            1,
            ClientLayout::Split {
                horizontal: true,
                ratio: 400,
                first: Box::new(ClientLayout::Leaf { pane_id: 10 }),
                second: Box::new(ClientLayout::Leaf { pane_id: 11 }),
            },
        )],
    );
    store
}

/// 图片粘贴走 Core：前端只渲染阶段横幅，失败/成功都由 Core 的结果驱动。
#[gpui::test]
fn image_paste_banner_follows_the_core_stage(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::model::image_paste::ImagePasteStage;
    let (view, cx) = mount(two_tab_store(), cx);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("image-paste-banner").is_none(),
        "空闲时不画横幅"
    );

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_image_paste_stage_for_tests(ImagePasteStage::Uploading);
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("image-paste-banner").is_some(),
        "上传中要有横幅"
    );

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_image_paste_stage_for_tests(ImagePasteStage::Failed("boom".into()));
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("image-paste-banner").is_some(),
        "失败也要留着"
    );
}

/// 聚合槽是固定入口：颜色用专属身份色，不借用 blocked（粉）/ done（青绿）。
#[gpui::test]
fn aggregate_rows_use_their_identity_colors(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::model::aggregate::AggregateIdentity;
    let (view, _cx) = mount(two_tab_store(), cx);
    cx.update(|cx| {
        view.update(cx, |app, _| {
            let rows = app.sidebar_model();
            let workspaces = rows
                .iter()
                .find(|section| format!("{:?}", section.section) == "Workspaces")
                .expect("workspaces section");
            let shells = workspaces
                .rows
                .iter()
                .find(|row| row.identity == Some(AggregateIdentity::Shells))
                .expect("shells aggregate row");
            let agents = workspaces
                .rows
                .iter()
                .find(|row| row.identity == Some(AggregateIdentity::Agents))
                .expect("agents aggregate row");
            assert_eq!(shells.identity.unwrap().token(), "teal");
            assert_eq!(agents.identity.unwrap().token(), "purple");
            // 聚合行不是连接：永远不应被标成 blocked 的粉。
            assert_eq!(shells.detail_alert, false);
            assert_eq!(agents.detail_alert, false);
        })
    });
}

/// 设置行不能把控件挤出卡片：右侧控件的右边缘必须落在面板内。
///
/// 之前左侧描述没有 `min_w(0)`，长文案把 200pt 的控件推到面板外，
/// 下拉框与 Apply 状态整个跑到卡片右边（截图上直接被裁掉）。
#[gpui::test]
fn settings_controls_stay_inside_the_panel(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-,");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "font": { "size": 18.0 } }),
                cx,
            );
            app.settings_select("ui", cx);
        })
    });
    cx.run_until_parked();

    let panel = cx.debug_bounds("settings-panel").expect("settings panel");
    let panel_right = f32::from(panel.origin.x) + f32::from(panel.size.width);
    let control = cx
        .debug_bounds("settings-control-/ui/show_title_bar")
        .expect("show_title_bar control");
    let control_right = f32::from(control.origin.x) + f32::from(control.size.width);
    assert!(
        control_right <= panel_right,
        "控件右边缘 {control_right} 落在面板 {panel_right} 外面"
    );
    // 描述最长的几行也要检查：之前正是它们把控件推出面板。
    for selector in [
        "settings-control-/ui/show_title_bar",
        "settings-control-/ui/language",
        "settings-control-/ui/tab_bar_position",
    ] {
        let bounds = cx.debug_bounds(selector).expect(selector);
        let right = f32::from(bounds.origin.x) + f32::from(bounds.size.width);
        assert!(
            right <= panel_right,
            "{selector} 的控件越界：{right} > {panel_right}"
        );
    }
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.settings_select("appearance", cx);
        })
    });
    cx.run_until_parked();
    let control = cx
        .debug_bounds("settings-control-/font/family")
        .expect("family control");
    let right = f32::from(control.origin.x) + f32::from(control.size.width);
    assert!(
        right <= panel_right,
        "长描述把字体控件推到了面板外：{right}"
    );
}

/// 面板查询必须真的过滤：输入 "dev" 后只剩名字/路径匹配的行。
#[gpui::test]
fn panel_query_filters_every_row(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    let before = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows().len()));
    assert!(before > 0);

    cx.simulate_keystrokes("z z z z");
    cx.run_until_parked();
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    for row in &rows {
        let text = format!("{} {:?}", row.title(), row.detail());
        assert!(
            text.to_lowercase().contains("zzzz"),
            "不匹配的行没被过滤掉：{text}"
        );
    }
}

/// Search tab 的查询要打到 Core：注入快照后输入关键字只剩匹配的命中行。
#[gpui::test]
fn search_tab_filters_core_hits(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_search_snapshot(Some(
                muxterm::frontend::gpui::model::search::SearchSnapshot::decode(
                    br#"{"ok":true,"hits":[
                        {"workspace_id":"local@@dev@tmux@dev","tab_id":1,"pane_id":10,"seq":4,"line":"cargo test ok"},
                        {"workspace_id":"local@@dev@tmux@dev","tab_id":1,"pane_id":11,"seq":5,"line":"vim notes"}
                    ]}"#,
                )
                .unwrap(),
            ));
        })
    });
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_keystrokes("v i m");
    cx.run_until_parked();
    let rows = cx.update(|_, cx| view.update(cx, |app, _| app.panel_rows()));
    assert_eq!(rows.len(), 1, "只剩含 vim 的那条命中");
}

/// 编号 / 字母必须紧挨标记列，而不是飘在行尾（Swift `shortcutLabel` 的位置）。
#[gpui::test]
fn sidebar_shortcuts_sit_next_to_the_marker(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.run_until_parked();
    let shortcut = cx
        .debug_bounds("sidebar-shortcut-dev")
        .expect("workspace 1 的编号");
    let title = cx
        .debug_bounds("sidebar-title-dev")
        .expect("workspace 1 的标题");
    let shortcut_right = f32::from(shortcut.origin.x) + f32::from(shortcut.size.width);
    assert!(
        shortcut_right <= f32::from(title.origin.x) + 1.0,
        "编号必须在标题左边：{shortcut_right} > {}",
        f32::from(title.origin.x)
    );
    let _ = view;
}

/// `[ui] show_title_bar` 关掉后窗口标题为空串（只剩 traffic lights）。
#[gpui::test]
fn window_title_follows_the_ui_toggle(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::state::window_title_text;
    assert_eq!(window_title_text(true, "dev"), "dev — Muxterm");
    assert_eq!(window_title_text(false, "dev"), "");
    assert_eq!(window_title_text(true, "  "), "Muxterm");

    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.simulate_keystrokes("cmd-,");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_settings(
                &settings_manifest(),
                &serde_json::json!({ "ui": { "show_title_bar": false } }),
                cx,
            );
            app.set_show_title_bar(false);
            cx.notify();
        })
    });
    cx.run_until_parked();
    let (show, applied) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.show_title_bar(), app.applied_window_title().to_string())
        })
    });
    assert!(!show, "开关要反映到状态");
    assert_eq!(applied, "", "关掉后原生窗口标题必须被清空");
}

/// Attention 行必须带状态色与状态徽标（之前整行全灰，看不出 working/blocked）。
#[gpui::test]
fn attention_rows_show_a_status_color(cx: &mut TestAppContext) {
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.set_attention(Some(
                muxterm::frontend::gpui::model::attention::AttentionSnapshot::decode(
                    br#"{"ok":true,"workspaces":[
                        {"workspace_id":"local@@dev@tmux@dev","name":"dev","panes":[
                            {"pane_id":10,"status":"working","agent":"pi","tab_id":1,"tab_index":1,"read":false}
                        ]}
                    ]}"#,
                )
                .expect("attention snapshot"),
            ));
        })
    });
    cx.simulate_keystrokes("cmd-p");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.show_panel(
                muxterm::frontend::gpui::model::panel::PanelTab::Attention,
                cx,
            );
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("panel-status-0").is_some(),
        "Attention 行要有状态徽标"
    );
}

/// 两 tab 的工作区：tab 1 有 pane 10，tab 2 有 pane 20（默认停 tab 1）。
fn two_tab_store_with_pane_20() -> ViewStore {
    let mut store = ViewStore::default();
    store.replace_topology(
        workspace("local@@dev@tmux@dev", "dev", true),
        vec![
            ClientTab {
                id: 1,
                name: "main".to_string(),
                is_active: true,
            },
            ClientTab {
                id: 2,
                name: "ops".to_string(),
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
                    title: "zsh".to_string(),
                }],
            ),
            (
                2,
                vec![ClientPane {
                    id: 20,
                    cols: 100,
                    rows: 30,
                    is_active: true,
                    title: "server".to_string(),
                }],
            ),
        ],
    );
    store.ensure_workspace("local@@dev@tmux@dev").backend_status = Some(backend_status::CONNECTED);
    store
}

/// 用户报障：点 Attention 行要跳到那个 pane 的**完整身份**（workspace+tab+pane），
/// 不能停在当前 tab 上。
#[gpui::test]
fn attention_rows_reveal_the_exact_tab_and_pane(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::model::attention::{PaneAttention, WorkspaceAttention};
    use muxterm::frontend::gpui::model::panel::PanelTab;
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_attention(Some(AttentionSnapshot {
                blocked_count: 1,
                workspaces: vec![WorkspaceAttention {
                    workspace_id: "local@@dev@tmux@dev".into(),
                    name: "dev".into(),
                    transport: "local".into(),
                    ..WorkspaceAttention::new(
                        "local@@dev@tmux@dev",
                        vec![PaneAttention {
                            pane_id: 20,
                            status: PaneAttentionStatus::Blocked,
                            last_line: "waiting on tests".into(),
                            seq: 9,
                            ..PaneAttention::default()
                        }],
                    )
                }],
            }));
            app.show_panel(PanelTab::Attention, cx);
            app.take_commands_for_tests();
        })
    });
    let index = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.panel_rows()
                .iter()
                .position(|row| {
                    matches!(
                        row,
                        muxterm::frontend::gpui::chrome::panel_view::PanelRow::Attention {
                            pane_id: 20,
                            ..
                        }
                    )
                })
                .expect("pane 20 attention row")
        })
    });
    cx.update(|_, cx| {
        view.update(cx, |app, cx| app.select_panel_row(index, cx));
    });
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let switched_tab = commands.iter().find_map(|command| match command {
        ClientCommand::Task {
            task: ClientTask::SwitchTab { tab_id },
            ..
        } => Some(*tab_id),
        _ => None,
    });
    let switched_pane = commands.iter().find_map(|command| match command {
        ClientCommand::Task {
            task: ClientTask::SwitchPane { pane_id },
            ..
        } => Some(*pane_id),
        _ => None,
    });
    assert_eq!(
        switched_tab,
        Some(2),
        "必须先切到 pane 20 所在的 tab：{commands:?}"
    );
    assert_eq!(switched_pane, Some(20), "命令：{commands:?}");
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.panel_open())) == false,
        "跳转后面板关闭"
    );
}

/// 搜索命中同理：跳到命中所在的 tab + pane。
#[gpui::test]
fn search_hits_reveal_the_exact_tab_and_pane(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::model::panel::PanelTab;
    use muxterm::frontend::gpui::model::search::SearchHit;
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_search_for_tests(SearchSnapshot {
                hits: vec![SearchHit {
                    workspace_id: "local@@dev@tmux@dev".into(),
                    tab_id: 2,
                    pane_id: 20,
                    seq: 3,
                    line: "cargo test".into(),
                }],
            });
            app.show_panel(PanelTab::Search, cx);
            app.take_commands_for_tests();
        })
    });
    let index = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.panel_rows()
                .iter()
                .position(|row| {
                    matches!(
                        row,
                        muxterm::frontend::gpui::chrome::panel_view::PanelRow::Search(_)
                    )
                })
                .expect("search row")
        })
    });
    cx.update(|_, cx| {
        view.update(cx, |app, cx| app.select_panel_row(index, cx));
    });
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SwitchTab { tab_id: 2 },
                ..
            }
        )),
        "搜索命中要切到 tab 2：{commands:?}"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Task {
                task: ClientTask::SwitchPane { pane_id: 20 },
                ..
            }
        )),
        "搜索命中要切到 pane 20：{commands:?}"
    );
}

/// 用户报障：状态栏一直显示打开时的 80x24。它必须读 Surface 的实时网格。
#[gpui::test]
fn status_bar_follows_the_live_pane_grid(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    let before = cx.update(|_, cx| view.update(cx, |app, _| app.pane_geometry_label()));
    assert!(before.contains("80x24"), "初始来自快照：{before}");

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests("local@@dev@tmux@dev", 10, 132, 40, b"live\r\n", cx);
        })
    });
    cx.run_until_parked();
    let after = cx.update(|_, cx| view.update(cx, |app, _| app.pane_geometry_label()));
    assert!(
        after.contains("132x40"),
        "窗口变大后状态栏必须跟着 Surface：{after}"
    );
}

/// 用户报障：复制图片粘贴失败时**什么提示都没有**。转不了码必须出横幅。
#[gpui::test]
fn an_unreadable_clipboard_image_reports_a_banner(cx: &mut TestAppContext) {
    use gpui::{ClipboardItem, Image, ImageFormat};
    let (view, cx) = mount(store_with_two_workspaces(), cx);
    cx.run_until_parked();
    // 空字节：`to_png` 在进 AppKit 之前就返回 None。不要用畸形的
    // 非空字节 —— AppKit 解码它会卡十几秒，横幅早过期了。
    let image = Image::from_bytes(ImageFormat::Jpeg, Vec::new());
    let item = ClipboardItem::new_image(&image);
    cx.update(|_, cx| cx.write_to_clipboard(item));
    let handled = cx.update(|_, cx| view.update(cx, |app, cx| app.paste_clipboard_image(cx)));
    assert!(handled, "图片粘贴要吃掉这次 Cmd-V");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("image-paste-banner").is_some(),
        "失败必须在横幅上写出来（用户看不到就等于没提示）"
    );
}

/// 用户报障「终端一片空白」的真根因：本地 shell 可能在拓扑快照回来之前就
/// 开始输出。drain 不能只看 `view.panes`，否则这段输出永远没人取。
#[gpui::test]
fn early_pane_output_reaches_the_surface(cx: &mut TestAppContext) {
    use muxterm::frontend::utils::corebridge::{ClientEvent, ClientWorkspaceEvent};
    let mut store = ViewStore::default();
    // 只有 workspace 行，没有 pane 列表（模拟 workspace_list 还没回来）。
    store.replace_topology(workspace("local///shell/", "local", true), vec![], vec![]);
    let (view, cx) = mount(store, cx);
    cx.run_until_parked();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.store_for_tests_event(ClientWorkspaceEvent {
                workspace_id: "local///shell/".into(),
                event: ClientEvent {
                    type_: 0, // STATE_PANE_OUTPUT
                    pane_id: 1,
                    tab_id: 1,
                    window_id: 0,
                    data: b"early output\r\n".to_vec(),
                    name: String::new(),
                },
            });
            let dirty = app.drain_render_events();
            assert!(dirty, "有输出就必须重绘");
            cx.notify();
        })
    });
    let text = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.surface("local///shell/", 1)
                .map(|surface| surface.grid().screen_lines().join("\n"))
                .unwrap_or_default()
        })
    });
    assert!(
        text.contains("early output"),
        "拓扑缺失时输出也不能丢：{text:?}"
    );
}

/// 光标块必须正好一格：iTerm2 / Ghostty 的块光标是一个完整格子。
#[gpui::test]
fn the_cursor_block_covers_a_full_cell(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests("local@@dev@tmux@dev", 10, 40, 10, b"hi", cx);
        })
    });
    cx.run_until_parked();
    let cell = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    let bounds = cx.debug_bounds("pane-cursor");
    let Some(bounds) = bounds else {
        panic!("光标格必须有 pane-cursor 选择器");
    };
    assert!(
        (f32::from(bounds.size.height) - cell.1).abs() < 0.6,
        "光标块高度 = 格高：{} vs {}",
        f32::from(bounds.size.height),
        cell.1
    );
    assert!(
        (f32::from(bounds.size.width) - cell.0).abs() < 0.6,
        "光标块宽度 = 格宽：{} vs {}",
        f32::from(bounds.size.width),
        cell.0
    );
}

/// 用户要求：状态栏按钮要和 Swift 一样齐（侧栏开关、状态点、铃铛、全屏、设置、新建）。
#[gpui::test]
fn status_bar_carries_the_swift_controls(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    for selector in [
        "sidebar-toggle",
        "status-dot",
        "status-attention",
        "status-fullscreen",
        "status-settings",
        "new-tab",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "状态栏缺少 {selector}");
    }
}

/// 铃铛点开 Attention 面板（Swift `onAttentionClick`）。
#[gpui::test]
fn status_bar_bell_opens_the_attention_panel(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    let bell = cx.debug_bounds("status-attention").expect("bell");
    cx.simulate_mouse_down(
        bell.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let (open, tab) =
        cx.update(|_, cx| view.update(cx, |app, _| (app.panel_open(), app.panel_tab())));
    assert!(open, "铃铛必须打开面板");
    assert_eq!(tab, PanelTab::Attention);
}

/// 齿轮打开设置浮层（Swift `settingsButton`）。
#[gpui::test]
fn status_bar_settings_button_opens_settings(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    let gear = cx.debug_bounds("status-settings").expect("settings");
    cx.simulate_mouse_down(
        gear.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.settings_open)),
        "齿轮必须打开设置"
    );
}

/// 侧栏开关与 `Cmd-B` 同一条路径。
#[gpui::test]
fn status_bar_sidebar_toggle_hides_the_sidebar(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    cx.run_until_parked();
    let before = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_visible()));
    let toggle = cx.debug_bounds("sidebar-toggle").expect("sidebar toggle");
    cx.simulate_mouse_down(
        toggle.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let after = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_visible()));
    assert_ne!(before, after, "侧栏开关必须真的切换");
}

/// 状态点颜色跟随连接状态（Swift `StatusDotButton`）。
#[gpui::test]
fn status_dot_follows_the_backend_status(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::status_bar::connection_color;
    use muxterm::frontend::gpui::model::sidebar::backend_status;
    use muxterm::frontend::gpui::theme::Theme;
    let theme = Theme::default();
    assert_eq!(
        connection_color(backend_status::CONNECTED, &theme),
        theme.success
    );
    assert_eq!(
        connection_color(backend_status::CONNECTING, &theme),
        theme.warning
    );
    assert_eq!(
        connection_color(backend_status::ERROR, &theme),
        theme.danger
    );
    assert_eq!(
        connection_color(backend_status::EXITED, &theme),
        theme.danger
    );
    assert_eq!(
        connection_color(backend_status::DISCONNECTED, &theme),
        theme.muted
    );

    // 视图里的状态点读的是当前 workspace 的后端状态。
    let (view, cx) = mount(two_tab_store_with_pane_20(), cx);
    let _ = &view;
    cx.run_until_parked();
    let status = cx.update(|_, cx| view.update(cx, |app, _| app.visible_backend_status()));
    assert_eq!(status, backend_status::CONNECTED);
}

/// 拖选 + Cmd-C：选区文本必须进系统剪贴板（对照 iTerm2 / Ghostty 的默认复制）。
#[gpui::test]
fn dragging_selects_text_and_cmd_c_copies_it(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                40,
                10,
                b"hello world\r\nsecond line",
                cx,
            );
        })
    });
    cx.run_until_parked();
    let (cw, ch) = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    // 侧栏 240pt；内容区 y 从 0 开始。
    let start = gpui::point(px(240.0 + cw * 0.5), px(ch * 0.5));
    let end = gpui::point(px(240.0 + cw * 20.5), px(ch * 1.5));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(end, Some(gpui::MouseButton::Left), gpui::Modifiers::none());
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();

    let selection = cx.update(|_, cx| view.update(cx, |app, _| app.selection_for(10)));
    assert!(selection.is_some(), "拖拽后必须有选区");

    cx.simulate_keystrokes("cmd-c");
    let copied = cx.update(|_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    });
    assert!(
        copied.contains("hello world") && copied.contains("second"),
        "Cmd-C 必须复制选区：{copied:?}"
    );
}

/// 切 pane 只允许光影变：内容盒几何一格都不能动。
///
/// 用户报障：「切换 pane 的时候会出现 pane 的变化一下…应该只有光影的明暗
/// 变换才对」。以前给活动 pane 加 `border_1`，边框会挤小内容盒，文字整体
/// 位移 1pt、可见列数也跟着变。现在描边画在覆盖层，不占布局。
#[gpui::test]
fn switching_focus_only_changes_the_lighting(cx: &mut TestAppContext) {
    let (view, cx) = mount(three_pane_store(10), cx);
    cx.run_until_parked();
    let before = cx.debug_bounds("pane-text-10").expect("活动 pane 要渲染");
    assert!(
        cx.debug_bounds("pane-focus-ring").is_some(),
        "活动 pane 要有焦点描边"
    );
    assert!(
        cx.debug_bounds("pane-dimmed").is_some(),
        "非活动 pane 要压暗"
    );

    cx.update(|_, cx| view.update(cx, |app, cx| app.focus_pane(11, cx)));
    cx.run_until_parked();

    let after = cx
        .debug_bounds("pane-text-10")
        .expect("非活动 pane 仍要渲染");
    assert_eq!(before.origin, after.origin, "焦点变化不能让终端内容位移");
    assert_eq!(before.size, after.size, "焦点变化不能改变终端内容尺寸");
}

/// 用户要求：终端里 Command+点击链接要开浏览器（对照 Swift `requestOpenLink`）。
#[gpui::test]
fn cmd_click_opens_a_url_from_the_terminal(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                40,
                10,
                b"see https://example.com/x now",
                cx,
            );
        })
    });
    cx.run_until_parked();
    let (cw, ch) = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    // 列 10 落在链接中间（"see " 之后）。
    let point = gpui::point(px(240.0 + cw * 10.0 + 1.0), px(ch * 0.5));
    let cmd = gpui::Modifiers {
        platform: true,
        ..Default::default()
    };
    cx.simulate_mouse_down(point, gpui::MouseButton::Left, cmd);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, cmd);
    cx.run_until_parked();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/x"),
        "Command+点击必须打开格子下的链接"
    );
}

/// 没有 Command 时拖过链接仍然是选字，不能顺手开浏览器。
#[gpui::test]
fn dragging_over_a_url_still_selects_text(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                40,
                10,
                b"see https://example.com/x now",
                cx,
            );
        })
    });
    cx.run_until_parked();
    let (cw, ch) = cx.update(|_, cx| view.update(cx, |app, _| app.cell()));
    let start = gpui::point(px(240.0 + cw * 8.0 + 1.0), px(ch * 0.5));
    let end = gpui::point(px(240.0 + cw * 16.0 + 1.0), px(ch * 0.5));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(end, Some(gpui::MouseButton::Left), gpui::Modifiers::none());
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(cx.opened_url(), None, "普通拖选不能打开浏览器");
    assert!(
        cx.update(|_, cx| view.update(cx, |app, _| app.selection_for(10)))
            .is_some(),
        "拖过链接仍然是选区"
    );
}

/// Cmd-A / Edit → Select All：整屏（含滚动历史）都进选区，Cmd-C 复制全文。
#[gpui::test]
fn select_all_takes_the_whole_screen(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                20,
                4,
                b"first line\r\nsecond line\r\nthird",
                cx,
            );
        })
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-a");
    let (selection, text) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.selection_for(10),
                app.selected_text().unwrap_or_default(),
            )
        })
    });
    let selection = selection.expect("Cmd-A 必须建出选区");
    assert_eq!(
        (selection.anchor.row, selection.anchor.col),
        (0, 0),
        "选区从第 0 行第 0 列开始"
    );
    assert!(text.contains("first line"), "整屏都要选中：{text:?}");
    assert!(text.contains("third"), "最后一行也要选中：{text:?}");

    cx.simulate_keystrokes("cmd-c");
    let copied = cx.update(|_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    });
    assert!(copied.contains("first line") && copied.contains("third"));
}

/// 没有 pane 时 Cmd-A 不能 panic，也不能凭空造选区。
#[gpui::test]
fn select_all_without_a_pane_is_a_no_op(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.clear_focus_for_tests();
            assert!(!app.select_all(cx));
            assert!(app.selection_for(10).is_none());
        })
    });
}

/// 双击选词走 `word_selection`，三击整行；单击不算选区。
#[gpui::test]
fn click_counts_pick_words_lines_and_nothing(cx: &mut TestAppContext) {
    let (view, cx) = mount(two_tab_store(), cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.feed_pane_for_tests(
                "local@@dev@tmux@dev",
                10,
                40,
                10,
                b"git checkout feature/x\r\nnext",
                cx,
            );
        })
    });
    cx.run_until_parked();

    // 双击 `checkout`（列 5）。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| app.begin_selection(10, 0, 5, 2, cx));
    });
    let word = cx.update(|_, cx| view.update(cx, |app, _| app.selected_text()));
    assert_eq!(word.as_deref(), Some("checkout"), "双击选一整词");

    // 三击整行。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| app.begin_selection(10, 0, 5, 3, cx));
    });
    let line = cx.update(|_, cx| view.update(cx, |app, _| app.selected_text()));
    assert_eq!(line.as_deref(), Some("git checkout feature/x"));

    // 单击然后松手：选区清掉，Cmd-C 不复制任何东西。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.begin_selection(10, 0, 5, 1, cx);
            app.finish_drag(cx);
            assert!(app.selection_for(10).is_none(), "单击不是选区");
            assert!(!app.copy_selection(cx), "没有选区时 Cmd-C 不写剪贴板");
        });
    });
}
