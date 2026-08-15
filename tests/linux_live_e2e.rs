//! 真隔离 tmux live e2e（本文件唯一 AppWindow）。
//!
//! LINUX-PLAN §5.4 S8 / S9 / S13b：真实 attach + echo 到 replica/VTE、
//! CUP 脚本停在末帧、点 status tab 真的切 window。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use support::linux_gtk::*;
use support::tmux_test_support::*;

use muxterm::core::config::{Config, Theme};
use muxterm::platform::linux::window::AppWindow;

fn theme() -> Theme {
    Theme::load("light").unwrap_or_else(|_| Theme {
        name: "test".into(),
        background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
        foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
        cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
        colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
    })
}

/// S8：echo 出现在 replica 与 VTE（真实 attach，不靠 test_feed_replica）。
fn isolated_tmux_echo_reaches_replica_and_vte(app: &AppWindow, socket: &str) {
    send_keys(socket, "s", "echo MUXTERM_LIVE_TOKEN");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let capture = capture_pane(socket, "s");
        let replica = app.test_replica_last_n(0, 5).join("\n");
        let vte = app.test_active_pane_vte_text();
        if capture.contains("MUXTERM_LIVE_TOKEN")
            && replica.contains("MUXTERM_LIVE_TOKEN")
            && vte.contains("MUXTERM_LIVE_TOKEN")
        {
            ok = true;
            break;
        }
    }
    assert!(ok, "5s 内 echo 应到达 capture/replica/VTE");
}

/// S9：CUP 脚本后 VTE 停在末帧。
fn isolated_tmux_cup_script_lands_on_last_frame(app: &AppWindow, socket: &str) {
    send_keys(
        socket,
        "s",
        "python3 -c 'import sys; [sys.stdout.write(\"\\x1b[H\\x1b[2Jframe-%d\\n\"%i) or sys.stdout.flush() for i in range(20)]'",
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let vte = app.test_active_pane_vte_text();
        if vte.contains("frame-19") {
            ok = true;
            assert!(!vte.contains("frame-0"), "VTE 不应残留 frame-0: {vte}");
            break;
        }
    }
    assert!(ok, "5s 内 VTE 应停在 frame-19");
}

/// C8.5：attach 后 VTE 非空，底行 PROMPT 不塌缩。
fn live_attach_vte_nonempty_and_prompt_not_collapsed(app: &AppWindow, socket: &str) {
    // 光标放到底行再写 PROMPT_BOTTOM。
    // 注意：Rust 里要写 \\033（反斜杠+033），让 tmux send-keys 解释成 ESC。
    send_keys(socket, "s", "printf '\\033[24;1HPROMPT_BOTTOM'");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let vte = app.test_active_pane_vte_text();
        let replica = app.test_replica_last_n(0, 5).join("\n");
        if vte.is_empty() || replica.is_empty() {
            continue;
        }
        let lines: Vec<&str> = vte.lines().collect();
        // 底行区域（最后 3 个非空行）含 PROMPT_BOTTOM，且不在第一行。
        let nonempty: Vec<&str> = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .copied()
            .collect();
        let bottom = nonempty
            .iter()
            .rev()
            .take(3)
            .any(|l| l.contains("PROMPT_BOTTOM"));
        if bottom {
            ok = true;
            assert!(
                !lines
                    .first()
                    .map(|l| l.contains("PROMPT_BOTTOM"))
                    .unwrap_or(true),
                "第一行不应含 PROMPT_BOTTOM: {vte:?}"
            );
            break;
        }
    }
    assert!(ok, "5s 内 VTE 应非空且底行含 PROMPT_BOTTOM");
}

