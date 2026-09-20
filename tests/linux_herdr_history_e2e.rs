//! 真实 Herdr：GTK 分配尺寸、服务端历史与回到当前屏。
#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use muxterm::test_support::core::{config::Config, workspace::WorkspaceSpec};
use muxterm::test_support::frontend::linux::keymap::Action;
use muxterm::test_support::frontend::linux::window::AppWindow;
use std::time::{Duration, Instant};
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::*;

fn until(app: &AppWindow, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Herdr convergence timed out: {:?}",
            app.test_active_pane_vte_text()
        );
    }
}

#[test]
fn herdr_history_and_allocation_reach_the_real_surface() {
    if skip_no_display() || !herdr_available() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("gtk-history");
        let (ws, _, pane) = herdr.create_workspace("/tmp", "history");
        let app = AppWindow::new(Config::default(), load_theme());
        app.window.set_default_size(1600, 800);
        app.window.set_size_request(1600, 800);
        app.window.present();
        pump_main_loop(150);
        app.test_open_spec(WorkspaceSpec::herdr(
            herdr.name(),
            ws,
            herdr.socket_path().to_string_lossy().to_string(),
        ));
        until(&app, || !app.test_layout_leaf_ids().is_empty());
        let id = app.test_active_pane_id();
        // Ready 后让实际 GTK allocation 通过生产 resize 队列到达 PTY。
        for _ in 0..30 {
            app.test_poll_once();
            pump_main_loop(30);
        }
        let status_label = find_by_name(&app.window, "muxterm-status-popover-label")
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap();
        assert!(
            status_label.text().contains("status=connected"),
            "real Herdr status must be connected: {}",
            status_label.text()
        );
        // 由夹具产生输出，禁止先向 frontend 输入：输入会 promote controller，
        // 掩盖仅打开/切换 tab 时 PTY 尺寸未送达的缺陷。
        herdr.paint(&pane, r"printf '\033[0m\033[2J\033[H'; i=0; while [ $i -lt 160 ]; do printf 'HISTORY_ROW_%03d\n' $i; i=$((i+1)); done; printf 'PTY_GRID='; stty size; printf 'LIVE_BOTTOM\n'");
        until(&app, || {
            app.test_pane_screen_text(id)
                .lines()
                .any(|line| line.trim() == "LIVE_BOTTOM")
        });
        let text = app.test_pane_screen_text(id);
        let grid = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("PTY_GRID="))
            .unwrap_or_else(|| panic!("PTY size output: {text:?}"));
        let cols: u16 = grid.split_whitespace().last().unwrap().parse().unwrap();
        assert!(cols > 120, "fixture must exercise a wide terminal");
        assert_eq!(
            cols,
            app.test_pane_allocated_grid(id).0,
            "PTY must match actual GTK allocation before input: {grid}"
        );
        assert!(!text.contains("HISTORY_ROW_000"));
        for _ in 0..100 {
            app.test_emit_scroll(id, -3.0);
        }
        until(&app, || {
            app.test_pane_screen_text(id).contains("HISTORY_ROW_000")
        });
        for _ in 0..100 {
            app.test_emit_scroll(id, 3.0);
        }
        until(&app, || {
            app.test_pane_screen_text(id)
                .lines()
                .any(|line| line.trim() == "LIVE_BOTTOM")
        });
        let first_tab = app.test_active_tab_id();
        app.test_handle_action(Action::NewTab);
        until(&app, || {
            app.test_tab_ids().len() == 2 && app.test_active_tab_id() != first_tab
        });
        app.test_handle_action(Action::SwitchTab1);
        until(&app, || {
            app.test_active_tab_id() == first_tab
                && app.test_pane_screen_text(id).contains("LIVE_BOTTOM")
        });
        // 切回来后同一 surface 的滚轮仍指向原 workspace/pane。
        for _ in 0..100 {
            app.test_emit_scroll(id, -3.0);
        }
        until(&app, || {
            app.test_pane_screen_text(id).contains("HISTORY_ROW_000")
        });
        for _ in 0..100 {
            app.test_emit_scroll(id, 3.0);
        }
        until(&app, || {
            app.test_pane_screen_text(id).contains("LIVE_BOTTOM")
        });
        app.window.close();
    });
}
