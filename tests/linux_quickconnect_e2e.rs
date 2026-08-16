//! Linux QuickConnect / status bar / pool 的隔离 tmux e2e。
//!
//! 所有真实 tmux 操作使用 `tmux -L muxterm-test-<unique>`，清理也带同一个
//! `-L`。绝不碰默认 server。

#![cfg(feature = "gtk")]

use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muxterm::platform::linux::ffi_bridge::{tasks, CoreBridge};
use muxterm::platform::linux::pane_view::should_forward_replies;
use muxterm::platform::linux::quickconnect::font::{FontSettings, Preferences};
use muxterm::platform::linux::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use muxterm::platform::linux::quickconnect::project_flow::{
    ProjectConnectFlow, ProjectConnectState,
};

fn unique_socket(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-test-{}-{}-{}", label, std::process::id(), nanos)
}

struct IsolatedTmux {
    socket: String,
}

impl IsolatedTmux {
    fn new(label: &str) -> Option<Self> {
        let socket = unique_socket(label);
        let probe = Command::new("tmux")
            .args(["-L", &socket, "-f", "/dev/null", "list-sessions"])
            .output();
        if probe.is_err() {
            return None;
        }
        Some(IsolatedTmux { socket })
    }

    fn has_session(&self, name: &str) -> bool {
        Command::new("tmux")
            .args(["-L", &self.socket, "has-session", "-t", name])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn new_session(&self, name: &str) -> bool {
        Command::new("tmux")
            .args([
                "-L",
                &self.socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "24",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn zoomed(&self) -> bool {
        let out = Command::new("tmux")
            .args([
                "-L",
                &self.socket,
                "display-message",
                "-p",
                "#{window_zoomed_flag}",
            ])
            .output()
            .ok();
        out.map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    }
}

impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
    }
}

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn project_flow_attach_existing_then_create_then_attach() {
    let Some(tmux) = IsolatedTmux::new("qc-flow") else {
        eprintln!("skip: tmux 不可用");
        return;
    };
    let dir = std::env::temp_dir();
    let dir = dir.to_str().unwrap_or("/tmp");

    assert!(tmux.new_session("existing"));
    assert!(tmux.has_session("existing"));

    let cfg = TargetConfig::new("existing", TargetRuntime::Tmux, TargetTransport::Local, dir);
    let mut flow = ProjectConnectFlow::new(&cfg);
    assert!(matches!(
        flow.state,
        ProjectConnectState::AttachExisting { .. }
    ));

    let bridge = CoreBridge::connect(
        "tmux",
        Some(&tmux.socket),
        Some("existing"),
        None,
        Some(dir),
    )
    .expect("attach 已有 session");
    let _ = bridge.poll_events();
    flow.attach_existing_succeeded();
    assert_eq!(flow.state, ProjectConnectState::Done);
    assert!(!bridge.get_tabs().is_empty(), "attach 后应有 window/tab");
    drop(bridge);

    assert!(
        tmux.has_session("existing"),
        "drop control client 不得杀掉 session"
    );

    let created = TargetConfig::new("created", TargetRuntime::Tmux, TargetTransport::Local, dir);
    let mut create_flow = ProjectConnectFlow::new(&created);
    create_flow.attach_existing_failed("no session");
    assert!(matches!(
        create_flow.state,
        ProjectConnectState::CreateDetached { .. }
    ));

    let (backend, target) = created.transport.create_backend();
    CoreBridge::create_workspace(backend, target, Some(&tmux.socket), "created", dir)
        .expect("create detached session");
    create_flow.create_succeeded();
    assert!(tmux.has_session("created"));

    let attached =
        CoreBridge::connect("tmux", Some(&tmux.socket), Some("created"), None, Some(dir))
            .expect("attach 刚创建的 session");
    let _ = attached.poll_events();
    create_flow.attach_created_succeeded();
    assert_eq!(create_flow.state, ProjectConnectState::Done);
    drop(attached);
    assert!(tmux.has_session("created"));
}

#[test]
fn pool_detach_keeps_isolated_session() {
    let Some(tmux) = IsolatedTmux::new("qc-pool") else {
        eprintln!("skip: tmux 不可用");
        return;
    };
    assert!(tmux.new_session("warm"));
    let dir = std::env::temp_dir();
    let dir = dir.to_str().unwrap_or("/tmp");

    let bridge = CoreBridge::connect("tmux", Some(&tmux.socket), Some("warm"), None, Some(dir))
        .expect("connect warm session");
    let rc = bridge.detach();
    assert_eq!(rc, 0, "detach 应成功");
    drop(bridge);
    assert!(
        tmux.has_session("warm"),
        "detach 后 isolated session 必须仍在"
    );

    let re = CoreBridge::connect("tmux", Some(&tmux.socket), Some("warm"), None, Some(dir))
        .expect("re-attach 复用同一 session");
    let _ = re.poll_events();
    assert!(!re.get_tabs().is_empty());
}

#[test]
fn status_snapshot_and_fullscreen_zoom_on_isolated_tmux() {
    let Some(tmux) = IsolatedTmux::new("qc-status") else {
        eprintln!("skip: tmux 不可用");
        return;
    };
    assert!(tmux.new_session("stat"));
    let dir = std::env::temp_dir();
    let dir = dir.to_str().unwrap_or("/tmp");
    let bridge = CoreBridge::connect("tmux", Some(&tmux.socket), Some("stat"), None, Some(dir))
        .expect("connect for status");
    let _ = bridge.poll_events();

    let snap = wait_until(Duration::from_secs(3), || {
        let _ = bridge.poll_events();
        bridge
            .status_snapshot()
            .is_some_and(|s| s.enabled && !s.windows.is_empty())
    });
    assert!(snap, "status snapshot 应含窗口列表");
    let snapshot = bridge.status_snapshot().expect("snapshot");
    assert!(
        snapshot.windows.iter().any(|w| w.current),
        "应有当前窗口标记"
    );

    let panes = bridge.get_panes(
        bridge
            .get_tabs()
            .iter()
            .find(|t| t.is_active)
            .map(|t| t.id)
            .unwrap_or(0),
    );
    let pane_id = panes
        .iter()
        .find(|p| p.is_active)
        .map(|p| p.id)
        .or_else(|| panes.first().map(|p| p.id))
        .expect("应有 pane");
    let _ = bridge.execute(tasks::split_h(pane_id));
    let _ = wait_until(Duration::from_secs(2), || {
        let _ = bridge.poll_events();
        bridge.get_panes(pane_id).len() >= 2
            || bridge
                .get_tabs()
                .iter()
                .any(|t| bridge.get_panes(t.id).len() >= 2)
    });
    let rc = bridge.execute(tasks::toggle_pane_fullscreen(pane_id));
    assert_eq!(rc, 0, "resize-pane -Z 应成功");
    assert!(
        wait_until(Duration::from_secs(2), || tmux.zoomed()),
        "tmux window_zoomed_flag 应为 1"
    );
    let _ = bridge.execute(tasks::toggle_pane_fullscreen(pane_id));
}

#[test]
fn mirror_mode_drops_parser_query_replies() {
    assert!(!should_forward_replies(true, b"\x1b]11;?\x07"));
    assert!(!should_forward_replies(true, b"\x1b[c"));
    assert!(should_forward_replies(false, b"\x1b]11;?\x07"));
}

#[test]
fn font_theme_preferences_roundtrip() {
    let p = Preferences {
        theme: Some("light".into()),
        statusbar_mode: Some("tmux".into()),
        font_size: Some(FontSettings::zoomed(12.0, 1)),
    };
    let raw = toml::to_string_pretty(&p).unwrap();
    let back: Preferences = toml::from_str(&raw).unwrap();
    assert_eq!(back.theme.as_deref(), Some("light"));
    assert_eq!(back.statusbar_mode.as_deref(), Some("tmux"));
    assert_eq!(back.font_size, Some(13.0));
}
