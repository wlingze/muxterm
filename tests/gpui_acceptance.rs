//! GPUI 前端的真机验收：真实隔离 tmux + 真实 `GpuiApp`，不是只喂 Surface。
//!
//! 每个场景自己建一个 `tmux -L muxterm-test-...` server，结束时用同一个 `-L`
//! 关掉。绝不碰用户默认 tmux server。
//!
//! 覆盖 macOS `MuxtermAppUITests` 里不需要真 GUI 的那部分：启动即有 workspace、
//! 输入进 shell、Cmd-T 新 tab、侧栏切换 workspace、关闭 workspace。

#![cfg(feature = "gpui")]

use std::time::{Duration, Instant};

use gpui::TestAppContext;
use muxterm::frontend::command_queue::CommandQueue;
use muxterm::frontend::event_pump::EventPump;
use muxterm::frontend::gpui::state::GpuiApp;
use muxterm::frontend::utils::corebridge::FfiClient;
use muxterm::frontend::view_store::ViewStore;

/// 一次隔离 tmux 会话的句柄；drop 时用同一个 `-L` 清理。
struct IsolatedTmux {
    socket: String,
    session: String,
}

impl IsolatedTmux {
    fn start() -> Self {
        Self::start_with_command(&[])
    }

    fn start_with_command(command: &[&str]) -> Self {
        // 并行测试可能在同一纳秒起步，所以再拼一个进程内序号。
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let socket = format!("muxterm-test-gpui-accept-{unique}-{sequence}");
        let session = format!("muxterm-test-gpui-{unique}-{sequence}");
        let status = std::process::Command::new("tmux")
            .args(["-L", &socket, "new-session", "-d", "-s", &session])
            .args(command)
            .status()
            .expect("tmux 必须可用");
        assert!(status.success(), "隔离 tmux session 必须创建成功");
        Self { socket, session }
    }

    /// 只杀自己的测试会话（必须带同一个 `-L`）。
    fn kill_session(&self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.socket, "kill-session", "-t", &self.session])
            .status();
    }

    fn connect(&self) -> EventPump {
        // tmux 是 control runtime：先报基准尺寸才会有首帧。
        let client = FfiClient::new_connect_sized(
            "tmux",
            Some(&self.socket),
            Some(&self.session),
            None,
            None,
            80,
            24,
        )
        .expect("隔离 socket 必须能连接");
        EventPump::new(client)
    }
}

impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        // 清理必须带同一个 -L：只杀自己的测试 server。
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .status();
    }
}

/// 建一个连着隔离 tmux 的 GpuiApp，并泵到首帧出现。
fn app_with_isolated_tmux<'a>(
    cx: &'a mut TestAppContext,
    tmux: &IsolatedTmux,
) -> (gpui::Entity<GpuiApp>, &'a mut gpui::VisualTestContext) {
    let pump = tmux.connect();
    let mut store = ViewStore::default();
    pump.sync_view_store(&mut store).expect("初始快照必须可用");

    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut app = GpuiApp::new(pump, store, CommandQueue::default(), None, cx);
        app.pump_once(cx);
        app
    });
    // gpui 只有聚焦的 window 才会把按键送到 on_key_down。
    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle()));
    cx.run_until_parked();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let ready = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.focused_pane()
                    .and_then(|pane| app.pane_screen(pane))
                    .is_some_and(|screen| {
                        screen.contains('$')
                            || screen.contains('%')
                            || screen.contains('❯')
                            || !screen.trim().is_empty()
                    })
            })
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "首帧必须在超时前出现");
        std::thread::sleep(Duration::from_millis(50));
    }
    (view, cx)
}

/// 泵到画面里出现指定文本（或超时失败）。
fn pump_until(view: &gpui::Entity<GpuiApp>, cx: &mut gpui::VisualTestContext, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let screen = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.focused_pane()
                    .and_then(|pane| app.pane_screen(pane))
                    .unwrap_or_default()
            })
        });
        if screen.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "等待 {needle:?} 超时，当前画面:\n{screen}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 启动即有 workspace：侧栏一行、tab 栏一个、pane 有画面。
