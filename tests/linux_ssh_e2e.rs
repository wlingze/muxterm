//! W18：loopback SSH attach 的 GTK 路径，断言与本地 attach 一致。
//!
//! 本 crate 只构造一个 AppWindow。禁止 MockRuntime。无 sshd 二进制 skip。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::tmux_available;

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + SSH_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

/// SSH attach 已有 `/bin/cat` 画面：VTE 与 search_all 都能看到 token。
#[test]
fn linux_ssh_attach_shows_preexist_token() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    if !ssh_tmux_available() {
        eprintln!("skip: 无 sshd 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_remote_one_pane("gtk-ssh-att");
        fx.apply_ssh_config_env();

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_spec(WorkspaceSpec::ssh_tmux(
            fx.sshd.alias.clone(),
            Some(fx.session.clone()),
            Some(fx.socket.clone()),
        ));

        assert!(wait_ready(&app), "SSH attach 后应有 pane");
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        let deadline = Instant::now() + SSH_TIMEOUT;
        let mut ok = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&fx.token)
                && !app.test_search_all(&fx.token).is_empty()
            {
                ok = true;
                break;
            }
        }
        assert!(
            ok,
            "SSH attach 后 VTE 与 search_all 必须含 {}（与本地 linux_feature_e2e 同级）。vte={:?} search={:?}",
            fx.token,
            app.test_pane_vte_text(pane),
            app.test_search_all(&fx.token)
        );
        assert!(
            app.test_workspace_replica_ids()
                .iter()
                .any(|id| id.contains(&fx.sshd.alias)),
            "池里必须是 SSH 工作区（replica 含 loopback alias），不能误开成本地。ids={:?}",
            app.test_workspace_replica_ids()
        );
    });
}
