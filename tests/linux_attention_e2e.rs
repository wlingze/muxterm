//! 唯一 AppWindow e2e：BEL → 红点/标题/Attention tab（LINUX-PLAN C3.6）。
//!
//! 本 crate 全文件只允许一个 AppWindow 生命周期（二次析构堆损坏）。
//! E1 已证明 tmux 3.7b %output 原样携带 BEL/OSC 133（PASS_THROUGH），
//! 因此同时覆盖注入 BEL（必过）与真实 tmux printf BEL 两条路径。

#![cfg(feature = "gtk")]

mod support;

use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gtk4::gdk;
use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::config::{Config, Theme};
use muxterm::platform::linux::window::AppWindow;

fn unique_socket(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("muxterm-test-{}-{}-{}", label, std::process::id(), nanos)
}

struct IsolatedTmux {
    socket: String,
}

impl IsolatedTmux {
    fn new(label: &str) -> Option<Self> {
        let socket = unique_socket(label);
        let probe = Command::new("tmux")
            .args(["-L", &socket, "-f", "/dev/null", "list-sessions"])
            .output();
        if probe.is_err() {
            return None;
        }
        Some(IsolatedTmux { socket })
    }

    fn new_session(&self, name: &str) -> bool {
        Command::new("tmux")
            .args([
                "-L",
                &self.socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "24",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn capture_pane(&self, target: &str) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.socket, "capture-pane", "-p", "-t", target])
            .output()
            .expect("capture-pane 失败");
        String::from_utf8_lossy(&out.stdout).to_string()
    }
}

impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
    }
}

#[test]
fn attention_bel_paints_badge_and_panel() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        // 隔离 tmux：pane 跑 /bin/cat，真实 BEL 走 printf。
        let tmux = IsolatedTmux::new("att-e2e");
        let mut cfg = Config::default();
        if let Some(t) = &tmux {
            assert!(t.new_session("att"));
            cfg.tmux.socket = t.socket.clone();
            cfg.tmux.default_session = "att".into();
        }

        let app = AppWindow::new(
            cfg,
            Theme::load("light").unwrap_or_else(|_| Theme {
                name: "test".into(),
                background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
                foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
                cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
                colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
            }),
        );
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(150);

        // 注入路径（必过）：文本 + BEL → blocked 工作区 = 1。
        let pane = 0;
        app.test_feed_replica(pane, b"hello\r\n\x07");
        app.test_poll_once();
        let ok = wait_until_widget(5000, || app.test_attention_blocked_workspaces() == 1);
        assert!(ok, "注入 BEL 后 blocked 工作区应为 1");

        // 标题含 ●，badge 可见且含 1。
        assert!(
            app.test_window_title().contains('●'),
            "标题应含红点: {}",
            app.test_window_title()
        );
        let badge = find_by_name(&app.test_window(), "muxterm-attention-badge")
            .expect("badge 应存在")
            .downcast::<gtk4::Label>()
            .expect("Label 类型");
        assert!(badge.is_visible(), "badge 应可见");
        assert!(badge.text().contains('1'), "badge 应含 1: {}", badge.text());

        // Alt+Q → Workspaces tab。
        let ctrl = window_key_controller(&app.window).expect("窗口应有 EventControllerKey");
        simulate_key_press(&ctrl, gdk::Key::q, gdk::ModifierType::ALT_MASK);
        pump_main_loop(80);
        assert!(app.test_panel_open(), "Alt+Q 应打开面板");
        assert_eq!(app.test_active_panel_tab(), 0, "Alt+Q 应落在 Workspaces");

        // 关闭面板（Esc），再经测试钩子打开 Attention tab。
        let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
            .expect("面板 Entry 应存在")
            .downcast::<gtk4::Entry>()
            .expect("Entry 类型");
        let entry_ctrl = window_key_controller(&entry).expect("Entry 应有 controller");
        simulate_key_press(&entry_ctrl, gdk::Key::Escape, gdk::ModifierType::empty());
        pump_main_loop(40);
        assert!(!app.test_panel_open(), "Esc 后面板应关闭");

        app.test_open_panel(1);
        pump_main_loop(80);
        assert!(app.test_panel_open(), "测试钩子应打开面板");
        assert_eq!(app.test_active_panel_tab(), 1, "应落在 Attention tab");
        let attention_tab = find_by_name(&app.test_window(), "muxterm-panel-tab-attention")
            .expect("Attention tab 按钮应存在")
            .downcast::<gtk4::ToggleButton>()
            .expect("ToggleButton 类型");
        assert!(attention_tab.is_active(), "Attention tab 应激活");

        // 选中注意力行 → peek 非空（副本里有 hello）。
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .expect("列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        let row = list.row_at_index(1).expect("注意力行");
        list.select_row(Some(&row));
        let entry = find_by_name(&app.test_window(), "muxterm-panel-entry")
            .expect("共享 Entry 应存在")
            .downcast::<gtk4::Entry>()
            .expect("Entry 类型");
        entry.set_text("x");
        entry.set_text("");
        pump_main_loop(40);
        let peek = find_by_name(&app.test_window(), "muxterm-peek-view")
            .expect("peek 应存在")
            .downcast::<gtk4::TextView>()
            .expect("TextView 类型");
        let buf = peek.buffer();
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
        assert!(text.contains("hello"), "peek 应含副本文本: {text:?}");

        // 答复 y + Enter → 真实 tmux capture-pane 含 y（E1 PASS_THROUGH 路径）。
        let reply_entry = find_by_name(&app.test_window(), "muxterm-reply-entry")
            .expect("答复输入框应存在")
            .downcast::<gtk4::Entry>()
            .expect("Entry 类型");
        reply_entry.set_text("y");
        let reply_ctrl = window_key_controller(&reply_entry).expect("答复 Entry 应有 controller");
        simulate_key_press(&reply_ctrl, gdk::Key::Return, gdk::ModifierType::empty());
        pump_main_loop(80);
        if let Some(t) = &tmux {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut got = false;
            while Instant::now() < deadline {
                if t.capture_pane("att").contains('y') {
                    got = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(got, "真实 tmux pane 应收到答复 y");
        }

        app.shutdown();
        pump_main_loop(250);
    });
}
