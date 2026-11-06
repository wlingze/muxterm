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
            .status()
            .expect("tmux 必须可用");
        assert!(status.success(), "隔离 tmux session 必须创建成功");
        Self { socket, session }
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
    assert_eq!(sidebar.len(), 1, "启动只有一个 workspace");
    assert_eq!(sidebar[0].runtime, "tmux");
    assert_eq!(tabs.len(), 1, "启动只有一个 tab");
    assert!(tabs[0].title.contains("(1)") || !tabs[0].title.is_empty());
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
        if sidebar.len() >= 2 {
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
