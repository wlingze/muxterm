//! W18：切走再切回，必须标出「上次看到这里」。
//!
//! 本 crate 只构造一个 AppWindow。覆盖层是客户端的，不得改 pane 字节。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::feature_e2e_contract::{build_two_pane_cat, FEATURE_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::{send_keys_line, tmux_available};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_search(app: &AppWindow, token: &str) -> bool {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_search_all(token).is_empty() {
            return true;
        }
    }
    false
}

/// 离开 pane 后再回来：出现 muxterm-last-seen，点它滚到离开时的那一行。
#[test]
fn linux_last_seen_mark_on_return() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_two_pane_cat("gtk-last");
        let left_here = format!("LEFT_HERE_{}", fx.search_token);
        let after = format!("AFTER_LEAVE_{}", fx.search_token);

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
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
        assert!(ready, "应有 2 个 pane");
        assert!(wait_search(&app, &fx.search_token), "先看到 pane0 token");

        send_keys_line(&fx.socket, &fx.pane_target(0), &left_here);
        assert!(wait_search(&app, &left_here), "离开前的地标必须进索引");

        app.test_switch_pane(fx.panes[1]);
        pump_main_loop(80);
        app.test_poll_once();

        send_keys_line(&fx.socket, &fx.pane_target(0), &after);
        for i in 1..=24 {
            send_keys_line(&fx.socket, &fx.pane_target(0), &format!("more-{i}"));
        }
        assert!(wait_search(&app, &after), "离开期间的新行必须进索引");

        app.test_switch_pane(fx.panes[0]);
        pump_main_loop(120);
        app.test_poll_once();
        app.test_flush_feeds();

        let mark = find_by_name(&app.test_window(), "muxterm-last-seen")
            .expect("切回 pane 后必须出现 muxterm-last-seen");
        assert!(mark.is_visible(), "muxterm-last-seen 必须可见");

        if let Ok(btn) = mark.clone().downcast::<gtk4::Button>() {
            let _: () = btn.emit_by_name("clicked", &[]);
        } else if let Ok(row) = mark.downcast::<gtk4::ListBoxRow>() {
            let _: () = row.emit_by_name("activate", &[]);
        }
        pump_main_loop(80);
        app.test_flush_feeds();
        let visible = app.test_pane_vte_text(fx.panes[0]);
        assert!(
            visible.contains(&left_here),
            "点「上次看到这里」必须滚到离开时的行 {left_here}。visible={visible:?}"
        );
    });
}
