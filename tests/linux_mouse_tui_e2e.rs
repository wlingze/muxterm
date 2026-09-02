//! Mouse-reporting TUI：1003/1006 滚轮穿透、OSC 52 复制、bracketed paste。
//!
//! 夹具 `tests/scripts/mouse_tui.py` 模拟 Grok/htop/vim 这一类软件。
//! tmux 只用隔离 `-L muxterm-test-*`；Herdr 只用 named `muxterm-test-*`。

#![cfg(feature = "gtk")]

mod support;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gtk4::prelude::*;
use gtk4::Widget;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::*;
use support::tmux_test_support::{kill_server, list_pane_ids, tmux_available, unique_socket};

use muxterm::core::config::Config;
use muxterm::core::protocol::terminal::mirror::encode_clipboard_paste;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::pane_view::PaneView;
use muxterm::platform::linux::quickconnect::font::FontSettings;
use muxterm::platform::linux::window::AppWindow;

fn mouse_tui_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/mouse_tui.py")
}

fn wait_ready(app: &AppWindow) -> bool {
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    while Instant::now() < deadline {
        app.test_poll_once();
        pump_main_loop(30);
        if !app.test_layout_leaf_ids().is_empty() {
            return true;
        }
    }
    false
}

/// PaneView：开 1003+1006 后滚轮必须是 SGR，不能滚本地历史。
#[test]
fn pane_view_mouse_reporting_sends_sgr_wheel() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let view = PaneView::new(1, &load_theme(), &FontSettings::default(), true, 10_000);
        let win = gtk4::Window::builder()
            .title("mouse-tui-wheel")
            .default_width(640)
            .default_height(400)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);

        let got = Rc::new(RefCell::new(Vec::<u8>::new()));
        let got_cb = got.clone();
        view.connect_input(move |_, data| got_cb.borrow_mut().extend_from_slice(data));
        view.feed_output(b"\x1b[?1003h\x1b[?1006hMOUSE_TUI_READY\r\n");
        view.flush_deferred_feed();
        pump_main_loop(80);
        assert!(view.test_mouse_reporting(), "1003h 必须留在 reply_state");

        view.test_emit_scroll(-1.0);
        pump_main_loop(40);
        let bytes = got.borrow().clone();
        assert!(
            bytes.windows(b"\x1b[<64;".len()).any(|w| w == b"\x1b[<64;"),
            "mouse TUI 滚轮必须 SGR 64: {bytes:?}"
        );

        win.set_child(None::<&Widget>);
        win.destroy();
        pump_main_loop(50);
    });
}

/// OSC 52 复制：VTE GTK4 不实现，必须由 reply_state 写入剪贴板。
#[test]
fn pane_view_osc52_copy_reaches_clipboard() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let view = PaneView::new(1, &load_theme(), &FontSettings::default(), true, 10_000);
        let win = gtk4::Window::builder()
            .title("mouse-tui-osc52")
            .default_width(480)
            .default_height(240)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);

        view.feed_output(b"\x1b]52;c;TVVYVEVSTV9PU0M1Mg==\x07");
        view.flush_deferred_feed();
        pump_main_loop(80);

        let clipboard = view.widget().clipboard();
        let got = Rc::new(RefCell::new(None::<String>));
        let got_cb = got.clone();
        clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
            *got_cb.borrow_mut() = result.ok().flatten().map(|text| text.to_string());
        });
        pump_main_loop(300);
        let text = got.borrow().clone().unwrap_or_default();
        assert_eq!(
            text, "MUXTERM_OSC52",
            "OSC 52 必须进入 GTK 剪贴板, got {text:?}"
        );

        win.set_child(None::<&Widget>);
        win.destroy();
        pump_main_loop(50);
    });
}

/// 2004h 时粘贴必须带 bracketed 包装，空剪贴板不得发空 200~/201~。
#[test]
fn bracketed_paste_wraps_and_skips_empty() {
    assert_eq!(
        encode_clipboard_paste("hello", true),
        b"\x1b[200~hello\x1b[201~"
    );
    assert!(encode_clipboard_paste("", true).is_empty());
}