#[gpui::test]
fn startup_workspace_renders_end_to_end(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    let (sidebar, tabs) =
        cx.update(|_, cx| view.update(cx, |app, _| (app.sidebar_items(), app.tab_items())));
    // 对照 Swift `sidebarItems()`：两个固定聚合槽永远在最前，真实 workspace 跟在其后。
    assert_eq!(
        sidebar.len(),
        3,
        "Shells + Agents + 一个真实 workspace：{:?}",
        sidebar.iter().map(|item| &item.name).collect::<Vec<_>>()
    );
    assert_eq!(
        sidebar[0].workspace_id,
        muxterm::frontend::gpui::model::sidebar::AGGREGATE_SHELLS
    );
    assert_eq!(
        sidebar[1].workspace_id,
        muxterm::frontend::gpui::model::sidebar::AGGREGATE_AGENTS
    );
    assert_eq!(sidebar[2].runtime, "tmux");
    assert_eq!(tabs.len(), 1, "启动只有一个 tab");
    assert!(tabs[0].title.contains("(1)") || !tabs[0].title.is_empty());
}

/// 本机 shell 启动：侧栏只有两个聚合槽，Shells 高亮，tab 栏是 shell 的 tab。
///
/// 对照 Swift 启动路径 `workspacePresentation = .shells(...)` 与
/// `sidebarItems()`：`shell` runtime 的原始 workspace 不单独占侧栏行。
#[gpui::test]
fn local_shell_launch_shows_the_shells_aggregate(cx: &mut TestAppContext) {
    let client = FfiClient::new_connect("local", None, None, None, Some("")).expect("本机连接");
    let pump = EventPump::new(client);
    let mut store = ViewStore::default();
    pump.sync_view_store(&mut store).expect("初始快照");

    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut app = GpuiApp::new(pump, store, CommandQueue::default(), None, cx);
        app.pump_once(cx);
        app
    });
    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle()));

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| view.update(cx, |app, cx| app.pump_once(cx)));
        let ready = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.focused_pane()
                    .and_then(|pane| app.pane_screen(pane))
                    .is_some_and(|screen| !screen.trim().is_empty())
            })
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "本机 shell 必须起来");
        std::thread::sleep(Duration::from_millis(50));
    }

    let (items, aggregate, tabs) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (app.sidebar_items(), app.aggregate_kind(), app.tab_items())
        })
    });
    assert_eq!(
        items.len(),
        2,
        "本机 shell 不占原始 workspace 行：{:?}",
        items.iter().map(|item| &item.name).collect::<Vec<_>>()
    );
    assert_eq!(
        items[0].workspace_id,
        muxterm::frontend::gpui::model::sidebar::AGGREGATE_SHELLS
    );
    assert_eq!(
        items[1].workspace_id,
        muxterm::frontend::gpui::model::sidebar::AGGREGATE_AGENTS
    );
    assert!(items[0].is_active, "启动后 Shells 聚合应当是当前来源");
    assert_eq!(
        aggregate,
        Some(muxterm::frontend::gpui::chrome::panel_view::AggregateKind::Shells)
    );
    assert_eq!(tabs.len(), 1, "Shells 聚合里有 shell 的 tab");
    assert!(
        tabs[0].title.contains("local"),
        "聚合 tab 标题要带来源：{:?}",
        tabs[0].title
    );
    let screen = cx
        .update(|_, cx| {
            view.update(cx, |app, _| {
                app.focused_pane().and_then(|pane| app.pane_screen(pane))
            })
        })
        .unwrap_or_default();
    assert!(!screen.trim().is_empty(), "shell 首帧不能是空画面");
}

/// 真实按键路径：`simulate_keystrokes` → encode → CommandQueue → tmux → Surface。
#[gpui::test]
fn typed_input_reaches_the_real_shell(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    cx.simulate_keystrokes("e c h o space g p u i - a c c e p t");
    cx.simulate_keystrokes("enter");
    pump_until(&view, cx, "gpui-accept");
}

