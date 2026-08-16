//! W14 Linux Surface：真 tmux attach 后的搜索 / Done 通知 / mock-codex / tail-f。
//!
//! 本 crate 只构造一个 AppWindow。`linux_search_e2e` 的 Mock PaneBuf 不算本契约。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::feature_e2e_contract::*;
use support::linux_gtk::*;
use support::tmux_test_support::{tmux_available, wait_capture_contains};
use vte4::prelude::*;

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_vte_contains(app: &AppWindow, pane: u32, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if app.test_pane_vte_text(pane).contains(needle) {
            return true;
        }
        let all: String = app
            .test_layout_leaf_ids()
            .into_iter()
            .map(|id| app.test_pane_vte_text(id))
            .collect();
        if all.contains(needle) {
            return true;
        }
    }
    false
}

fn wait_search_pool(app: &AppWindow, token: &str) -> Vec<(String, u32, String)> {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
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

/// attach 已有 2 pane 画面：搜索能跳、后台 Done 有通知、mock-codex 末帧可见、tail -f 能跟。
#[test]
fn linux_feature_search_notify_codex_tail() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_two_pane_cat("gtk-feat");

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
        assert!(
            ready,
            "attach 后应有 2 个 pane 控件，leaves={:?}",
            app.test_layout_leaf_ids()
        );

        // --- 搜索：真 PaneBuf，不是 Mock ---
        let hits = wait_search_pool(&app, &fx.search_token);
        assert!(
            !hits.is_empty(),
            "WorkspacePool::search_all 必须找到播种 token {}（linux_search_e2e Mock 路径不算）",
            fx.search_token
        );

        app.test_open_panel(2);
        pump_main_loop(80);
        let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
            .expect("搜索 Entry")
            .downcast::<gtk4::Entry>()
            .expect("Entry");
        entry.set_text(&fx.search_token);
        pump_main_loop(120);
        let hit = find_by_name_prefix(&app.test_window(), "muxterm-search-hit-")
            .expect("Search tab 必须出现 muxterm-search-hit-*（AppWindow 生产路径）");
        assert!(hit.is_visible(), "命中行应可见");
        if let Ok(row) = hit.downcast::<gtk4::ListBoxRow>() {
            let _: () = row.emit_by_name("activate", &[]);
        }
        pump_main_loop(80);
        app.test_poll_once();
        assert!(
            wait_vte_contains(&app, fx.panes[0], &fx.search_token, FEATURE_TIMEOUT),
            "跳转后 VTE 必须含搜索 token {}。vte0={:?} vte1={:?}",
            fx.search_token,
            app.test_pane_vte_text(fx.panes[0]),
            app.test_pane_vte_text(fx.panes[1])
        );

        // --- 真 BEL：通知 + 选中 peek + 快速回复进 tmux（必须在 Done respawn 之前，pane1 还是 /bin/cat）---
        app.test_switch_pane(fx.panes[0]);
        app.test_poll_once();
        pump_main_loop(40);
        send_background_bel(&fx.socket, &fx.pane_target(1));
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut saw_blocked = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_notifications_recorded().iter().any(|n| {
                let l = n.to_lowercase();
                l.contains("attention") || l.contains("blocked") || n.contains("需要")
            }) {
                saw_blocked = true;
                break;
            }
        }
        assert!(
            saw_blocked,
            "真 %output BEL 必须 notify_blocked。实际: {:?}",
            app.test_notifications_recorded()
        );
        app.test_open_panel(1);
        pump_main_loop(80);
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .expect("Attention 列表")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox");
        let row = list
            .row_at_index(1)
            .or_else(|| list.row_at_index(0))
            .expect("应有注意力行");
        list.select_row(Some(&row));
        let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
            .expect("Entry")
            .downcast::<gtk4::Entry>()
            .expect("Entry");
        entry.set_text("x");
        entry.set_text("");
        pump_main_loop(80);
        let peek_sw = find_by_name(&app.test_window(), "muxterm-attention-peek")
            .expect("小 VTE")
            .downcast::<gtk4::ScrolledWindow>()
            .expect("ScrolledWindow");
        let peek_term = peek_sw
            .child()
            .expect("peek 子控件")
            .downcast::<vte4::Terminal>()
            .expect("VTE");
        let peek_text = peek_term
            .text_format(vte4::Format::Text)
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert!(
            peek_text.contains(&fx.bg_token),
            "选中后小 VTE 必须是该 pane 画面（含 {}）。peek={peek_text:?}",
            fx.bg_token
        );
        app.test_peek_emit_input(b"W15_REPLY");
        app.test_poll_once();
        pump_main_loop(80);
        wait_capture_contains(&fx.socket, &fx.pane_target(1), "W15_REPLY", FEATURE_TIMEOUT);

        // --- 后台任务完成通知（前台是 pane0，Done 打在 pane1）---
        app.test_switch_pane(fx.panes[0]);
        app.test_poll_once();
        pump_main_loop(40);
        send_background_task_done(&fx.socket, &fx.pane_target(1));
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut saw_done = false;
        let mut saw_notify = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_attention_done_count() >= 1 {
                saw_done = true;
            }
            if app.test_notifications_recorded().iter().any(|n| {
                let l = n.to_lowercase();
                l.contains("done")
                    || l.contains("complete")
                    || l.contains("finished")
                    || n.contains("完成")
            }) {
                saw_notify = true;
            }
            if saw_done && saw_notify {
                break;
            }
        }
        assert!(
            saw_done,
            "后台 OSC 133 D 必须让 AttentionEngine.done ≥ 1（前台 CommandDone 会清成 Idle，所以必须打非激活 pane）"
        );
        assert!(
            saw_notify,
            "任务完成必须走 NotificationSink::notify_done（或等价），test_notifications_recorded 含 done/complete。实际: {:?}",
            app.test_notifications_recorded()
        );

        // --- mock-codex TUI 末帧 ---
        respawn_mock_codex(&fx.socket, &fx.pane_target(0));
        wait_capture_contains(
            &fx.socket,
            &fx.pane_target(0),
            "MOCK_CODEX_DONE",
            FEATURE_TIMEOUT,
        );
        // 末帧头在 row 1，VTE 视口默认在底部；先滚到顶再断言可见文本。
        app.test_scroll_pane_to_top(fx.panes[0]);
        pump_main_loop(40);
        assert!(
            wait_vte_contains(&app, fx.panes[0], "TOKEN_HEADER", FEATURE_TIMEOUT)
                && wait_vte_contains(&app, fx.panes[0], "TOKEN_PROMPT", FEATURE_TIMEOUT),
            "mock-codex 末帧必须进 VTE（TOKEN_HEADER + TOKEN_PROMPT）。vte={:?}",
            app.test_pane_vte_text(fx.panes[0])
        );

        // --- tail -f 跟随新行 ---
        let log = std::env::temp_dir().join(format!("muxterm-gtk-tail-{}.log", fx.session));
        let _ = std::fs::remove_file(&log);
        append_line(&log, "TAIL_BOOT");
        start_tail_f(&fx.socket, &fx.pane_target(1), &log);
        wait_capture_contains(&fx.socket, &fx.pane_target(1), "TAIL_BOOT", FEATURE_TIMEOUT);
        append_line(&log, "TAIL_FOLLOW_TOKEN");
        wait_capture_contains(
            &fx.socket,
            &fx.pane_target(1),
            "TAIL_FOLLOW_TOKEN",
            FEATURE_TIMEOUT,
        );
        assert!(
            wait_vte_contains(&app, fx.panes[1], "TAIL_FOLLOW_TOKEN", FEATURE_TIMEOUT),
            "tail -f 追加行必须出现在 VTE。vte={:?}",
            app.test_pane_vte_text(fx.panes[1])
        );
        let _ = std::fs::remove_file(&log);

        app.shutdown();
        pump_main_loop(250);
    });
}
