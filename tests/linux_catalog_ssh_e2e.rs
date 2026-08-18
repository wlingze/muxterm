//! C9：面板 click 已有的连接 → 扁平列表同时有 local 与 ssh-self 双份隔离 tmux。
//!
//! 本 crate 只构造一个 AppWindow。走面板 click，不直接 test_open_spec 冒充。
//! Host alias 固定 `self`（连 127.0.0.1）。不要求 archmini/cd。
//! 隔离 `-L muxterm-test-*`；无 sshd eprintln skip，禁止 #[ignore]。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};
use support::tmux_test_support::{create_session, kill_server, tmux_available, unique_socket};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

struct TmuxGuard {
    socket: String,
}

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

fn activate_named(app: &AppWindow, name: &str) {
    let list = find_by_name(&app.window, "muxterm-panel-list")
        .expect("面板列表应存在")
        .downcast::<gtk4::ListBox>()
        .expect("ListBox 类型");
    let mut idx = 0i32;
    let row = loop {
        let Some(row) = list.row_at_index(idx) else {
            panic!("列表里应有 {name}");
        };
        if row.widget_name() == name {
            break row;
        }
        idx += 1;
    };
    row.activate();
}

/// 已有的连接（扁平）→ local + ssh-self 各一行同一隔离 session。
#[test]
fn linux_catalog_panel_lists_local_and_ssh_self_duplicates() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux 二进制");
        return;
    }
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let sshd =
            LoopbackSshd::start_with_alias("gtk-cat-self", "self").expect("启动 Host self sshd");
        sshd.apply_ssh_config_env();
        let socket = unique_socket("gtk-cat-self");
        create_session(&socket, "mux-dup", 80, 24);
        let _tmux = TmuxGuard {
            socket: socket.clone(),
        };
        std::env::set_var("MUXTERM_TEST_LOCAL_TMUX_SOCKET", &socket);
        std::env::set_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET", &socket);
        std::env::set_var(
            "HERDR_SOCKET_PATH",
            format!("/tmp/muxterm-no-herdr-{}", std::process::id()),
        );

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_panel(0);
        pump_main_loop(80);
        app.test_poll_once();

        find_by_name(&app.window, "muxterm-existing-connections").expect("根列表应有已有的连接");
        activate_named(&app, "muxterm-existing-connections");
        pump_main_loop(60);

        let deadline = Instant::now() + PROBE_TIMEOUT;
        let mut saw_local = false;
        let mut saw_self = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(40);
            saw_local |=
                find_by_name(&app.window, "muxterm-existing-row-tmux-local-mux-dup").is_some();
            saw_self |=
                find_by_name(&app.window, "muxterm-existing-row-tmux-self-mux-dup").is_some();
            if saw_local && saw_self {
                break;
            }
        }
        assert!(
            find_by_name(&app.window, "muxterm-existing-local").is_none(),
            "禁止再出现 SSH/本地目录"
        );
        assert!(
            find_by_name(&app.window, "muxterm-existing-ssh").is_none(),
            "禁止再出现 SSH 目录；runtime list 必须扁平"
        );
        assert!(
            find_by_name(&app.window, "muxterm-existing-host-self").is_none()
                && find_by_name(&app.window, "muxterm-existing-host-local").is_none(),
            "禁止 Host 行；点已有的连接就应看到 session"
        );
        assert!(
            saw_local && saw_self,
            "local + ssh-self 必须双份 mux-dup。local={saw_local} self={saw_self} loading={:?}",
            find_by_name(&app.window, "muxterm-existing-ssh-loading").is_some()
        );

        std::env::remove_var("MUXTERM_TEST_LOCAL_TMUX_SOCKET");
        std::env::remove_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET");
        std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
        std::env::remove_var("HERDR_SOCKET_PATH");
        app.shutdown();
        pump_main_loop(80);
    });
}
