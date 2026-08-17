//! W17b：向上滚之后新输出不得把视口拽回底部（scroll lock）。
//!
//! 本 crate 只构造一个 AppWindow。回底按钮本身由 `linux_attach_history_e2e` 覆盖。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::attach_history_contract::{build_offscreen_history, HISTORY_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::{send_keys_line, tmux_available};

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

/// 滚到顶看历史时，新行只能进索引，不能把可见区拽到尾部。
#[test]
fn linux_scroll_up_does_not_follow_new_output() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_offscreen_history("gtk-lock");
        let lock_token = format!("LOCK_NEW_{}", fx.token);

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
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        let mut tailed = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&fx.tail_mark) {
                tailed = true;
                break;
            }
        }
        assert!(tailed, "先应看到尾标 {}", fx.tail_mark);

        app.test_scroll_pane_to_top(pane);
        pump_main_loop(80);
        app.test_flush_feeds();
        assert!(
            app.test_pane_vte_text(pane).contains(&fx.token),
            "滚到顶必须看见离屏历史 {}",
            fx.token
        );
        let jump = find_by_name(&app.test_window(), "muxterm-jump-latest")
            .expect("滚离底部后必须有 muxterm-jump-latest");
        assert!(jump.is_visible(), "scroll lock 期间回底按钮必须可见");

        send_keys_line(&fx.socket, &fx.pane_target(), &lock_token);
        let mut indexed = false;
        let deadline = Instant::now() + HISTORY_TIMEOUT;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if !app.test_search_all(&lock_token).is_empty() {
                indexed = true;
                break;
            }
        }
        assert!(
            indexed,
            "新输出必须进 PaneBuf（search_all 能找到 {lock_token}）"
        );
        app.test_flush_feeds();
        let visible = app.test_pane_vte_text(pane);
        assert!(
            visible.contains(&fx.token),
            "新输出不得把视口拽离历史。仍应看见 {}。visible={visible:?}",
            fx.token
        );
        assert!(
            !visible.contains(&lock_token),
            "scroll lock 时可见区不应跳到最新行 {lock_token}。visible={visible:?}"
        );
        assert!(
            find_by_name(&app.test_window(), "muxterm-jump-latest").is_some_and(|w| w.is_visible()),
            "仍离开底部时回底按钮必须还在"
        );
    });
}
