//! GPUI 前端的 Herdr 验收：真实 named session，绝不碰用户默认 herdr server。
//!
//! Herdr 走 Catalog resolver（`open_target`），不是 legacy `new_connect_sized`：
//! 这条路径正是新建项目窗口保存后走的路径，所以这里顺带验证它能在真机上开起来。

#![cfg(feature = "gpui")]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use muxterm::frontend::command_queue::CommandQueue;
use muxterm::frontend::event_pump::EventPump;
use muxterm::frontend::gpui::state::GpuiApp;
use muxterm::frontend::utils::corebridge::{ClientOpenIntent, ClientTarget, FfiClient};
use muxterm::frontend::view_store::ViewStore;

/// 一个隔离的 named Herdr session；Drop 时 `session stop` + `session delete`。
struct IsolatedHerdr {
    name: String,
    socket: PathBuf,
    child: std::process::Child,
}

impl IsolatedHerdr {
    /// 没有 herdr 就显式 skip（和既有契约测试一致，不用 `#[ignore]`）。
    fn start() -> Option<Self> {
        if Command::new("herdr").arg("--version").output().is_err() {
            eprintln!("skip: herdr 不可用");
            return None;
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let name = format!("muxterm-test-gpui-{}-{unique}", std::process::id());
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let socket = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".config"))
            .join("herdr")
            .join("sessions")
            .join(&name)
            .join("herdr.sock");
        let child = Command::new("herdr")
            .args(["--session", &name, "server"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn herdr server 失败");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !socket.exists() {
            if Instant::now() >= deadline {
                eprintln!("skip: herdr socket 未出现 {socket:?}");
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Some(Self {
            name,
            socket,
            child,
        })
    }
}

impl Drop for IsolatedHerdr {
    fn drop(&mut self) {
        // 只停自己的 named session；禁止无名字的 `herdr server stop`。
        let _ = Command::new("herdr")
            .args(["session", "stop", &self.name])
            .output();
        let _ = Command::new("herdr")
            .args(["session", "delete", &self.name])
            .output();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 通过 Catalog 打开 Herdr workspace，返回挂好的 GpuiApp。
fn app_with_herdr<'a>(
    cx: &'a mut TestAppContext,
    fixture: &IsolatedHerdr,
) -> (gpui::Entity<GpuiApp>, &'a mut gpui::VisualTestContext) {
    let client = FfiClient::new_catalog().expect("catalog handle");
    let target = ClientTarget {
        name: "herdr-accept".to_string(),
        runtime: "herdr".to_string(),
        transport: "local".to_string(),
        target: None,
        path: std::env::temp_dir().to_string_lossy().to_string(),
        session: Some(fixture.name.clone()),
        socket: Some(fixture.socket.to_string_lossy().to_string()),
    };
    let opened = client
        .open_target(&target, ClientOpenIntent::CreateIfMissing)
        .expect("Herdr workspace 必须能打开");
    let mut store = ViewStore::default();
    let pump = EventPump::new(client);
    pump.sync_view_store(&mut store).expect("初始快照必须可用");

    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut app = GpuiApp::new(pump, store, CommandQueue::default(), None, cx);
        app.pump_once(cx);
        app
    });
    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle()));
    cx.run_until_parked();

    // Herdr 需要前端先报 viewport，否则不会开流；gpui 的 resize 路径会发 Resize。
    cx.simulate_resize(gpui::size(gpui::px(1024.0), gpui::px(700.0)));
    cx.run_until_parked();

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.pump_once(cx);
            })
        });
        let ready = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                app.sidebar_items()
                    .iter()
                    .any(|item| item.workspace_id == opened.id)
                    && app.pane_screen_of(&opened.id).is_some()
            })
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Herdr workspace {} 必须在超时前出现在侧栏并有画面",
            opened.id
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    (view, cx)
}

/// Herdr workspace 出现在侧栏，runtime 标成 herdr。
#[gpui::test]
fn herdr_workspace_appears_in_the_sidebar(cx: &mut TestAppContext) {
    let Some(fixture) = IsolatedHerdr::start() else {
        return;
    };
    let (view, cx) = app_with_herdr(cx, &fixture);
    let runtime = cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.sidebar_items()
                .into_iter()
                .find(|item| item.runtime == "herdr")
                .map(|item| item.runtime)
        })
    });
    assert_eq!(runtime.as_deref(), Some("herdr"));
}

/// 真实按键进 Herdr pane：输出回到 Surface。
#[gpui::test]
fn typing_reaches_the_herdr_pane(cx: &mut TestAppContext) {
    let Some(fixture) = IsolatedHerdr::start() else {
        return;
    };
    let (view, cx) = app_with_herdr(cx, &fixture);

    cx.simulate_keystrokes("e c h o space g p u i - h e r d r - a c c e p t");
    cx.simulate_keystrokes("enter");

    let deadline = Instant::now() + Duration::from_secs(30);
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
        if screen.contains("gpui-herdr-accept") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Herdr pane 必须回显输入，当前画面:\n{screen}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