/// Cmd-T 新建 tab：Core 拓扑事件回来后 tab 栏变两个。
#[gpui::test]
fn cmd_t_adds_a_tab_through_core(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    cx.simulate_keystrokes("cmd-t");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let count = cx.update(|_, cx| view.update(cx, |app, _| app.tab_items().len()));
        if count >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cmd-T 之后 tab 栏必须有两个 tab（实际 {count}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 打开第二个 workspace：真的走 Catalog resolver，侧栏两行、能切过去。
#[gpui::test]
fn opening_a_second_workspace_through_core_adds_a_sidebar_row(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_target_editor(None, cx);
            app.update_target_editor(
                |editor| {
                    editor.set_name("second");
                    editor.set_path("/tmp");
                },
                cx,
            );
            app.save_target_editor(cx);
        })
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let sidebar = cx.update(|_, cx| view.update(cx, |app, _| app.sidebar_items()));
        // 聚合槽不算真实 workspace：只数后面的行。
        if sidebar.iter().filter(|item| !item.is_aggregate()).count() >= 2 {
            assert!(
                sidebar.iter().any(|item| item.name.contains("second")),
                "侧栏要出现新 workspace：{:?}",
                sidebar.iter().map(|item| &item.name).collect::<Vec<_>>()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "打开第二个 workspace 超时，侧栏还是 {} 行",
            sidebar.len()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // 切到第二行：活动标记跟着走，pane 画面来自新 workspace。
    let target = cx
        .update(|_, cx| {
            view.update(cx, |app, _| {
                app.sidebar_items()
                    .iter()
                    .find(|item| item.name.contains("second"))
                    .map(|item| item.workspace_id.clone())
            })
        })
        .expect("第二个 workspace");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.activate_sidebar_row(
                muxterm::frontend::gpui::chrome::sidebar_view::RowTarget::Workspace(target.clone()),
                cx,
            )
        })
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let active = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.sidebar_items()
                    .iter()
                    .find(|item| item.workspace_id == target)
                    .map(|item| item.is_active)
            })
        });
        if active == Some(true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "切换后第二个 workspace 必须变成活动行"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 面板的 Open 流程真的建出 workspace：机器 → 新路径 → 目录 → 确认 → Core。
///
/// 回归两件事：`Open` 不再打开旧表单；本地 `~/…` 路径必须先展开成绝对路径，
/// 否则列表为空、`create` 也「找不到路径」打不开。
#[gpui::test]
fn picker_open_flow_creates_a_real_workspace(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    use muxterm::frontend::gpui::model::panel::PanelTab;
    use muxterm::frontend::gpui::model::target::TargetRuntime;

    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    let activate = |view: &gpui::Entity<GpuiApp>, cx: &mut gpui::VisualTestContext, title: &str| {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                let rows = app.panel_rows();
                let index = rows
                    .iter()
                    .position(|row| row.title() == title)
                    .unwrap_or_else(|| panic!("面板缺少 {title:?}：{rows:?}"));
                app.select_panel_row(index, cx);
            })
        })
    };

    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.begin_panel_open(cx);
            app.show_panel(PanelTab::Workspaces, cx);
        })
    });
    activate(&view, cx, "local");
    activate(&view, cx, "+ Open New Path");
    // `~` 开头是默认 seed：目录列表与 create 都必须展开它。
    cx.update(|_, cx| view.update(cx, |app, cx| app.set_panel_query("~/tmp", cx)));
    activate(&view, cx, "Open This Directory");
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.choose_panel_runtime(TargetRuntime::Tmux, cx);
            app.set_panel_query("picker-new", cx);
        })
    });
    activate(&view, cx, "Create Workspace");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let (sidebar, panel_open, error, rows) = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                (
                    app.sidebar_items()
                        .iter()
                        .map(|item| item.name.clone())
                        .collect::<Vec<_>>(),
                    app.panel_open(),
                    app.panel_rows().into_iter().find_map(|row| match row {
                        PanelRow::Status(message) => Some(message),
                        _ => None,
                    }),
                    app.panel_rows().len(),
                )
            })
        });
        if sidebar.iter().any(|item| item.contains("picker-new")) {
            assert!(!panel_open, "打开成功后必须收起面板");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Open 流程必须建出 workspace；侧栏={sidebar:?} 错误={error:?} 行数={rows}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 布局里的叶子 pane 数量与 id（分屏断言用）。
fn leaves(layout: &Option<muxterm::frontend::utils::corebridge::ClientLayout>) -> usize {
    leaf_ids(layout).len()
}

fn leaf_ids(layout: &Option<muxterm::frontend::utils::corebridge::ClientLayout>) -> Vec<u32> {
    use muxterm::frontend::utils::corebridge::ClientLayout;
    fn walk(layout: &ClientLayout, out: &mut Vec<u32>) {
        match layout {
            ClientLayout::Leaf { pane_id } => out.push(*pane_id),
            ClientLayout::Split { first, second, .. } => {
                walk(first, out);
                walk(second, out);
            }
        }
    }
    layout
        .as_ref()
        .map(|layout| {
            let mut out = Vec::new();
            walk(layout, &mut out);
            out
        })
        .unwrap_or_default()
}

/// Cmd-D 分屏：Core 回来的拓扑里多一个 pane，两块画面都在，焦点跟到新 pane。
#[gpui::test]
fn cmd_d_splits_the_pane_through_core(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);
    let before = cx.update(|_, cx| view.update(cx, |app, _| app.layout()));
    assert_eq!(leaves(&before), 1, "启动只有一个 pane");

    cx.simulate_keystrokes("cmd-d");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let panes = cx.update(|_, cx| view.update(cx, |app, _| leaves(&app.layout())));
        if panes >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cmd-D 之后必须有两个 pane（实际 {panes}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let (focused, pane_ids) =
        cx.update(|_, cx| view.update(cx, |app, _| (app.focused_pane(), leaf_ids(&app.layout()))));
    assert!(
        focused.is_some_and(|pane| pane_ids.contains(&pane) && pane_ids.last() == Some(&pane)),
        "分屏后焦点必须在新 pane 上（焦点 {focused:?}，panes {pane_ids:?}）"
    );
}

