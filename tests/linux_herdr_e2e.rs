//! H2 GTK：Herdr attach 的 GTK 路径，断言与本地/SSH attach 一致。
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

/// 夹具先涂 token 再 GTK attach：VTE 与 search_all 都能看到，且池里
/// 必须是 herdr runtime（不能误开成本地 tmux）。
#[test]
fn linux_herdr_attach_shows_preexist_token() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("gtk-att");
        let (ws, _tab, pane) = herdr.create_workspace("/tmp", "mux-h2-gtk");
        let token = format!("HERDR_LIVE_{}", "gtk-att");
        herdr.paint(&pane, &token);

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_spec(WorkspaceSpec::herdr(
            herdr.name(),
            ws.clone(),
            herdr.socket_path().to_string_lossy().to_string(),
        ));

        assert!(wait_ready(&app), "Herdr attach 后应有 pane");
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        let deadline = Instant::now() + HERDR_TIMEOUT;
        let mut ok = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&token)
                && !app.test_search_all(&token).is_empty()
            {
                ok = true;
                break;
            }
        }
        assert!(
            ok,
            "Herdr attach 后 VTE 与 search_all 必须含 {token}。vte={:?} search={:?}",
            app.test_pane_vte_text(pane),
            app.test_search_all(&token)
        );
        assert!(
            app.test_workspace_runtimes().iter().any(|r| r == "herdr"),
            "池里必须是 Herdr 工作区（runtime=herdr），不能误开成本地 tmux。runtimes={:?}",
            app.test_workspace_runtimes()
        );
    });
}
