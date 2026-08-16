//! W13 Linux Surface：attach 已有 2tab/3pane 必须能看见、几何对、洪水不白屏。
//!
//! 本 crate 只构造一个 AppWindow（Mesa 限制）。夹具先于窗口创建。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::tmux_test_support::{respawn_cup_flood, tmux_available};
use support::workspace_attach_contract::{
    build_painted_2tab_3pane, ATTACH_TIMEOUT, CUP_FLOOD_FRAMES, MAX_OUTPUT_EVENTS_PER_SEC,
    MIN_PANE_PX,
};

use muxterm::core::config::{Config, Theme};
use muxterm::platform::linux::window::AppWindow;

fn theme() -> Theme {
    load_theme()
}

fn all_visible_vte(app: &AppWindow) -> String {
    app.test_flush_feeds();
    let mut text = String::new();
    for id in app.test_layout_leaf_ids() {
        text.push_str(&app.test_pane_vte_text(id));
        text.push('\n');
    }
    text
}

fn wait_vte_contains(app: &AppWindow, needle: &str) -> bool {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if all_visible_vte(app).contains(needle) {
            return true;
        }
    }
    false
}

fn click_status_tab(app: &AppWindow, tab: u32) {
    let btn = find_by_name(&app.test_window(), &format!("muxterm-status-tab-{tab}"))
        .expect("status tab 按钮")
        .downcast::<gtk4::Button>()
        .expect("Button");
    let _: () = btn.emit_by_name("clicked", &[]);
}

/// 1820.log 用户路径：先有画面，再 attach，VTE 不能是白的，3 pane 必须有面积。
#[test]
fn linux_attach_preexist_2tab_3pane_is_usable() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let painted = build_painted_2tab_3pane("gtk-attach");

        let mut cfg = Config::default();
        cfg.tmux.socket = painted.socket.clone();
        cfg.tmux.default_session = painted.session.clone();
        let app = AppWindow::new(cfg, theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);

        let deadline = Instant::now() + ATTACH_TIMEOUT;
        let mut ready = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            let (tabs, panes) = app.test_tab_and_pane_counts();
            let leaves = app.test_layout_leaf_ids();
            if tabs >= 2 && panes >= 3 && leaves.len() == 3 {
                ready = true;
                break;
            }
        }
        assert!(
            ready,
            "attach 后应有 2 tab / 3 pane 布局，实际 tabs/panes={:?} leaves={:?}",
            app.test_tab_and_pane_counts(),
            app.test_layout_leaf_ids()
        );

        app.test_flush_feeds();
        for id in app.test_layout_leaf_ids() {
            let (w, h) = app.test_pane_allocation(id);
            assert!(
                w >= MIN_PANE_PX && h >= MIN_PANE_PX,
                "pane {id} 分配 {w}x{h} < {MIN_PANE_PX}px（白屏/错布局）"
            );
        }
        let paned = count_paned(&app.test_window());
        assert!(paned >= 2, "3 pane 应有 ≥2 个 GtkPaned，实际 {paned}");

        let vte = all_visible_vte(&app);
        assert!(!vte.trim().is_empty(), "attach 后 VTE 不能空（1820 白屏）");
        for token in &painted.tab1_tokens {
            assert!(
                wait_vte_contains(&app, token),
                "可见 pane 应含播种 token {token}。vte={:?}",
                all_visible_vte(&app)
            );
        }

        let tabs = app.test_tab_ids();
        let current = app.test_active_tab_id();
        let other = tabs
            .iter()
            .copied()
            .find(|t| *t != current)
            .expect("应有第二个 tab");
        click_status_tab(&app, other);
        assert!(
            wait_vte_contains(&app, &painted.tab2_token),
            "切到 tab 2 应看到 {}",
            painted.tab2_token
        );

        click_status_tab(&app, current);
        for token in &painted.tab1_tokens {
            assert!(
                wait_vte_contains(&app, token),
                "切回 tab 1 像素缓存应仍有 {token}"
            );
        }

        app.test_clear_active_pane_render_trace();
        let resets_before = app.test_active_pane_resets();
        let flood_target = painted.pane_target(painted.tab1_panes[0]);
        respawn_cup_flood(&painted.socket, &flood_target, CUP_FLOOD_FRAMES);

        let window = Duration::from_secs(1);
        let start = Instant::now();
        let mut output_events = 0usize;
        while start.elapsed() < window {
            output_events += app.test_poll_output_event_count();
            pump_main_loop(16);
        }
        assert!(
            output_events <= MAX_OUTPUT_EVENTS_PER_SEC,
            "GTK 1s 内 PaneOutput={output_events} > {MAX_OUTPUT_EVENTS_PER_SEC}（1820 CPU）"
        );

        let deadline = Instant::now() + ATTACH_TIMEOUT;
        let mut flood_ok = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            let text = all_visible_vte(&app);
            if text.contains("FLOOD_DONE") || text.contains("frame-") {
                flood_ok = true;
                assert!(!text.trim().is_empty(), "洪水后 VTE 仍不能空");
                break;
            }
        }
        assert!(flood_ok, "CUP 洪水后 VTE 应留下末帧，不能白屏");
        let resets_after = app.test_active_pane_resets();
        assert!(
            resets_after.saturating_sub(resets_before) <= 1,
            "洪水不得 reset 追帧 {}→{}",
            resets_before,
            resets_after
        );

        app.shutdown();
        pump_main_loop(250);
    });
}