#[test]
fn pane_view_tracks_bracketed_paste_mode() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let view = PaneView::new(1, &load_theme(), &FontSettings::default(), true, 10_000);
        let win = gtk4::Window::builder()
            .title("mouse-tui-paste")
            .default_width(480)
            .default_height(240)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);
        view.feed_output(b"\x1b[?2004h");
        view.flush_deferred_feed();
        pump_main_loop(80);
        assert!(view.bracketed_paste(), "2004h 必须打开 bracketed paste");
        win.set_child(None::<&Widget>);
        win.destroy();
        pump_main_loop(50);
    });
}

/// 隔离 tmux 跑 mouse_tui.py：滚轮变成 send-keys -H SGR。
#[test]
fn tmux_mouse_tui_wheel_is_sgr() {
    if skip_no_display() {
        return;
    }
    if !tmux_available() {
        eprintln!("skip: 无 tmux");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let script = mouse_tui_script();
        assert!(script.is_file(), "missing {}", script.display());
        let socket = unique_socket("mouse-tui");
        let session = "mouse-tui";
        let output = std::process::Command::new("tmux")
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                session,
                "-x",
                "80",
                "-y",
                "24",
                "--",
                "python3",
                script.to_str().expect("utf8 path"),
            ])
            .output()
            .expect("new-session");
        assert!(
            output.status.success(),
            "tmux new-session 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ids = list_pane_ids(&socket, session);
        assert_eq!(ids.len(), 1);
        let mut cfg = Config::default();
        cfg.tmux.socket = socket.clone();
        cfg.tmux.default_session = session.to_string();
        let app = AppWindow::new(cfg, load_theme());
        app.window.set_default_size(960, 600);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(200);
        assert!(wait_ready(&app), "attach 后应有 pane");
        let pane = *app.test_layout_leaf_ids().first().expect("pane");

        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        let mut mouse = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(40);
            app.test_flush_feeds();
            if app.test_pane_mouse_reporting(pane)
                || app.test_pane_vte_text(pane).contains("MOUSE_TUI_READY")
            {
                mouse = app.test_pane_mouse_reporting(pane);
                if mouse {
                    break;
                }
            }
        }
        if !mouse {
            app.test_feed_pane_view(pane, b"\x1b[?1003h\x1b[?1006h");
            pump_main_loop(80);
            mouse = app.test_pane_mouse_reporting(pane);
        }
        assert!(mouse, "mouse_tui 必须打开 1003");

        app.test_emit_scroll(pane, -1.0);
        pump_main_loop(80);
        let last = app.test_last_raw_input();
        assert!(
            last.windows(b"\x1b[<64;".len()).any(|w| w == b"\x1b[<64;")
                || last.windows(b"\x1b[<65;".len()).any(|w| w == b"\x1b[<65;"),
            "tmux mouse TUI 滚轮必须 SGR: {last:?}"
        );

        app.shutdown();
        pump_main_loop(80);
        kill_server(&socket);
    });
}

/// Herdr attach 后同样把 1003 滚轮写成 control Input。
#[test]
fn herdr_mouse_reporting_sends_sgr_wheel() {
    if skip_no_display() {
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let herdr = IsolatedHerdr::start("mouse-tui");
        let (ws, _tab, pane_id) = herdr.create_workspace("/tmp", "mux-mouse-tui");
        herdr.paint_until_visible_token(&pane_id, "HERDR_MOUSE_SEED");

        let app = AppWindow::new(Config::default(), load_theme());
        app.window.set_default_size(960, 600);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);
        app.test_open_spec(WorkspaceSpec::herdr(
            herdr.name(),
            ws,
            herdr.socket_path().to_string_lossy().to_string(),
        ));
        assert!(wait_ready(&app), "Herdr attach 后应有 pane");
        let pane = *app.test_layout_leaf_ids().first().expect("pane");
        app.test_feed_pane_view(pane, b"\x1b[?1003h\x1b[?1006h");
        pump_main_loop(80);
        assert!(
            app.test_pane_mouse_reporting(pane),
            "Herdr pane 喂 1003h 后必须 reporting"
        );
        app.test_emit_scroll(pane, -1.0);
        pump_main_loop(80);
        let last = app.test_last_raw_input();
        assert!(
            last.windows(b"\x1b[<64;".len()).any(|w| w == b"\x1b[<64;"),
            "Herdr mouse TUI 滚轮必须经 WriteRaw SGR: {last:?}"
        );
        app.shutdown();
        pump_main_loop(80);
    });
}