/// Cmd-[ / Cmd-]：Core 换 active pane，前端焦点必须跟着。
#[gpui::test]
fn cmd_brackets_walk_the_panes_through_core(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);
    cx.simulate_keystrokes("cmd-d");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        if cx.update(|_, cx| view.update(cx, |app, _| leaves(&app.layout()))) >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "Cmd-D 必须先分出一个 pane");
        std::thread::sleep(Duration::from_millis(50));
    }
    let first = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
    cx.simulate_keystrokes("cmd-[");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let focused = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
        if focused != first {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cmd-[ 之后焦点必须换到另一个 pane（还停在 {first:?}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let second = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
    assert_ne!(first, second, "Cmd-[ 必须换 pane");
    // Cmd-] 回到原来的 pane。
    cx.simulate_keystrokes("cmd-]");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        if cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane())) == first {
            break;
        }
        assert!(Instant::now() < deadline, "Cmd-] 必须回到原来的 pane");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 活动接入：Core 的活动快照要 join 到前端 workspace id 上。
///
/// Core 用 replica id（`session:path@transport`）聚合活动，侧栏用产品 id；
/// 这条用真实 FFI 快照证明两边对得上——对不上就是「跑着 pi，Agents 还是空」。
#[gpui::test]
fn core_activity_joins_the_frontend_workspace(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    let workspace = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.visible_workspace().map(str::to_string).unwrap()
        })
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let joined = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.attention_workspace_ids()
                    .iter()
                    .any(|id| id == &workspace)
            })
        });
        if joined {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Core 的活动快照必须 join 到 {workspace:?} 上（实际 {:?}）",
            cx.update(|_, cx| view.update(cx, |app, _| app.attention_workspace_ids()))
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 泵到某个条件成立（超时失败）。
fn pump_until_true(
    view: &gpui::Entity<GpuiApp>,
    cx: &mut gpui::VisualTestContext,
    what: &str,
    mut done: impl FnMut(&GpuiApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let ok = cx.update(|_, cx| view.update(cx, |app, _| done(app)));
        if ok {
            return;
        }
        assert!(Instant::now() < deadline, "等待 {what} 超时");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Rename Tab 走真实 Core：面板里改名 → tmux 窗口名变了 → tab 栏跟着变。
#[gpui::test]
fn rename_tab_reaches_core(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);
    cx.simulate_keystrokes("cmd-shift-p");
    let picked = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_select_action_for_tests(
                muxterm::frontend::gpui::chrome::palette_view::PaletteAction::RenameTab,
            )
        })
    });
    assert!(picked, "命令面板里必须有 Rename Tab");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // 清掉带出的旧名字，输入新名字。
    for _ in 0..40 {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_keystrokes("g p u i space r e n a m e d");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    pump_until_true(&view, cx, "tab 改名生效", |app| {
        app.tab_items()
            .iter()
            .any(|tab| tab.title.contains("gpui renamed"))
    });
}

