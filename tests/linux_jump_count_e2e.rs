//! W18：滚离底部后若有新输出，回底按钮必须显示 +N。
//!
//! 本 crate 只构造一个 AppWindow。按钮 widget_name 仍是 `muxterm-jump-latest`。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::attach_history_contract::{build_offscreen_history, HISTORY_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::{send_keys_line, tmux_available};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

/// 向上看历史时新来 5 行：按钮标签含 + 和数字。
#[test]
fn linux_jump_latest_shows_unseen_count() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_offscreen_history("gtk-jumpn");

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);

        let deadline = Instant::now() + HISTORY_TIMEOUT;
        let mut ready = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if !app.test_layout_leaf_ids().is_empty() {
                ready = true;
                break;
            }
        }
        assert!(ready, "attach 后应有 pane");
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&fx.tail_mark) {
                break;
            }
        }

        app.test_scroll_pane_to_top(pane);
        pump_main_loop(80);
        for i in 1..=5 {
            send_keys_line(
                &fx.socket,
                &fx.pane_target(),
                &format!("UNSEEN_{i}_{}", fx.token),
            );
        }
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        let mut indexed = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if !app
                .test_search_all(&format!("UNSEEN_5_{}", fx.token))
                .is_empty()
            {
                indexed = true;
                break;
            }
        }
        assert!(indexed, "5 行新输出必须进索引");

        let jump = find_by_name(&app.test_window(), "muxterm-jump-latest")
            .expect("muxterm-jump-latest")
            .downcast::<gtk4::Button>()
            .expect("Button");
        assert!(jump.is_visible(), "回底按钮必须可见");
        let label = jump.label().unwrap_or_default();
        let has_plus_n = label.contains('+') && label.chars().any(|c| c.is_ascii_digit());
        assert!(
            has_plus_n,
            "滚离底部且有新输出时，回底按钮必须显示 +N（愿景「↓ 最新 · +37 行」）。label={label:?}"
        );
    });
}
