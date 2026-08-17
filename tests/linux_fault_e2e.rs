//! W19e：GTK fault 兜底——注入 fault 后弹 muxterm-fault-dialog，进程继续。
//!
//! 本 crate 只构造一个 AppWindow。不真的把 emulate 炸穿 glib。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::config::Config;
use muxterm::core::fault;
use muxterm::platform::linux::window::AppWindow;

/// 注入 fault 后：对话框存在、last_message 含 token、进程还能继续轮询。
#[test]
fn linux_fault_dialog_shows_and_process_survives() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        fault::clear_last_message();
        muxterm::platform::linux::fault_gtk::reset_dialog_shown_for_test();

        let cfg = Config::default();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_inject_fault("W19_FAULT_TOKEN");
        pump_main_loop(80);

        // fault 对话框是独立顶层 Window，在 app 的 window group 里找。
        let group = app.window.group();
        let dialog = group
            .list_windows()
            .iter()
            .find_map(|w| find_by_name(w, "muxterm-fault-dialog"))
            .expect("注入 fault 后必须有 muxterm-fault-dialog");
        assert!(dialog.is_visible(), "fault 对话框应可见");
        let ok =
            find_by_name(&dialog, "muxterm-fault-dialog-ok").expect("fault 对话框必须有 OK 按钮");
        assert!(ok.is_sensitive(), "OK 按钮应可点");

        let last = fault::last_message().expect("last_message 应记录 fault");
        assert!(
            last.contains("W19_FAULT_TOKEN"),
            "last_message 必须含 token: {last}"
        );

        // 进程继续：注入后还能正常轮询（不 abort）。
        app.test_poll_once();
        pump_main_loop(30);
        assert!(
            app.test_window().is_visible(),
            "fault 后窗口仍在，进程未退出"
        );
    });
}
