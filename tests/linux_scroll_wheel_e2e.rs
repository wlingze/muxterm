//! W21d：真 tmux attach 后滚轮走生产路径——主屏滚历史，alt-screen 发 CSI A。
//!
//! 本 crate 只构造一个 AppWindow。隔离 tmux `-L muxterm-test-*`。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::attach_history_contract::{build_offscreen_history, HISTORY_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::{tmux_available, tmux_ok};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + HISTORY_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

/// 主屏滚轮看到离屏历史；alt-screen 滚轮把 CSI A 送进 input 路径。
#[test]
fn linux_wheel_scrolls_history_and_sends_alt_screen_arrows() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_offscreen_history("gtk-wheel");
        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);
        assert!(wait_ready(&app), "attach 后应有 pane");

        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        // 等 attach 快照进 VTE。
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        let mut seeded = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&fx.tail_mark) {
                seeded = true;
                break;
            }
        }
        assert!(seeded, "attach 后 VTE 应含尾标 {}", fx.tail_mark);

        // 主屏：生产滚轮路径滚到顶，离屏 token 出现。
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        let mut saw_token = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            // 一格 3 行；离屏 80 行需要 ~30 格。
            for _ in 0..30 {
                app.test_emit_scroll(pane, -1.0);
            }
            pump_main_loop(30);
            if app.test_pane_vte_text(pane).contains(&fx.token) {
                saw_token = true;
                break;
            }
        }
        assert!(
            saw_token,
            "滚轮向上必须看到离屏 token {}。vte={:?}",
            fx.token,
            app.test_pane_vte_text(pane)
        );

        // alt-screen：把 pane 换成「先进 1049 再留在 cat」的 shell，
        // 滚轮 → input_cb 收到 CSI A。
        tmux_ok(
            &fx.socket,
            &[
                "respawn-pane",
                "-k",
                "-t",
                &format!("%{}", fx.pane),
                "--",
                "sh",
                "-c",
                "printf '\x1b[?1049h'; exec /bin/cat",
            ],
        );
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        let mut alt = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_pane_alternate_screen(pane) {
                alt = true;
                break;
            }
        }
        assert!(alt, "respawn 后 pane 应进入 alt-screen");

        app.test_emit_scroll(pane, -1.0);
        pump_main_loop(60);
        let last = app.test_last_raw_input();
        assert!(
            last.starts_with(b"\x1b[A"),
            "alt-screen 滚轮必须经 input 路径发 CSI A: {last:?}"
        );
    });
}
