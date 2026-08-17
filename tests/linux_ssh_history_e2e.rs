//! W18：SSH attach 必须恢复离屏历史，回底按钮与本地 `linux_attach_history_e2e` 同断言。
//!
//! 本 crate 只构造一个 AppWindow。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::ssh_tmux_contract::{build_remote_offscreen_history, ssh_tmux_available, SSH_TIMEOUT};
use support::tmux_test_support::tmux_available;

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

/// 滚出可见区的 token：搜索能命中，滚到顶 VTE 能看见，点回底回到尾标。
#[test]
fn linux_ssh_attach_restores_offscreen_history_and_jump_latest() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    if !ssh_tmux_available() {
        eprintln!("skip: 无 sshd 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let (fx, tail_mark) = build_remote_offscreen_history("gtk-ssh-hist");
        fx.apply_ssh_config_env();

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_spec(WorkspaceSpec::ssh_tmux(
            fx.sshd.alias.clone(),
            Some(fx.session.clone()),
            Some(fx.socket.clone()),
        ));

        let deadline = Instant::now() + SSH_TIMEOUT;
        let mut ready = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if !app.test_layout_leaf_ids().is_empty() {
                ready = true;
                break;
            }
        }
        assert!(ready, "SSH attach 后应有 pane");

        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        let mut tailed = false;
        let deadline = Instant::now() + SSH_TIMEOUT;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(&tail_mark) {
                tailed = true;
                break;
            }
        }
        assert!(tailed, "可见尾标 {tail_mark} 必须在 VTE 里");

        let mut hits = Vec::new();
        let deadline = Instant::now() + SSH_TIMEOUT;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            hits = app.test_search_all(&fx.token);
            if !hits.is_empty() {
                break;
            }
        }
        assert!(
            !hits.is_empty(),
            "SSH attach 后 search_all 必须找到离屏 {}（只抓可见屏不够）",
            fx.token
        );

        app.test_scroll_pane_to_top(pane);
        pump_main_loop(80);
        app.test_flush_feeds();
        assert!(
            app.test_pane_vte_text(pane).contains(&fx.token),
            "滚到顶之后 VTE 必须能看见离屏历史 {}",
            fx.token
        );

        let jump = find_by_name(&app.test_window(), "muxterm-jump-latest")
            .expect("向上滚动后必须出现 muxterm-jump-latest");
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
            "点回底之后可见区应回到尾部。got={after:?}"
        );
        assert!(
            after.contains(&tail_mark),
            "点回底之后可见区应含尾标 {tail_mark}"
        );
    });
}