/// Move Tab Right 走真实 Core：两个 tab 交换顺序。
#[gpui::test]
fn move_tab_reorders_through_core(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);
    cx.simulate_keystrokes("cmd-t");
    pump_until_true(&view, cx, "第二个 tab", |app| app.tab_items().len() >= 2);
    cx.simulate_keystrokes("cmd-shift-p");
    let picked = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.palette_select_action_for_tests(
                muxterm::frontend::gpui::chrome::palette_view::PaletteAction::MoveTabLeft,
            )
        })
    });
    assert!(picked, "命令面板里必须有 Move Tab Left");
    cx.simulate_keystrokes("enter");

    // 活动 tab（第二个）左移后应该变成第一个。
    pump_until_true(&view, cx, "tab 顺序变化", |app| {
        let tabs = app.tab_items();
        tabs.len() >= 2 && tabs[0].active
    });
}

/// 起一个连本机 shell 的 GpuiApp，泵到首帧出现。
fn app_with_local_shell<'a>(
    cx: &'a mut TestAppContext,
) -> (gpui::Entity<GpuiApp>, &'a mut gpui::VisualTestContext) {
    let client = FfiClient::new_connect("local", None, None, None, Some("")).expect("本机连接");
    let pump = EventPump::new(client);
    let mut store = ViewStore::default();
    pump.sync_view_store(&mut store).expect("初始快照");
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut app = GpuiApp::new(pump, store, CommandQueue::default(), None, cx);
        app.pump_once(cx);
        window.focus(&app.focus_handle());
        app
    });
    pump_until_true(&view, cx, "本机 shell 首帧", |app| {
        app.focused_pane()
            .and_then(|pane| app.pane_screen(pane))
            .is_some_and(|screen| !screen.trim().is_empty())
    });
    (view, cx)
}

/// 对照 AttentionSemanticsE2ETests.testBlockedLeavesAttentionAfterViewAndRegexLightsAgain。
/// 真 PTY 发两次 BEL，中间停在 read；不能只藏前端行，下一次 Core 快照也必须已读。
#[gpui::test]
fn viewing_a_notification_survives_a_core_refresh_but_not_the_next_alert(cx: &mut TestAppContext) {
    use muxterm::frontend::gpui::chrome::panel_view::PanelRow;
    use muxterm::frontend::gpui::model::panel::PanelTab;
    use muxterm::frontend::gpui::model::sidebar::Indicator;

    // 专用 socket 上的交互程序：不用用户的 shell 初始化文件；每次回车才发一条提醒。
    let tmux = IsolatedTmux::start_with_command(&[
        "/bin/sh",
        "-c",
        r#"printf 'READ_FIXTURE_READY\n'; while IFS= read -r reply; do printf 'READ_FIXTURE_ALERT\n\007'; done"#,
    ]);
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);
    pump_until(&view, cx, "READ_FIXTURE_READY");
    cx.simulate_keystrokes("enter");
    pump_until(&view, cx, "READ_FIXTURE_ALERT");
    let is_blocked = |app: &GpuiApp| app.status_attention().indicator == Some(Indicator::Blocked);
    pump_until_true(&view, cx, "第一次 BEL 产生未读", is_blocked);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            let id = app.focused_pane().unwrap();
            app.show_panel(PanelTab::Attention, cx);
            let index = app
                .panel_rows()
                .iter()
                .position(
                    |row| matches!(row, PanelRow::Attention { pane_id, .. } if *pane_id == id),
                )
                .unwrap();
            app.select_panel_row(index, cx);
            app.pump_once(cx);
            let snapshot = app.client_for_tests().unwrap().activity_snapshot().unwrap();
            let pane = snapshot
                .workspaces
                .iter()
                .flat_map(|ws| &ws.panes)
                .find(|p| p.pane_id == id)
                .expect("Core pane");
            assert!(
                pane.acknowledged,
                "下一轮 Core 快照不能把点过的提醒带回来：{pane:?}"
            );
            assert_eq!(pane.status, "blocked", "查看不等于给进程输入");
            app.refresh_attention();
            assert!(!app.status_attention().is_active());
        })
    });
    cx.simulate_keystrokes("enter");
    pump_until_true(&view, cx, "同一 pane 的第二次 BEL 仍可提醒", is_blocked);
}

