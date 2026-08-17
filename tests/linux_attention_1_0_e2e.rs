//! W17d：前台 OSC 133 D 不通知；后台 Done 看见即熄；静音后 BEL 不再点亮。
//!
//! 本 crate 只构造一个 AppWindow。Done 脚本必须是 `osc133_d_only.py`（无额外 BEL）。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::feature_e2e_contract::*;
use support::linux_gtk::*;
use support::tmux_test_support::{tmux_available, tmux_ok};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if app.test_layout_leaf_ids().len() >= 2 {
            return true;
        }
    }
    false
}

fn wait_done(app: &AppWindow, min: usize) -> bool {
    let deadline = Instant::now() + FEATURE_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if app.test_attention_done_count() >= min {
            return true;
        }
    }
    false
}

fn notify_has_done(app: &AppWindow) -> bool {
    app.test_notifications_recorded().iter().any(|n| {
        let l = n.to_lowercase();
        l.contains("complete") || l.contains("done") || n.contains("完成")
    })
}

/// 愿景 §2.15.1 + B.2：前台跑完不是通知；Done 看见才熄；静音后不再亮。
#[test]
fn linux_done_visible_clears_foreground_silent_mute_holds() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_two_pane_cat("gtk-attn-10");

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        cfg.attention.enabled = true;
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);
        assert!(wait_ready(&app), "attach 后应有 2 pane");

        let pane0 = fx.panes[0];
        let pane1 = fx.panes[1];
        app.test_switch_pane(pane0);
        pump_main_loop(50);
        app.test_poll_once();

        send_command_done_no_bel(&fx.socket, &fx.pane_target(0));
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app
                .test_search_all("CMD_DONE_ONLY")
                .iter()
                .any(|(_, p, _)| *p == pane0)
            {
                break;
            }
        }
        pump_main_loop(80);
        app.test_poll_once();
        assert_eq!(
            app.test_attention_done_count(),
            0,
            "前台 pane 的 OSC 133 D 必须被当成已看见，done 计数为 0。count={}",
            app.test_attention_done_count()
        );
        assert!(
            !notify_has_done(&app),
            "前台跑完不得 notify_done。log={:?}",
            app.test_notifications_recorded()
        );

        app.test_switch_pane(pane0);
        pump_main_loop(40);
        send_command_done_no_bel(&fx.socket, &fx.pane_target(1));
        assert!(
            wait_done(&app, 1),
            "后台 OSC 133 D（无 BEL）必须点亮 done。count={} log={:?}",
            app.test_attention_done_count(),
            app.test_notifications_recorded()
        );
        assert!(
            notify_has_done(&app),
            "后台完成必须 notify_done。log={:?}",
            app.test_notifications_recorded()
        );

        app.test_switch_pane(pane1);
        pump_main_loop(80);
        app.test_poll_once();
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut cleared = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_attention_done_count() == 0 {
                cleared = true;
                break;
            }
        }
        assert!(
            cleared,
            "切到 Done pane 必须熄灭 done。count={}",
            app.test_attention_done_count()
        );

        tmux_ok(
            &fx.socket,
            &["respawn-pane", "-k", "-t", &fx.pane_target(0), "/bin/cat"],
        );
        app.test_switch_pane(pane1);
        pump_main_loop(80);

        send_background_bel(&fx.socket, &fx.pane_target(0));
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut blocked = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if app.test_attention_blocked_workspaces() >= 1 {
                blocked = true;
                break;
            }
        }
        assert!(
            blocked,
            "静音前后台 BEL 必须点亮。count={}",
            app.test_attention_blocked_workspaces()
        );
        let n_before = app.test_notifications_recorded().len();

        app.test_open_panel(1);
        pump_main_loop(80);
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .expect("Attention 列表")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox");
        let row = list
            .row_at_index(0)
            .or_else(|| list.row_at_index(1))
            .expect("应有注意力行");
        list.select_row(Some(&row));
        pump_main_loop(40);
        let mute = find_by_name(&app.test_window(), "muxterm-attention-mute-1h")
            .expect("muxterm-attention-mute-1h")
            .downcast::<gtk4::Button>()
            .expect("Button");
        let _: () = mute.emit_by_name("clicked", &[]);
        pump_main_loop(40);
        app.test_poll_once();
        assert_eq!(
            app.test_attention_blocked_workspaces(),
            0,
            "静音后红点必须为 0"
        );

        send_background_bel(&fx.socket, &fx.pane_target(0));
        pump_main_loop(200);
        app.test_poll_once();
        pump_main_loop(80);
        assert_eq!(
            app.test_attention_blocked_workspaces(),
            0,
            "静音期内再 BEL 不得重亮红点。count={}",
            app.test_attention_blocked_workspaces()
        );
        assert_eq!(
            app.test_notifications_recorded().len(),
            n_before,
            "静音期内不得再追加通知。log={:?}",
            app.test_notifications_recorded()
        );
    });
}
