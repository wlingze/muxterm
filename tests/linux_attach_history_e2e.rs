//! W16a：attach 后必须能搜到 / 滚到 attach 之前已经滚出可见区的历史。
//!
//! 本 crate 只构造一个 AppWindow。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::attach_history_contract::{build_offscreen_history, HISTORY_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::tmux_available;

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_search(app: &AppWindow, token: &str) -> Vec<(String, u32, String)> {
    let deadline = Instant::now() + HISTORY_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let hits = app.test_search_all(token);
        if !hits.is_empty() {
            return hits;
        }
    }
    Vec::new()
}

fn wait_vte_contains(app: &AppWindow, needle: &str) -> bool {
    let deadline = Instant::now() + HISTORY_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if app.test_active_pane_vte_text().contains(needle) {
            return true;
        }
        let ids = app.test_layout_leaf_ids();
        if ids
            .iter()
            .any(|id| app.test_pane_vte_text(*id).contains(needle))
        {
            return true;
        }
    }
    false
}

/// 滚出可见区的 token：搜索能命中，滚到顶 VTE 能看见，回底按钮能回到尾部。
#[test]
fn linux_attach_restores_offscreen_history_and_jump_latest() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_offscreen_history("gtk-hist");

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
        assert!(
            ready,
            "attach 后应有 pane 控件，leaves={:?}",
            app.test_layout_leaf_ids()
        );

        assert!(
            wait_vte_contains(&app, &fx.tail_mark),
            "可见尾标 {} 必须在 VTE 里（attach 播种连当前屏都没有）",
            fx.tail_mark
        );

        let hits = wait_search(&app, &fx.token);
        assert!(
            !hits.is_empty(),
            "WorkspacePool::search_all 必须找到滚出可见区的 {}（只抓 capture-pane 可见屏不够）",
            fx.token
        );

        let pane = *app.test_layout_leaf_ids().first().expect("至少 1 个 pane");
        app.test_scroll_pane_to_top(pane);
        pump_main_loop(80);
        app.test_flush_feeds();
        assert!(
            app.test_pane_vte_text(pane).contains(&fx.token),
            "滚到顶之后 VTE 必须能看见离屏历史 {}，不能只有 attach 那一屏",
            fx.token
        );

        let jump = find_by_name(&app.test_window(), "muxterm-jump-latest")
            .expect("向上滚动后必须出现回底按钮 muxterm-jump-latest");
        assert!(jump.is_visible(), "muxterm-jump-latest 必须可见");
        let btn = jump
            .downcast::<gtk4::Button>()
            .expect("muxterm-jump-latest 应是 Button");
        let _: () = btn.emit_by_name("clicked", &[]);
        pump_main_loop(80);
        app.test_flush_feeds();
        let after = app.test_pane_vte_text(pane);
        assert!(
            !after.contains(&fx.token),
            "点回底之后可见区应回到尾部，不应再显示离屏 token。got={after:?}"
        );
        assert!(
            after.contains(&fx.tail_mark),
            "点回底之后可见区应含尾标 {}",
            fx.tail_mark
        );
    });
}