/// 回归（pane 关闭）：本机 shell 进程退出只移除它自己的 pane；
/// 焦点回到剩下的 pane，tab 不能被误关。
#[gpui::test]
fn a_local_shell_exit_removes_only_its_pane(cx: &mut TestAppContext) {
    let (view, cx) = app_with_local_shell(cx);
    let first = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
    cx.simulate_keystrokes("cmd-d");
    pump_until_true(&view, cx, "分屏出第二个 pane", |app| {
        leaves(&app.layout()) == 2
    });
    let second = cx.update(|_, cx| view.update(cx, |app, _| app.focused_pane()));
    assert_ne!(second, first, "分屏后焦点必须落在新 pane");

    cx.simulate_keystrokes("e x i t");
    cx.simulate_keystrokes("enter");
    pump_until_true(&view, cx, "退出的 pane 被移除", |app| {
        leaves(&app.layout()) == 1
    });

    let (focused, tabs, quit) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.focused_pane(),
                app.tab_items().len(),
                app.quit_requested(),
            )
        })
    });
    assert_eq!(focused, first, "焦点必须回到剩下的 pane");
    assert_eq!(tabs, 1, "只关一个 pane 时 tab 必须留下");
    assert!(!quit, "还有 pane 时不能退出窗口");
}

/// 回归（shell 退出）：隔离 tmux 会话结束后，冻住的 workspace 必须收起并请求关窗口，
/// 不能停在最后一帧（用户报的「卡住」）。
#[gpui::test]
fn a_tmux_session_that_ends_closes_the_workspace(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);
    tmux.kill_session();
    pump_until_true(&view, cx, "结束的 workspace 被收起", |app| {
        app.visible_workspace().is_none()
    });
    let (sidebar, tabs, quit) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.sidebar_items().len(),
                app.tab_items().len(),
                app.quit_requested(),
            )
        })
    });
    assert_eq!(sidebar, 2, "只剩 Shells / Agents 两个聚合槽");
    assert_eq!(tabs, 0, "僵尸 tab 必须退出");
    assert!(quit, "没有活 workspace 时按 Swift 关窗口");
}

/// 回归（关闭 workspace）：关掉可见 workspace 要真的让 Core 收起它，
/// 前端立刻切到剩下的那个，画面不能空。
///
/// 旧实现只把 `CloseWorkspace` 塞进队列；Core 不会为此发 topology 事件，
/// 于是侧栏行和冻住的画面永远留着，用户看到的就是「关闭 workspace 失败」。
#[gpui::test]
fn closing_a_workspace_reaches_core_and_falls_back(cx: &mut TestAppContext) {
    let tmux = IsolatedTmux::start();
    let (view, cx) = app_with_isolated_tmux(cx, &tmux);

    // 通过面板开第二个 workspace（与上一个测试同一条真实路径）。
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.open_target_editor(None, cx);
            app.update_target_editor(
                |editor| {
                    editor.set_name("second");
                    editor.set_path("/tmp");
                },
                cx,
            );
            app.save_target_editor(cx);
        })
    });
    pump_until_true(&view, cx, "第二个 workspace 出现", |app| {
        app.sidebar_items()
            .iter()
            .filter(|item| !item.is_aggregate())
            .count()
            >= 2
    });
    let before = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.core_workspace_count_for_tests(),
                app.visible_workspace().map(str::to_string),
            )
        })
    });
    assert_eq!(before.0, Some(2), "Core 里应该有两个 workspace");
    let closing = before.1.expect("可见 workspace");

    cx.update(|_, cx| view.update(cx, |app, cx| app.close_workspace(closing.clone(), cx)));
    pump_until_true(&view, cx, "Core 收起被关的 workspace", |app| {
        app.core_workspace_count_for_tests() == Some(1)
    });
    pump_until_true(&view, cx, "切到剩下的 workspace", |app| {
        app.visible_workspace().is_some_and(|id| id != closing)
    });

    let (sidebar, quit, screen) = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            (
                app.sidebar_items()
                    .iter()
                    .filter(|item| !item.is_aggregate())
                    .count(),
                app.quit_requested(),
                app.focused_pane().and_then(|pane| app.pane_screen(pane)),
            )
        })
    });
    assert_eq!(sidebar, 1, "侧栏只剩没被关的那个 workspace");
    assert!(!quit, "还有 workspace 时不能退出窗口");
    assert!(
        screen.is_some_and(|screen| !screen.trim().is_empty()),
        "剩下的 workspace 必须还有画面"
    );
}
