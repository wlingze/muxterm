//! H3 GTK：同一 socket 两格 Herdr workspace，切过去 VTE 仍有各自 token。
//!
//! 本 crate 只构造一个 AppWindow。禁止 MockRuntime。无 herdr 二进制 skip。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::*;

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

const HERDR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

fn wait_vte_contains(app: &AppWindow, pane: u32, token: &str) -> bool {
    let deadline = Instant::now() + HERDR_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if app.test_pane_vte_text(pane).contains(token) {
            return true;
        }
    }
    false
}

/// 两格都打开后切回 A：VTE 仍含 A token，search_all 两个 token 都在。
#[test]
fn linux_herdr_switch_keeps_tokens() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("gtk-switch");
        let (ws_a, _ta, pane_a) = herdr.create_workspace("/tmp", "mux-a");
        let (ws_b, _tb, pane_b) = herdr.create_workspace("/tmp", "mux-b");
        let token_a = format!("HERDR_LIVE_{}", "switch-a");
        let token_b = format!("HERDR_LIVE_{}", "switch-b");
        herdr.paint(&pane_a, &token_a);
        herdr.paint(&pane_b, &token_b);

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        let socket = herdr.socket_path().to_string_lossy().to_string();
        let spec_a = WorkspaceSpec::herdr(herdr.name(), ws_a.clone(), socket.clone());
        let spec_b = WorkspaceSpec::herdr(herdr.name(), ws_b.clone(), socket);
        let replica_a = spec_a.id().replica_id();
        let replica_b = spec_b.id().replica_id();
        assert_ne!(replica_a, replica_b, "同一 session 两格 replica 必须可区分");
        app.test_open_spec(spec_a);
        assert!(wait_ready(&app), "A attach 后应有 pane");
        app.test_open_spec(spec_b);

        let deadline = Instant::now() + HERDR_TIMEOUT;
        let mut two = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_workspace_replica_ids().len() >= 2
                && !app.test_search_all(&token_b).is_empty()
            {
                two = true;
                break;
            }
        }
        assert!(
            two,
            "必须同时连上两个工作区。ids={:?} b_hits={:?}",
            app.test_workspace_replica_ids(),
            app.test_search_all(&token_b)
        );

        // 当前激活是 B：VTE 应含 B token。
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        assert!(
            wait_vte_contains(&app, pane, &token_b),
            "B 激活时 VTE 必须含 {token_b}。vte={:?}",
            app.test_pane_vte_text(pane)
        );

        // 切回 A：VTE 应含 A token，且 search_all 两个 token 都在。
        app.test_activate_workspace(&replica_a);
        pump_main_loop(80);
        app.test_poll_once();
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        assert!(
            wait_vte_contains(&app, pane, &token_a),
            "切回 A 后 VTE 必须含 {token_a}。vte={:?}",
            app.test_pane_vte_text(pane)
        );
        assert!(
            !app.test_search_all(&token_a).is_empty(),
            "search_all 必须仍含 A token"
        );
        assert!(
            !app.test_search_all(&token_b).is_empty(),
            "search_all 必须仍含 B token"
        );
    });
}
