//! 唯一 AppWindow e2e：BEL → 红点/标题/Attention tab（LINUX-PLAN C3.6）。
//!
//! 本 crate 全文件只允许一个 AppWindow 生命周期（二次析构堆损坏）。
//! E1 已证明 tmux 3.7b %output 原样携带 BEL/OSC 133（PASS_THROUGH），
//! 因此同时覆盖注入 BEL（必过）与真实 tmux printf BEL 两条路径。

#![cfg(feature = "gtk")]

mod support;

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

        // 注入路径（必过）：后台 pane 1 文本 + BEL → blocked 工作区 = 1。
        // 前台 pane 0 会被 E6 排除，不能用来制造注意力行。
        let pane = 1;
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
        // 红点已迁到通知按钮（muxterm-status-notify）：n=1 时文本含 1。
        let notify = find_by_name(&app.test_window(), "muxterm-status-notify")
            .expect("通知按钮应存在")
            .downcast::<gtk4::Button>()
            .expect("Button 类型");
        assert!(
            notify
                .label()
                .map(|l| l.to_string())
                .unwrap_or_default()
                .contains('1'),
            "通知按钮应含 1: {:?}",
            notify.label()
        );

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

        // Attention 行直接展示摘要和状态点，不再创建小终端。
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .expect("列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        let row = list.row_at_index(1).expect("注意力行");
        list.select_row(Some(&row));
        let labels = widget_label_texts(&row);
        assert!(
            labels.iter().any(|text| text.contains("hello")),
            "Blocked attention 行应直接包含摘要: {labels:?}"
        );
        let dot =
            find_by_name(&row, "muxterm-attention-status-dot").expect("Blocked attention 状态点");
        assert!(dot.has_css_class("needs-attention"));
        assert!(find_by_name(&app.test_window(), "muxterm-attention-peek").is_none());

        // 前台 pane 0 的 CommandDone（如 ls）→ 已看见，不进 attention 列表。
        app.test_feed_replica(0, b"\x1b]133;D;0\x07");
        app.test_poll_once();
        let ok = wait_until_widget(5000, || app.test_attention_blocked_workspaces() == 1);
        assert!(ok, "后台 BEL 的 blocked 工作区应保持 1");
        // 重开 Attention tab：前台 ls 不应出现在列表里。
        let entry_ctrl = window_key_controller(&entry).expect("Entry 应有 controller");
        simulate_key_press(&entry_ctrl, gdk::Key::Escape, gdk::ModifierType::empty());
        pump_main_loop(40);
        app.test_open_panel(1);
        pump_main_loop(80);
        let list = find_by_name(&app.test_window(), "muxterm-panel-list")
            .expect("列表应存在")
            .downcast::<gtk4::ListBox>()
            .expect("ListBox 类型");
        let labels = widget_label_texts(&list);
        assert!(
            !labels.iter().any(|t| t.contains("ls")),
            "前台 ls 不应出现在 attention 列表: {labels:?}"
        );
        assert!(
            labels.iter().any(|t| t.contains("hello")),
            "后台 BEL 行应仍在 attention 列表: {labels:?}"
        );

        app.shutdown();
        pump_main_loop(250);
    });
}
