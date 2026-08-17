//! W18：滚动条旁的命令刻度：绿=成功，红=失败；点击跳转；悬停显示命令。
//!
//! 本 crate 只构造一个 AppWindow。数据源是 OSC 133，不要改 pane 字节。

#![cfg(feature = "gtk")]

mod support;

use std::path::PathBuf;
use std::time::Instant;

use gtk4::prelude::*;
use support::feature_e2e_contract::{build_two_pane_cat, FEATURE_TIMEOUT};
use support::linux_gtk::*;
use support::tmux_test_support::{tmux_available, tmux_ok};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

fn send_rounds(socket: &str, pane: &str, suffix: &str) {
    let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/osc133_rounds.py");
    assert!(py.is_file(), "缺少 {}", py.display());
    let cmd = format!(
        "env MUXTERM_CMD_SUFFIX={suffix} python3 -u {}",
        py.display()
    );
    tmux_ok(socket, &["respawn-pane", "-k", "-t", pane, &cmd]);
}

/// 成功/失败刻度必须出现；点红色跳到失败命令；tooltip 含命令文本。
#[test]
fn linux_command_marks_jump_and_hover() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_two_pane_cat("gtk-cmd");
        let suffix = fx.search_token.clone();
        let ok = format!("CMD_OK_{suffix}");
        let fail = format!("CMD_FAIL_{suffix}");

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
            if !app.test_layout_leaf_ids().is_empty() {
                ready = true;
                break;
            }
        }
        assert!(ready, "attach 后应有 pane");

        send_rounds(&fx.socket, &fx.pane_target(0), &suffix);
        let deadline = Instant::now() + FEATURE_TIMEOUT;
        let mut indexed = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            if !app.test_search_all(&ok).is_empty() && !app.test_search_all(&fail).is_empty() {
                indexed = true;
                break;
            }
        }
        assert!(indexed, "两个命令文本必须进 PaneBuf：{ok} / {fail}");

        let ok_mark = find_by_name(&app.test_window(), "muxterm-cmd-mark-ok")
            .expect("成功命令必须有 muxterm-cmd-mark-ok（绿色刻度）");
        assert!(ok_mark.is_visible(), "muxterm-cmd-mark-ok 必须可见");
        let fail_mark = find_by_name(&app.test_window(), "muxterm-cmd-mark-fail")
            .expect("失败命令必须有 muxterm-cmd-mark-fail（红色刻度）");
        assert!(fail_mark.is_visible(), "muxterm-cmd-mark-fail 必须可见");
        let tip = fail_mark
            .tooltip_text()
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert!(
            tip.contains(&fail),
            "悬停失败刻度必须显示命令文本 {fail}。tooltip={tip:?}"
        );

        if let Ok(btn) = fail_mark.downcast::<gtk4::Button>() {
            let _: () = btn.emit_by_name("clicked", &[]);
        } else {
            panic!("muxterm-cmd-mark-fail 应可点击");
        }
        pump_main_loop(80);
        app.test_flush_feeds();
        let visible = app.test_pane_vte_text(fx.panes[0]);
        assert!(
            visible.contains(&fail),
            "点击红色刻度必须跳到失败命令 {fail}。visible={visible:?}"
        );
    });
}