/// S13b：点 status tab 真的切 window（tmux 侧确认）。
fn click_status_tab_switches_real_window(app: &AppWindow, socket: &str) {
    // 先建第二个 window。
    let _ = std::process::Command::new("tmux")
        .args(["-L", socket, "new-window", "-d", "-t", "s", "-n", "other"])
        .status();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tabs = Vec::new();
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        tabs = app.test_tab_ids();
        if tabs.len() >= 2 {
            break;
        }
    }
    assert!(tabs.len() >= 2, "应有 2 个 tab: {tabs:?}");

    // 找非当前 tab 的按钮并点击。
    let current = app.test_active_tab_id();
    let target = tabs
        .iter()
        .copied()
        .find(|t| *t != current)
        .expect("应有非当前 tab");
    let btn = find_by_name(&app.test_window(), &format!("muxterm-status-tab-{target}"))
        .expect("status tab 按钮应存在")
        .downcast::<gtk4::Button>()
        .expect("Button 类型");
    let _: () = btn.emit_by_name("clicked", &[]);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let out = std::process::Command::new("tmux")
            .args(["-L", socket, "display-message", "-p", "#{window_id}"])
            .output()
            .expect("display-message 失败");
        let wid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if wid == format!("@{}", target) && app.test_active_tab_id() == target {
            ok = true;
            break;
        }
    }
    assert!(ok, "点击后 tmux window 应切到 @{target}");
}

/// F1/F4：隔离 tmux 逐字打字（走 AppWindow 输入 → send-keys -H）——
/// VTE 里完整 token 恰好一份（2105「越写越长」）。
fn isolated_tmux_typing_token_appears_once(app: &AppWindow, _socket: &str) {
    // 逐字符经生产输入路径（muxterm_send_input → WriteRaw → send-keys -H）。
    for ch in "MUXTERM_TYPE_TOKEN".chars() {
        app.test_send_input(&[ch as u8]);
        pump_main_loop(20);
    }
    // 等 tmux 把字符回显到 pane 再轮询 VTE。
    std::thread::sleep(Duration::from_millis(300));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        let vte = app.test_active_pane_vte_text();
        let count = vte.matches("MUXTERM_TYPE_TOKEN").count();
        if count >= 1 {
            ok = true;
            assert_eq!(
                count, 1,
                "VTE 里完整 token 应恰好一次（2105 越写越长）: {vte:?}"
            );
            break;
        }
    }
    assert!(ok, "5s 内 VTE 应出现 MUXTERM_TYPE_TOKEN");
}

/// F1：切 tab 不 reset 刷屏——切换前后 resets 增量 ≤ 1。
fn isolated_tmux_switch_tab_resets_bounded(app: &AppWindow, _socket: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tabs = Vec::new();
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        tabs = app.test_tab_ids();
        if tabs.len() >= 2 {
            break;
        }
    }
    assert!(tabs.len() >= 2, "应有 2 个 tab: {tabs:?}");

    let current = app.test_active_tab_id();
    let target = tabs
        .iter()
        .copied()
        .find(|t| *t != current)
        .expect("应有非当前 tab");
    app.test_clear_active_pane_render_trace();
    let before = app.test_active_pane_resets();
    let btn = find_by_name(&app.test_window(), &format!("muxterm-status-tab-{target}"))
        .expect("status tab 按钮应存在")
        .downcast::<gtk4::Button>()
        .expect("Button 类型");
    let _: () = btn.emit_by_name("clicked", &[]);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if app.test_active_tab_id() == target {
            ok = true;
            break;
        }
    }
    assert!(ok, "点击后应切到 @{target}");
    let after = app.test_active_pane_resets();
    assert!(
        after.saturating_sub(before) <= 1,
        "切 tab 不应 reset 刷屏（增量 {before}→{after}）"
    );
}

#[test]
fn live_e2e_s8_s9_s13b() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let socket = unique_socket("live");
        create_session(&socket, "s", 80, 24);

        let mut cfg = Config::default();
        cfg.tmux.socket = socket.clone();
        cfg.tmux.default_session = "s".into();
        let app = AppWindow::new(cfg, theme());
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);

        isolated_tmux_echo_reaches_replica_and_vte(&app, &socket);
        isolated_tmux_cup_script_lands_on_last_frame(&app, &socket);
        live_attach_vte_nonempty_and_prompt_not_collapsed(&app, &socket);
        click_status_tab_switches_real_window(&app, &socket);
        isolated_tmux_typing_token_appears_once(&app, &socket);
        isolated_tmux_switch_tab_resets_bounded(&app, &socket);

        app.shutdown();
        pump_main_loop(250);
        kill_server(&socket);
    });
}
