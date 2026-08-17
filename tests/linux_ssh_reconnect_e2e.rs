//! W18：SSH 路径上 `detach-client` 后必须自动重连，断言与本地 `linux_reconnect_e2e` 对齐。
//!
//! 本 crate 只构造一个 AppWindow。远端 session 必须还在；禁止连用户默认 tmux。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::ssh_tmux_contract::{build_remote_one_pane, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::tmux_available;

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

fn overlay_visible(app: &AppWindow) -> bool {
    find_by_name(&app.test_window(), "muxterm-disconnect-overlay").is_some_and(|w| w.is_visible())
}

/// detach-client 之后：session 还在，自动重连，断线期间的字和 BEL 都还在。
#[test]
fn linux_ssh_detach_client_reconnects_without_losing_gap() {
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
        let fx = build_remote_one_pane("gtk-ssh-re");
        fx.apply_ssh_config_env();
        let gap = format!("GAP_{}", fx.token);

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

        let deadline = Instant::now() + SSH_TIMEOUT;
        let mut attached = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if !app.test_search_all(&fx.token).is_empty() {
                attached = true;
                break;
            }
        }
        assert!(attached, "重连前必须已经 SSH attach 到 {}", fx.token);
        let resets_before = app.test_active_pane_resets();
        app.test_clear_active_pane_render_trace();

        fx.detach_clients();
        assert!(fx.has_session(), "detach-client 不得杀掉远端 session");

        fx.send_keys_line(&gap);
        fx.send_bel();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut reconnected = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if !overlay_visible(&app) && !app.test_search_all(&gap).is_empty() {
                reconnected = true;
                break;
            }
        }
        assert!(app.window.is_visible(), "重连过程中窗口必须留下");
        assert!(
            reconnected,
            "SSH detach-client 后 15s 内必须自动重连：水印消失，且搜得到 {gap}。overlay={} search={:?}",
            overlay_visible(&app),
            app.test_search_all(&gap)
        );
        assert!(
            !overlay_visible(&app),
            "重连成功后 muxterm-disconnect-overlay 必须隐藏"
        );
        assert!(
            !app.test_search_all(&fx.token).is_empty(),
            "重连后原来的 {} 必须还在索引里",
            fx.token
        );
        assert!(
            app.test_attention_blocked_workspaces() >= 1
                || app.test_notifications_recorded().iter().any(|n| n
                    .to_lowercase()
                    .contains("attention")
                    || n.to_lowercase().contains("blocked")
                    || n.contains("需要")),
            "断线期间的 BEL 重连后必须进 blocked / 通知。blocked={} log={:?}",
            app.test_attention_blocked_workspaces(),
            app.test_notifications_recorded()
        );
        let resets_after = app.test_active_pane_resets();
        assert!(
            resets_after <= 1,
            "重连 catch-up 必须 seed_raw，不得 vte.reset。resets_before={resets_before} after={resets_after}"
        );
    });
}
