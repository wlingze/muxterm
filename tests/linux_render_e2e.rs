//! 渲染 e2e（普通 GTK Window，无 AppWindow）：首屏走 replica 尾帧、CUP 风暴只提交末帧。
//!
//! LINUX-PLAN §5.4 S3 / S4。本机 xvfb/Mesa 在第二个 VTE 窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window，两个场景顺序执行（函数名与计划一致）。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;
use vte4::prelude::*;

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
    let first_row = text.lines().next().unwrap_or("");
    assert!(
        !first_row.contains("line-199"),
        "第一行不应是 line-199（几何 dump 应保留行位置）: {first_row:?}"
    );
    let trace = view.render_trace();
    assert_eq!(trace.resets, 1, "首屏应 reset 一次");
    assert_eq!(trace.feeds, 1, "首屏应只 feed 一次");
    assert!(
        trace.bytes_fed < 80 * 24 * 4,
        "首屏字节应远小于 200 行原始字节: {}",
        trace.bytes_fed
    );
}

/// C8.2：几何首屏——底行 PROMPT 保留在底行，第一行不含。
fn first_paint_keeps_prompt_on_last_row(view: &PaneView) {
    let mut store = ReplicaStore::new(10_000);
    // 合成 24 行：中间全空格，只在最后一行写 PROMPT。
    let mut bytes = Vec::new();
    for i in 0..23 {
        bytes.extend_from_slice(format!("\x1b[{};1H", i + 1).as_bytes());
        bytes.extend_from_slice(&[b' '; 80]);
    }
    bytes.extend_from_slice(b"\x1b[24;1HPROMPT");
    store.feed("ws", 1, &bytes, 80, 24);
    let ansi = store.visible_ansi("ws", 1);

    view.present_from_replica(&ansi);
    pump_main_loop(80);

    let text = view.visible_text();
    let lines: Vec<&str> = text.lines().collect();
    // 24 行网格：PROMPT 必须留在第 24 行（几何位置），首行不含。
    assert!(
        lines.get(23).map(|l| l.contains("PROMPT")).unwrap_or(false),
        "第 24 行应含 PROMPT: {text:?}"
    );
    assert!(
        !lines.first().map(|l| l.contains("PROMPT")).unwrap_or(true),
        "第一行不应含 PROMPT: {text:?}"
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

/// C8.3：滚动读 replica 历史，镜像 VTE scrollback 保持 0。
fn scroll_up_reveals_replica_history(view: &PaneView) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let store = Rc::new(RefCell::new(ReplicaStore::new(10_000)));
    for i in 0..200 {
        store
            .borrow_mut()
            .feed("ws", 1, format!("line-{i}\r\n").as_bytes(), 80, 24);
    }
    // 滚动 provider：offset 行前、rows 行的几何 ANSI。
    let provider = {
        let store = store.clone();
        Rc::new(move |offset: u32, rows: u32| store.borrow().scroll_ansi("ws", 1, offset, rows))
    };
    view.set_scroll_provider(provider);

    // 首屏：底行是 line-199，没有 line-0。
    let ansi = store.borrow().visible_ansi("ws", 1);
    view.present_from_replica(&ansi);
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-199"), "首屏应含 line-199: {text}");
    assert!(!text.contains("line-0"), "首屏不应含 line-0: {text}");
    assert_eq!(view.history_offset(), 0, "初始 offset 应为 0");

    // 向上滚 24 行：出现更早的块（line-152..175），line-199 消失。
    view.scroll_history(24);
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(
        text.contains("line-152") || text.contains("line-0"),
        "滚动后应出现更早历史: {text}"
    );
    assert!(
        !text.contains("line-199"),
        "滚动后不应再显示 line-199: {text}"
    );
    assert!(view.history_offset() > 0, "offset 应大于 0");

    // 滚回底部：恢复 line-199，offset 归零。
    view.scroll_history(-1000);
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-199"), "滚回后应含 line-199: {text}");
    assert_eq!(view.history_offset(), 0, "滚回底部 offset 应为 0");
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
            .default_height(640)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);
        // 镜像 80×24：VTE 网格与 replica 一致，几何 dump 的 24 行全部可见。
        view.ensure_grid_size(80, 24);
        pump_main_loop(80);

        eprintln!(
            "rows={} cols={}",
            view.terminal().row_count(),
            view.terminal().column_count()
        );
        first_paint_uses_replica_tail_not_full_replay(&view);
        view.clear_render_trace();
        first_paint_keeps_prompt_on_last_row(&view);
        view.clear_render_trace();
        cup_storm_feeds_only_last_frame(&view);
        url_click_records_https_uri(&view);
        scroll_up_reveals_replica_history(&view);

        win.close();
        win.destroy();
        pump_main_loop(40);
    });
}
