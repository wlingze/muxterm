//! 隐藏 SSH pane 的 Pause → Live 恢复：先保留最后已知帧，再异步补 baseline。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;

use muxterm::test_support::core::config::Config;
use muxterm::test_support::core::workspace::WorkspaceSpec;
use muxterm::test_support::platform::linux::window::AppWindow;

use support::linux_gtk::*;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::tmux_available;

fn wait_for_text(app: &AppWindow, pane: u32, text: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if app.test_pane_vte_text(pane).contains(text) {
            return true;
        }
    }
    false
}

#[test]
fn hidden_ssh_pane_keeps_last_frame_before_async_resume_baseline() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() || !ssh_tmux_available() {
        eprintln!("skip: 无 tmux 或 sshd 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_remote_one_pane("gtk-ssh-pause-resume");
        fx.apply_ssh_config_env();

        let app = AppWindow::new(Config::default(), load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        let local_replica = app.test_active_workspace_replica_id();
        app.test_open_spec(WorkspaceSpec::ssh_tmux(
            fx.sshd.alias.clone(),
            Some(fx.session.clone()),
            Some(fx.socket.clone()),
        ));

        assert!(
            wait_until_widget(SSH_TIMEOUT.as_millis() as u64, || {
                !app.test_layout_leaf_ids().is_empty()
            }),
            "SSH attach 后应有 pane"
        );
        let pane = *app.test_layout_leaf_ids().first().expect("SSH pane");
        assert!(
            wait_for_text(&app, pane, &fx.token, SSH_TIMEOUT),
            "SSH 首帧必须含 {}，got={:?}",
            fx.token,
            app.test_pane_vte_text(pane)
        );
        let ssh_replica = app
            .test_workspace_replica_ids()
            .into_iter()
            .find(|replica| replica.contains(&fx.sshd.alias))
            .expect("池里必须有 SSH workspace");

        // 切回本地并让 16ms poll 将隐藏 SSH pane 降到 Pause。
        app.test_activate_workspace(&local_replica);
        pump_main_loop(120);
        assert_eq!(app.test_active_workspace_replica_id(), local_replica);

        let hidden_marker = unique_marker("hidden-");
        fx.send_keys_line(&hidden_marker);
        pump_main_loop(80);

        // Scene 切换本身不 poll Core，也不等待 snapshot；最后已知帧必须仍在。
        app.test_activate_workspace(&ssh_replica);
        let before_baseline = app.test_pane_vte_text(pane);
        assert!(
            before_baseline.contains(&fx.token),
            "切回隐藏 SSH pane 时应先显示最后已知帧，不能白屏: {before_baseline:?}"
        );

        // 下一轮 poll 发出 RequestPaneSnapshot，随后 baseline 覆盖并带上隐藏期间的输出。
        assert!(
            wait_for_text(&app, pane, &hidden_marker, SSH_TIMEOUT),
            "异步 baseline 后应出现隐藏期间的新内容 {hidden_marker}，got={:?}",
            app.test_pane_vte_text(pane)
        );

        app.shutdown();
        pump_main_loop(120);
    });
}
