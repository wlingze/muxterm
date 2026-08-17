//! W17c：搜索命中在 scrollback 里时，跳转必须滚到那一行，并标出 muxterm-search-highlight。
//!
//! 本 crate 只构造一个 AppWindow。`linux_search_jump_e2e` 只证明切 tab，不证明滚到 seq。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::attach_history_contract::{build_offscreen_history, HISTORY_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::tmux_available;

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

/// 可见区没有离屏 token 时，点搜索命中必须把 VTE 滚到该行并显示高亮。
#[test]
fn linux_search_jump_scrolls_offscreen_hit_and_highlights() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_offscreen_history("gtk-shigh");

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
        assert!(tailed, "默认应停在尾部，看见 {}", fx.tail_mark);
        assert!(
            !app.test_pane_vte_text(pane).contains(&fx.token),
            "跳转前可见区不得已有离屏 token，否则测不到滚到 seq"
        );

        let hits = {
            let deadline = Instant::now() + HISTORY_TIMEOUT;
            let mut found = Vec::new();
            while Instant::now() < deadline {
                app.test_poll_once();
                pump_main_loop(30);
                found = app.test_search_all(&fx.token);
                if !found.is_empty() {
                    break;
                }
            }
            found
        };
        assert!(!hits.is_empty(), "PaneBuf 必须能搜到 {}", fx.token);

        app.test_open_panel(2);
        pump_main_loop(80);
        let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
            .expect("搜索 Entry")
            .downcast::<gtk4::Entry>()
            .expect("Entry");
        entry.set_text(&fx.token);
        pump_main_loop(120);
        let hit = find_by_name_prefix(&app.test_window(), "muxterm-search-hit-")
            .expect("必须出现 muxterm-search-hit-*");
        if let Ok(row) = hit.downcast::<gtk4::ListBoxRow>() {
            let _: () = row.emit_by_name("activate", &[]);
        }
        pump_main_loop(120);
        app.test_poll_once();
        app.test_flush_feeds();

        assert!(!app.test_panel_open(), "搜索跳转后面板必须关掉");
        assert!(
            app.test_pane_vte_text(pane).contains(&fx.token),
            "跳转后 VTE 可见区必须含离屏命中 {}（只切 pane 不滚不够）",
            fx.token
        );
        let mark = find_by_name(&app.test_window(), "muxterm-search-highlight")
            .expect("跳到命中行后必须出现 muxterm-search-highlight");
        assert!(mark.is_visible(), "muxterm-search-highlight 必须可见");
    });
}
