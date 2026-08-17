//! H4 GTK：worktree 创建入口只按 support() 露出。
//!
//! Herdr 格（临时 git 仓库）有 `muxterm-worktree-create` 且可点；
//! 同一 AppWindow 切到 tmux 格后按钮必须消失。本 crate 只构造一个 AppWindow。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::herdr_test_support::{herdr_available, IsolatedHerdr, TempGitRepo};
use support::linux_gtk::*;
use support::tmux_test_support::{create_session, kill_server, tmux_available, unique_socket};

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

const HERDR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 隔离 tmux 夹具（Drop 只杀自己的 -L server）。
struct TmuxGuard {
    socket: String,
}

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

fn wait_workspaces(app: &AppWindow, n: usize) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if app.test_workspace_replica_ids().len() >= n {
            return true;
        }
    }
    false
}

/// Herdr 格有 worktree 创建按钮；tmux 格没有。
#[test]
fn linux_herdr_worktree_button_gated_by_support() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("gtk-wt");
        let repo = TempGitRepo::new("gtk-wt");
        let repo_path = repo.path().to_string_lossy().to_string();
        let (ws, _tab, _pane) = herdr.create_workspace(&repo_path, "mux-wt-gtk");

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_spec(WorkspaceSpec::herdr(
            herdr.name(),
            ws.clone(),
            herdr.socket_path().to_string_lossy().to_string(),
        ));
        assert!(wait_ready(&app), "Herdr attach 后应有 pane");
        pump_main_loop(80);
        app.test_poll_once();

        let btn = find_by_name(&app.window, "muxterm-worktree-create")
            .expect("Herdr 格（support 含 WorktreeList）必须有 worktree 创建按钮");
        assert!(btn.is_sensitive(), "worktree 创建按钮应可点");

        // 同一 AppWindow 再开 tmux 格：激活后按钮必须消失。
        let tmux = TmuxGuard {
            socket: unique_socket("herdr-wt-tmux"),
        };
        create_session(&tmux.socket, "herdr-wt-tmux-sess", 80, 24);
        app.test_open_spec(WorkspaceSpec::local_tmux(
            Some("herdr-wt-tmux-sess".into()),
            Some(tmux.socket.clone()),
        ));
        assert!(
            wait_workspaces(&app, 2),
            "必须同时连上 herdr + tmux 两个工作区"
        );
        pump_main_loop(80);
        app.test_poll_once();
        assert!(
            find_by_name(&app.window, "muxterm-worktree-create").is_none(),
            "tmux 格（support 不含 WorktreeList）不得有 worktree 创建按钮"
        );
    });
}
