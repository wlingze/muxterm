//! C7：面板 click SSH Host `local` → 隔离 tmux 行出现。
//!
//! 本 crate 只构造一个 AppWindow。走面板 click，不直接 test_open_spec 冒充。
//! Host alias 固定 `local`（连 127.0.0.1），不是 Local Transport。
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

/// 已有的连接 → SSH → Host local → 隔离 tmux session 行。
#[test]
fn linux_catalog_panel_lists_ssh_host_local_tmux() {
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
            LoopbackSshd::start_with_alias("gtk-cat-local", "local").expect("启动 Host local sshd");
        sshd.apply_ssh_config_env();
        let socket = unique_socket("gtk-cat-local");
        create_session(&socket, "mux-ssh-local", 80, 24);
        let _tmux = TmuxGuard {
            socket: socket.clone(),
        };
        std::env::set_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET", &socket);

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

        find_by_name(&app.window, "muxterm-existing-ssh").expect("Home 应有 SSH 目录");
        activate_named(&app, "muxterm-existing-ssh");
        pump_main_loop(40);

        let deadline = Instant::now() + PROBE_TIMEOUT;
        let mut saw_host = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(40);
            if find_by_name(&app.window, "muxterm-existing-host-local").is_some() {
                saw_host = true;
                break;
            }
        }
        assert!(
            saw_host,
            "SSH 目录必须出现 Host local（muxterm-existing-host-local）。loading={:?}",
            find_by_name(&app.window, "muxterm-existing-ssh-loading").is_some()
        );

        activate_named(&app, "muxterm-existing-host-local");
        pump_main_loop(60);
        find_by_name(&app.window, "muxterm-existing-row-tmux-mux-ssh-local").unwrap_or_else(|| {
            panic!("Host local 下必须有隔离 tmux 行 muxterm-existing-row-tmux-mux-ssh-local")
        });

        std::env::remove_var("MUXTERM_TEST_REMOTE_TMUX_SOCKET");
        std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
        app.shutdown();
        pump_main_loop(80);
    });
}
