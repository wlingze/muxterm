//! C8：attach 后 Ctrl+= 缩放和回车不得冻 GTK；字号热路径禁止同步写盘。
//!
//! 本 crate 只构造一个 AppWindow。隔离 `-L muxterm-test-*`。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::tmux_test_support::{create_session, kill_server, tmux_available, unique_socket};

use muxterm::test_support::core::config::Config;
use muxterm::test_support::frontend::linux::window::AppWindow;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(8);

struct TmuxGuard {
    socket: String,
}

impl Drop for TmuxGuard {
    fn drop(&mut self) {
        kill_server(&self.socket);
    }
}

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

/// 缩放立刻改内存字号并把控制权还给 GTK；config.toml 不得在热路径写完。
#[test]
fn linux_font_zoom_and_enter_return_without_sync_persist() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let tmp = std::env::temp_dir().join(format!("muxterm-zoom-hot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("muxterm")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        let config_path = tmp.join("muxterm").join("config.toml");
        std::fs::write(&config_path, "[font]\nsize = 12.0\n").unwrap();

        let socket = unique_socket("gtk-zoom");
        create_session(&socket, "mux-zoom", 80, 24);
        let _tmux = TmuxGuard {
            socket: socket.clone(),
        };

        let mut cfg = Config::default();
        cfg.tmux.socket = socket.clone();
        cfg.tmux.default_session = "mux-zoom".into();
        cfg.font.size = 12.0;
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);
        assert!(wait_ready(&app), "隔离 tmux attach 后应有 pane");

        let before = app.test_font_size();
        let t0 = Instant::now();
        app.test_increase_font();
        let zoom_elapsed = t0.elapsed();
        assert!(
            zoom_elapsed < Duration::from_millis(200),
            "test_increase_font 必须立刻返回（禁止同步写盘 + 全 cache set_font），实际 {zoom_elapsed:?}"
        );
        assert!(
            (app.test_font_size() - before).abs() > f32::EPSILON,
            "内存字号必须立刻变大"
        );
        let raw = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            !raw.contains("13.0") && !raw.contains("size = 13"),
            "热路径禁止同步 persist_config，config 仍应是 12.0: {raw}"
        );

        let t1 = Instant::now();
        app.test_send_input(b"\r");
        let enter_elapsed = t1.elapsed();
        assert!(
            enter_elapsed < Duration::from_millis(50),
            "Enter/WriteRaw 必须立刻返回，实际 {enter_elapsed:?}"
        );

        // 配置只写用户覆盖：13.0 恰好等于默认字号，序列化时会省略，
        // 因此契约是「旧的 12.0 覆盖必须消失」，不是「文件里出现 13.0」。
        let persist_deadline = Instant::now() + Duration::from_secs(5);
        let mut persisted = false;
        while Instant::now() < persist_deadline {
            pump_main_loop(50);
            app.test_poll_once();
            let raw = std::fs::read_to_string(&config_path).unwrap();
            if !raw.contains("12.0") && !raw.contains("size = 12") {
                persisted = true;
                break;
            }
        }
        let raw = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            persisted,
            "防抖之后必须更新 config.toml 的字号覆盖（等值默认项会省略）：{raw}"
        );

        let t2 = Instant::now();
        app.test_decrease_font();
        let down_elapsed = t2.elapsed();
        assert!(
            down_elapsed < Duration::from_millis(200),
            "缩小字号同样不得冻 GTK，实际 {down_elapsed:?}"
        );

        app.shutdown();
        pump_main_loop(80);
        let _ = std::fs::remove_dir_all(&tmp);
    });
}
