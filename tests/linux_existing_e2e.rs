//! W20h / C9：面板 click 扁平列表的本地 Herdr 行 → attach → VTE/search_all 含 token。
//!
//! 本 crate 只构造一个 AppWindow。走面板 click，不直接 test_open_spec 冒充。
//! HERDR_CONFIG_DIR 指向临时目录，禁止连用户默认 herdr.sock。
//! runtime list 的等待只驱动 GLib 主循环，禁止 `test_poll_once()` 替生产 poll 收结果。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::*;

use muxterm::core::config::Config;
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

/// 面板：已有的连接 → 本地 → Herdr 行 → attach。
#[test]
fn linux_existing_panel_click_attaches_herdr() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("gtk-exist");
        let (ws, _tab, pane) = herdr.create_workspace("/tmp", "mux-w20h");
        let token = format!("HERDR_LIVE_{}", "w20h");
        herdr.paint(&pane, &token);
        // discover override：只扫测试 socket，禁止连用户默认 herdr.sock。
        std::env::set_var(
            "HERDR_SOCKET_PATH",
            herdr.socket_path().to_string_lossy().to_string(),
        );

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_panel(0);
        pump_main_loop(80);

        // 根 → 已有的连接（扁平列表，不要再点本地目录）。
        let list = find_by_name(&app.window, "muxterm-panel-list")
            .expect("面板列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        find_by_name(&app.window, "muxterm-existing-connections").expect("根列表应有已有的连接");
        let row = list.row_at_index(0).expect("Folder 行");
        row.activate();
        pump_main_loop(60);

        let row_name = format!("muxterm-existing-row-herdr-local-{ws}");
        let deadline = Instant::now() + HERDR_TIMEOUT;
        let mut saw = false;
        while Instant::now() < deadline {
            pump_main_loop(40);
            if find_by_name(&app.window, &row_name).is_some() {
                saw = true;
                break;
            }
        }
        assert!(saw, "扁平列表应有 {row_name}");
        let list = find_by_name(&app.window, "muxterm-panel-list")
            .expect("面板列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        let mut idx = 0usize;
        let herdr_row = loop {
            let Some(row) = list.row_at_index(idx as i32) else {
                panic!("Herdr 行应在列表里");
            };
            if row.widget_name() == row_name {
                break row;
            }
            idx += 1;
        };
        herdr_row.activate();
        pump_main_loop(80);

        assert!(wait_ready(&app), "面板 click 后应有 pane");
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
            "面板 click Herdr 行后 VTE 与 search_all 必须含 {token}。vte={:?} search={:?}",
            app.test_pane_vte_text(pane),
            app.test_search_all(&token)
        );
        assert!(
            app.test_workspace_runtimes().iter().any(|r| r == "herdr"),
            "池里必须是 herdr runtime: {:?}",
            app.test_workspace_runtimes()
        );

        std::env::remove_var("HERDR_SOCKET_PATH");
    });
}
