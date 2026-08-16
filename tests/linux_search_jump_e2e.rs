//! W15b：搜索命中在另一个 tab 时，激活必须切 tab + VTE 含 token + 关面板。
//!
//! 本 crate 只构造一个 AppWindow。`linux_search_e2e` 的 Mock 跳转不算本契约。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::tmux_test_support::tmux_available;
use support::workspace_attach_contract::{build_painted_2tab_3pane, ATTACH_TIMEOUT};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let (tabs, _) = app.test_tab_and_pane_counts();
        if tabs >= 2 {
            return true;
        }
    }
    false
}

fn wait_vte_contains(app: &AppWindow, needle: &str) -> bool {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        let mut text = String::new();
        for id in app.test_layout_leaf_ids() {
            text.push_str(&app.test_pane_vte_text(id));
        }
        if text.contains(needle) {
            return true;
        }
    }
    false
}

#[test]
fn search_hit_on_other_tab_switches_tab_and_closes_panel() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let painted = build_painted_2tab_3pane("gtk-sj");

        let mut cfg = Config::default();
        cfg.tmux.socket = painted.socket.clone();
        cfg.tmux.default_session = painted.session.clone();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);
        assert!(wait_ready(&app), "attach 后应有 2 个 tab");

        let tabs = app.test_tab_ids();
        let tab1 = app.test_active_tab_id();
        let tab2 = tabs
            .iter()
            .copied()
            .find(|t| *t != tab1)
            .expect("应有第二个 tab");

        let hits = {
            let deadline = Instant::now() + ATTACH_TIMEOUT;
            let mut found = Vec::new();
            while Instant::now() < deadline {
                app.test_poll_once();
                pump_main_loop(30);
                found = app.test_search_all(&painted.tab2_token);
                if !found.is_empty() {
                    break;
                }
            }
            found
        };
        assert!(
            !hits.is_empty(),
            "PaneBuf 必须能搜到 tab2 token {}",
            painted.tab2_token
        );

        app.test_open_panel(2);
        pump_main_loop(80);
        let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
            .expect("搜索 Entry")
            .downcast::<gtk4::Entry>()
            .expect("Entry");
        entry.set_text(&painted.tab2_token);
        pump_main_loop(120);
        let hit = find_by_name_prefix(&app.test_window(), "muxterm-search-hit-")
            .expect("必须出现 muxterm-search-hit-*");
        if let Ok(row) = hit.downcast::<gtk4::ListBoxRow>() {
            let _: () = row.emit_by_name("activate", &[]);
        }
        pump_main_loop(80);
        app.test_poll_once();
        pump_main_loop(80);

        assert!(!app.test_panel_open(), "搜索跳转后面板必须关掉");
        assert_eq!(
            app.test_active_tab_id(),
            tab2,
            "命中在 tab 2，跳转后当前 tab 必须是 {tab2}，实际 {}",
            app.test_active_tab_id()
        );
        assert!(
            wait_vte_contains(&app, &painted.tab2_token),
            "跳转后 VTE 必须含 {}",
            painted.tab2_token
        );

        app.shutdown();
        pump_main_loop(250);
    });
}
