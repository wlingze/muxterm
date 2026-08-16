//! W16b：隔离 tmux server 被杀后，窗口留下最后一帧 + 断线水印，不弹模态框。
//!
//! 本 crate 只构造一个 AppWindow。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use gtk4::Widget;
use support::linux_gtk::*;
use support::tmux_test_support::{
    kill_server, list_pane_ids, send_keys_literal, tmux_available, unique_socket,
    wait_capture_contains,
};

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

const TIMEOUT: Duration = Duration::from_secs(10);

struct OnePaneCat {
    socket: String,
    session: String,
    token: String,
}

impl Drop for OnePaneCat {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

fn build_one_pane(label: &str) -> OnePaneCat {
    let socket = unique_socket(label);
    let session = format!("disc-{label}");
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let token = format!("DISC_TOKEN_{suffix}");

    let output = std::process::Command::new("tmux")
        .args([
            "-L",
            &socket,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            "80",
            "-y",
            "24",
            "--",
            "/bin/cat",
        ])
        .output()
        .expect("new-session");
    assert!(
        output.status.success(),
        "new-session 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ids = list_pane_ids(&socket, &session);
    assert_eq!(ids.len(), 1, "应有 1 pane: {ids:?}");
    let target = format!("%{}", ids[0]);
    send_keys_literal(&socket, &target, &format!("{token}\n"));
    wait_capture_contains(&socket, &target, &token, Duration::from_secs(3));
    OnePaneCat {
        socket,
        session,
        token,
    }
}

fn count_type_name(root: &impl IsA<Widget>, type_name: &str) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.type_().name() == type_name);
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_type_name(&c, type_name);
        child = c.next_sibling();
    }
    n
}

fn wait_vte_contains(app: &AppWindow, needle: &str) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        app.test_flush_feeds();
        if app.test_active_pane_vte_text().contains(needle) {
            return true;
        }
    }
    false
}

/// 杀掉隔离 tmux server 之后：窗口还在、画面还在、水印可见、没有对话框。
#[test]
fn linux_disconnect_keeps_vte_and_shows_watermark() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let fx = build_one_pane("gtk-disc");

        let mut cfg = Config::default();
        cfg.tmux.socket = fx.socket.clone();
        cfg.tmux.default_session = fx.session.clone();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);

        assert!(
            wait_vte_contains(&app, &fx.token),
            "断开前 VTE 必须已有 {}",
            fx.token
        );

        kill_server(&fx.socket);

        let deadline = Instant::now() + TIMEOUT;
        let mut overlay = None;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            overlay = find_by_name(&app.test_window(), "muxterm-disconnect-overlay");
            if overlay.as_ref().is_some_and(|w| w.is_visible()) {
                break;
            }
        }

        assert!(
            app.window.is_visible(),
            "隔离 tmux server 被杀后主窗口必须留下，不能把最后一帧一起关掉"
        );
        app.test_flush_feeds();
        assert!(
            app.test_active_pane_vte_text().contains(&fx.token),
            "断线后 VTE 必须保留最后一帧 {}，禁止 reset 清空",
            fx.token
        );
        let overlay = overlay.expect("必须出现 muxterm-disconnect-overlay 水印");
        assert!(overlay.is_visible(), "muxterm-disconnect-overlay 必须可见");
        let dialogs = count_type_name(&app.test_window(), "GtkMessageDialog")
            + count_type_name(&app.test_window(), "GtkAlertDialog");
        assert_eq!(dialogs, 0, "断线不得弹 GtkMessageDialog / GtkAlertDialog");
    });
}
