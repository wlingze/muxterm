//! W16c：blocked 看见不熄、输入才熄；TOML 正则能点亮后台 pane。
//!
//! 本 crate 只构造一个 AppWindow。真 BEL 走 `%output`，禁止 `test_feed_replica`。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::feature_e2e_contract::*;
use support::linux_gtk::*;
use support::tmux_test_support::{send_keys_line, tmux_available};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_blocked(app: &AppWindow, min: usize) -> bool {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if app.test_attention_blocked_workspaces() >= min {
            return true;
        }
    }
    false
}

fn wait_blocked_eq(app: &AppWindow, n: usize) -> bool {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if app.test_attention_blocked_workspaces() == n {
            return true;
        }
    }
    false
}

/// 愿景 §2.15.1：blocked 不是「打开即已读」。正则不靠 BEL。
#[test]
fn linux_blocked_survives_view_clears_on_input_regex_lights() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_two_pane_cat("gtk-attn-sem");

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        cfg.attention.enabled = true;
        cfg.attention.debounce_ms = 50;
        cfg.attention.blocked_regex = vec!["NEED_INPUT".into()];
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);

        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut ready = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_layout_leaf_ids().len() >= 2 {
                ready = true;
                break;
            }
        }
        assert!(
            ready,
            "attach 后应有 2 个 pane，leaves={:?}",
            app.test_layout_leaf_ids()
        );

        let pane0 = fx.panes[0];
        let pane1 = fx.panes[1];
        app.test_switch_pane(pane0);
        pump_main_loop(50);

        send_background_bel(&fx.socket, &fx.pane_target(1));
        assert!(
            wait_blocked(&app, 1),
            "后台 pane 真 BEL 必须点亮 blocked。count={}",
            app.test_attention_blocked_workspaces()
        );

        app.test_switch_pane(pane1);
        pump_main_loop(80);
        app.test_poll_once();
        assert!(
            app.test_attention_blocked_workspaces() >= 1,
            "切到 blocked pane 只是看见，红点必须还在。count={}",
            app.test_attention_blocked_workspaces()
        );

        app.test_send_input(b"x");
        assert!(
            wait_blocked_eq(&app, 0),
            "对该 pane 输入之后 blocked 必须熄灭。count={}",
            app.test_attention_blocked_workspaces()
        );

        app.test_switch_pane(pane0);
        pump_main_loop(50);
        send_keys_line(&fx.socket, &fx.pane_target(1), "NEED_INPUT");
        assert!(
            wait_blocked(&app, 1),
            "后台 pane 写出 TOML 正则 NEED_INPUT 必须再点亮 blocked（不许要求 BEL）。count={}",
            app.test_attention_blocked_workspaces()
        );
    });
}
