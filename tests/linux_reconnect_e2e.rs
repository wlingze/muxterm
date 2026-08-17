//! W17a：control client 被 detach 后必须自动重连。session 还在，断线期间的输出和 BEL 不能丢。
//!
//! 本 crate 只构造一个 AppWindow。不要用 kill-server（那是 W16b 水印）。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::feature_e2e_contract::send_background_bel;
use support::linux_gtk::*;
use support::tmux_test_support::{
    detach_all_clients, has_session, kill_server, list_pane_ids, send_keys_line, tmux_available,
    unique_socket, wait_capture_contains,
};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

const TIMEOUT: Duration = Duration::from_secs(15);

struct OnePaneCat {
    socket: String,
    session: String,
    token: String,
}

impl Drop for OnePaneCat {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

fn build_one_pane(label: &str) -> OnePaneCat {
    let socket = unique_socket(label);
    let session = format!("reconn-{label}");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let token = format!("RECONN_LIVE_{suffix}");

    let output = std::process::Command::new("tmux")
        .args([
            "-L",
            &socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ])
        .output()
        .expect("new-session");
    assert!(
        output.status.success(),
        "new-session 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ids = list_pane_ids(&socket, &session);
    assert_eq!(ids.len(), 1, "应有 1 pane: {ids:?}");
    send_keys_line(&socket, &format!("%{}", ids[0]), &token);
    wait_capture_contains(
        &socket,
        &format!("%{}", ids[0]),
        &token,
        Duration::from_secs(3),
    );
    OnePaneCat {
        socket,
        session,
        token,
    }
}

fn overlay_visible(app: &AppWindow) -> bool {
    find_by_name(&app.test_window(), "muxterm-disconnect-overlay").is_some_and(|w| w.is_visible())
}

fn wait_vte_or_search(app: &AppWindow, needle: &str) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if app.test_active_pane_vte_text().contains(needle) {
            return true;
        }
        if !app.test_search_all(needle).is_empty() {
            return true;
        }
    }
    false
}

/// detach-client 之后：session 还在，自动重连，断线期间的字和 BEL 都还在。
#[test]
fn linux_detach_client_reconnects_without_losing_gap() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_one_pane("gtk-reconn");
        let gap = format!("GAP_{}", fx.token);

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);

        assert!(
            wait_vte_or_search(&app, &fx.token),
            "重连前 VTE/搜索必须已有 {}",
            fx.token
        );
        let resets_before = app.test_active_pane_resets();
        app.test_clear_active_pane_render_trace();

        let pane = format!("%{}", list_pane_ids(&fx.socket, &fx.session)[0]);
        detach_all_clients(&fx.socket, &fx.session);
        assert!(
            has_session(&fx.socket, &fx.session),
            "detach-client 不得杀掉 session"
        );

        send_keys_line(&fx.socket, &pane, &gap);
        send_background_bel(&fx.socket, &pane);

        let deadline = Instant::now() + TIMEOUT;
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
            "detach-client 后 15s 内必须自动重连：水印消失，且搜得到断线期间写入的 {gap}。overlay={} search={:?}",
            overlay_visible(&app),
            app.test_search_all(&gap)
        );
        assert!(
            !overlay_visible(&app),
            "重连成功后 muxterm-disconnect-overlay 必须隐藏"
        );
        assert!(
            wait_vte_or_search(&app, &fx.token),
            "重连后原来的画面/索引必须还在 {}",
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
            "重连 catch-up 必须 seed_raw，不得 vte.reset 刷屏。resets_before={resets_before} after={resets_after}"
        );
    });
}
