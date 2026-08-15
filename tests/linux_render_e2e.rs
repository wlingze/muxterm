//! 渲染 e2e（普通 GTK Window，无 AppWindow）：首屏走 replica 尾帧、CUP 风暴只提交末帧。
//!
//! LINUX-PLAN §5.4 S3 / S4。本机 xvfb/Mesa 在第二个 VTE 窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window，两个场景顺序执行（函数名与计划一致）。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::replica::ReplicaStore;
use muxterm::platform::linux::pane_view::PaneView;
use muxterm::platform::linux::quickconnect::font::FontSettings;

fn theme() -> muxterm::core::config::Theme {
    muxterm::core::config::Theme::load("light").unwrap_or_else(|_| muxterm::core::config::Theme {
        name: "test".into(),
        background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
        foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
        cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
        colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
    })
}

/// S3：首屏用 replica 尾帧，不重放 200 行历史。
fn first_paint_uses_replica_tail_not_full_replay(view: &PaneView) {
    let mut store = ReplicaStore::new(10_000);
    for i in 0..200 {
        store.feed("ws", 1, format!("line-{i}\r\n").as_bytes(), 80, 24);
    }
    let ansi = store.visible_ansi("ws", 1);

    view.present_from_replica(&ansi);
    pump_main_loop(80);

    let text = view.visible_text();
    assert!(text.contains("line-199"), "首屏应含最后一行: {text}");
    assert!(!text.contains("line-0"), "首屏不应含最早行: {text}");
    let trace = view.render_trace();
    assert_eq!(trace.resets, 1, "首屏应 reset 一次");
    assert_eq!(trace.feeds, 1, "首屏应只 feed 一次");
    assert!(
        trace.bytes_fed < 80 * 24 * 4,
        "首屏字节应远小于 200 行原始字节: {}",
        trace.bytes_fed
    );
}

/// S11：OSC 8 包着的 URL，Recording opener 收到一次（不真开浏览器）。
fn url_click_records_https_uri(view: &PaneView) {
    use muxterm::core::url_detect::RecordingOpener;
    use std::rc::Rc;

    let opener = Rc::new(RecordingOpener::new());
    view.set_url_opener(opener.clone());
    view.present_bytes(b"\x1b]8;;https://example.invalid/x\x1b\\hello", true);
    pump_main_loop(40);

    // 点击左上角（URL 在首行首列）。
    view.open_url_at(5.0, 5.0);
    pump_main_loop(40);
    let opened = opener.opened.borrow();
    assert_eq!(
        *opened,
        vec!["https://example.invalid/x".to_string()],
        "Recording opener 应收到一次 URI"
    );
}

/// S4：20 个全屏帧一次合并，只提交末帧。
fn cup_storm_feeds_only_last_frame(view: &PaneView) {
    let mut all = Vec::new();
    for i in 0..20 {
        all.extend_from_slice(format!("\x1b[H\x1b[2Jframe-{i}").as_bytes());
    }
    view.feed_output(&all);
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    assert!(text.contains("frame-19"), "应停在末帧: {text}");
    assert!(!text.contains("frame-0"), "不应含首帧: {text}");
    let trace = view.render_trace();
    assert_eq!(trace.resets, 1, "CUP 风暴应 reset 一次");
    assert_eq!(trace.feeds, 1, "CUP 风暴应只 feed 一次");
    assert!(
        trace.bytes_fed < 200,
        "只应喂最后一帧（约 20 字节）: {}",
        trace.bytes_fed
    );
}

#[test]
fn render_e2e_s3_s4() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let view = PaneView::new(1, &theme(), &FontSettings::default(), true);
        let win = gtk4::Window::builder()
            .title("render-e2e")
            .default_width(640)
            .default_height(400)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);

        first_paint_uses_replica_tail_not_full_replay(&view);
        view.clear_render_trace();
        cup_storm_feeds_only_last_frame(&view);
        url_click_records_https_uri(&view);

        win.close();
        win.destroy();
        pump_main_loop(40);
    });
}
