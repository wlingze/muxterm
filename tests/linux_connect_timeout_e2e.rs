//! W15c：SSH 连不上时 GTK 主线程不得 `block_on` 冻死。
//!
//! 本 crate 只构造一个 AppWindow。用 TEST-NET `192.0.2.1`（文档黑洞，应卡住到 ConnectTimeout）。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::config::Config;
use muxterm::platform::linux::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
use muxterm::platform::linux::window::AppWindow;

#[test]
fn unreachable_ssh_does_not_block_gtk_thread() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(800, 600);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        let target = TargetConfig::new(
            "w15-blackhole",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "192.0.2.1".into(),
            },
            "~",
        );
        let t0 = Instant::now();
        app.test_connect_target(target);
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "test_connect_target 必须立刻把控制权还给 GTK（后台等 SSH），实际 {elapsed:?}。禁止 rt.block_on(open_spec) 堵主线程"
        );

        pump_main_loop(80);
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut saw_fail = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(50);
            if app.test_notifications_recorded().iter().any(|n| {
                let l = n.to_lowercase();
                l.contains("fail")
                    || l.contains("timeout")
                    || l.contains("unreachable")
                    || l.contains("refused")
                    || l.contains("timed out")
                    || n.contains("失败")
                    || n.contains("超时")
            }) {
                saw_fail = true;
                break;
            }
        }
        assert!(
            saw_fail,
            "SSH 失败必须进 test_notifications_recorded（不要只 tracing::error）。实际: {:?}",
            app.test_notifications_recorded()
        );

        app.shutdown();
        pump_main_loop(250);
    });
}
