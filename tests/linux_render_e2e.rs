//! 渲染 e2e（普通 GTK Window，无 AppWindow）：首屏走 replica 尾帧、CUP 风暴只提交末帧。
//!
//! LINUX-PLAN §5.4 S3 / S4。本机 xvfb/Mesa 在第二个 VTE 窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window，两个场景顺序执行（函数名与计划一致）。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;
use vte4::prelude::*;

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

/// S3→F5：首屏用 VTE 自身 scrollback 尾部，不重放 200 行历史。
fn first_paint_uses_replica_tail_not_full_replay(view: &PaneView) {
    let mut bytes = Vec::new();
    for i in 0..200 {
        bytes.extend_from_slice(format!("line-{i}\r\n").as_bytes());
    }
    view.feed_output(&bytes);
    view.flush_pending_feed();
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
    assert_eq!(trace.resets, 0, "Surface 首屏不得 reset（F2）");
    assert_eq!(trace.feeds, 1, "首屏应只 feed 一次");
}

/// C8.2→F2：几何首屏——底行 PROMPT 保留在底行，第一行不含（raw feed）。
fn first_paint_keeps_prompt_on_last_row(view: &PaneView) {
    // 合成 24 行：中间全空行（EL 清行，避免 79 列软换行把行拼成一条），
    // 只在最后一行写 PROMPT_BOTTOM。
    let mut bytes = Vec::new();
    for i in 0..23 {
        bytes.extend_from_slice(format!("\x1b[{};1H\x1b[2K", i + 1).as_bytes());
    }
    bytes.extend_from_slice(b"\x1b[24;1HPROMPT_BOTTOM");
    view.feed_output(&bytes);
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    let lines: Vec<&str> = text.lines().collect();
    // 24 行网格：PROMPT 必须留在第 24 行（几何位置），首行不含。
    // 前序场景已滚出 200 行历史，VTE 视口在底部；取最后 24 行断言。
    let tail: Vec<&str> = lines.iter().rev().take(24).rev().copied().collect();
    assert!(
        tail.iter().any(|l| l.contains("PROMPT_BOTTOM")),
        "第 24 行应含 PROMPT: {text:?}"
    );
    assert!(
        !lines
            .first()
            .map(|l| l.contains("PROMPT_BOTTOM"))
            .unwrap_or(true),
        "第一行不应含 PROMPT: {text:?}"
    );
}

/// S11：OSC 8 包着的 URL，Recording opener 收到一次（不真开浏览器）。
fn url_click_records_https_uri(view: &PaneView) {
    use muxterm::core::url_detect::RecordingOpener;
    use std::rc::Rc;

    let opener = Rc::new(RecordingOpener::new());
    view.set_url_opener(opener.clone());
    view.feed_output(b"\x1b[H\x1b[2J\x1b]8;;https://example.invalid/x\x1b\\hello");
    view.flush_pending_feed();
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

/// C8.3→F5：滚动读 VTE 自身 scrollback（不 dump replica）。
fn scroll_up_reveals_vte_scrollback(view: &PaneView) {
    use gtk4::prelude::ScrollableExt;

    // 首屏：底行是 line-199，没有 line-0（VTE 自身 scrollback 尾部）。
    let mut bytes = Vec::new();
    for i in 0..200 {
        bytes.extend_from_slice(format!("line-{i}\r\n").as_bytes());
    }
    view.feed_output(&bytes);
    view.flush_pending_feed();
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-199"), "首屏应含 line-199: {text}");
    assert!(!text.contains("line-0"), "首屏不应含 line-0: {text}");

    // 向上滚到 scrollback 顶部：出现 line-0，line-199 消失。
    let adj = view.terminal().vadjustment().expect("VTE 应有 vadjustment");
    adj.set_value(0.0);
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-0"), "滚到顶部应出现 line-0: {text}");
    assert!(
        !text.contains("line-199"),
        "滚动后不应再显示 line-199: {text}"
    );

    // 滚回底部：恢复 line-199。
    adj.set_value(adj.upper() - adj.page_size());
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-199"), "滚回后应含 line-199: {text}");
}

/// E2→F2：合成 Codex 风格 TUI fixture——raw feed 后 VTE 同时有 HEADER/BODY/PROMPT
/// （或 FOOTER），盒线 `─` 保留，第一行不是 PROMPT（几何位置不能挤碎）。
fn codex_tui_fixture_keeps_header_and_prompt(view: &PaneView) {
    let raw = include_str!("samples/codex-tui-sanitized.txt");
    let payload = raw
        .split_once("PAYLOAD_UTF8_BELOW\n")
        .map(|(_, p)| p)
        .expect("fixture 应含 PAYLOAD_UTF8_BELOW 标记");
    view.feed_output(payload.as_bytes());
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    assert!(
        text.contains("TOKEN_HEADER"),
        "VTE 应含 TOKEN_HEADER: {text:?}"
    );
    assert!(text.contains("TOKEN_BODY"), "VTE 应含 TOKEN_BODY: {text:?}");
    assert!(
        text.contains("TOKEN_PROMPT") || text.contains("TOKEN_FOOTER"),
        "VTE 应含 TOKEN_PROMPT 或 TOKEN_FOOTER: {text:?}"
    );
    assert!(text.contains('─'), "VTE 应保留 U+2500 盒线: {text:?}");
    let first_row = text.lines().next().unwrap_or("");
    assert!(
        !first_row.contains("TOKEN_PROMPT"),
        "第一行不应是 PROMPT: {first_row:?}"
    );
}

/// E3→F2：seeded 后两段 CUP 半帧都按序 raw feed——VTE 仍同时有 HEADER 和 PROMPT
/// （1365/2730 是同一帧前后半，不是二选一；禁止 replica dump）。
fn cup_half_frames_keep_header_and_prompt(view: &PaneView) {
    // 先 raw feed 完整 fixture，再喂两段残缺 CUP 半帧（都进 VTE 合并缓冲）。
    let raw = include_str!("samples/codex-tui-sanitized.txt");
    let payload = raw
        .split_once("PAYLOAD_UTF8_BELOW\n")
        .map(|(_, p)| p)
        .expect("fixture 应含 PAYLOAD_UTF8_BELOW 标记");
    view.feed_output(payload.as_bytes());
    view.flush_pending_feed();
    pump_main_loop(40);

    // 同一帧被 tmux 切成两段：前半含清屏+头栏，后半继续画底栏（无第二次清屏）。
    let mut half1 = Vec::new();
    half1.extend_from_slice(b"\x1b[H\x1b[2J");
    half1.extend_from_slice(b"\x1b[1;1H\x1b[1m TOKEN_HEADER  example-project\x1b[0m\x1b[K");
    let mut half2 = Vec::new();
    half2.extend_from_slice(b"\x1b[22;1H");
    half2.extend_from_slice(
        b"\x1b[48;2;216;216;216m\x1b[30m TOKEN_PROMPT  example composer\x1b[0m\x1b[K",
    );
    view.feed_output(&half1);
    view.feed_output(&half2);
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    assert!(
        text.contains("TOKEN_HEADER"),
        "半帧按序 feed 后 VTE 应含 TOKEN_HEADER: {text:?}"
    );
    assert!(
        text.contains("TOKEN_PROMPT"),
        "半帧按序 feed 后 VTE 应含 TOKEN_PROMPT: {text:?}"
    );
}

/// E6：小 VTE 的键直接走 send_input 回调（不做输入框）。
fn mini_vte_input_routes_to_send_input(view: &PaneView) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let got = Rc::new(RefCell::new(Vec::<(u32, Vec<u8>)>::new()));
    let g = got.clone();
    view.connect_input(move |pid, data| g.borrow_mut().push((pid, data.to_vec())));
    view.test_emit_input(b"ls\r");
    assert_eq!(
        *got.borrow(),
        vec![(1, b"ls\r".to_vec())],
        "小 VTE 输入应原样路由到 send_input"
    );
}

/// F1：Surface 打字契约——`\r` + 更长前缀原地覆盖，完整句恰好一次。
fn surface_typing_overwrites_in_place(view: &PaneView) {
    view.feed_output(b"\x1b[H\x1b[2J");
    view.flush_pending_feed();
    view.clear_render_trace();
    view.feed_output(b"hello");
    view.flush_pending_feed();
    view.feed_output(b"\rhello world");
    view.flush_pending_feed();
    view.feed_output(b"\rhello world again");
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    assert_eq!(
        text.matches("hello world again").count(),
        1,
        "完整句应恰好一次（2105 越写越长）: {text:?}"
    );
    assert!(
        !text.contains("hello world\nhello world again"),
        "不应残留旧前缀: {text:?}"
    );
}

/// F1：Surface 无 reset 契约——seed 后 20 帧 CUP 只 feed 原始字节，resets 不涨。
fn surface_live_feed_does_not_reset(view: &PaneView) {
    view.feed_output(b"\x1b[H\x1b[2Jseed");
    view.flush_pending_feed();
    view.clear_render_trace();
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
    assert_eq!(
        trace.resets, 0,
        "seed 后 CUP 风暴不得 reset（白屏）: {trace:?}"
    );
}

/// F1：Codex fixture 直接 raw feed——头+底+盒线，不经 replica dump。
fn surface_codex_fixture_raw_feed(view: &PaneView) {
    let raw = include_str!("samples/codex-tui-sanitized.txt");
    let payload = raw
        .split_once("PAYLOAD_UTF8_BELOW\n")
        .map(|(_, p)| p)
        .expect("fixture 应含 PAYLOAD_UTF8_BELOW 标记");
    view.feed_output(payload.as_bytes());
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    assert!(
        text.contains("TOKEN_HEADER"),
        "raw feed 应含 HEADER: {text:?}"
    );
    assert!(
        text.contains("TOKEN_PROMPT") || text.contains("TOKEN_FOOTER"),
        "raw feed 应含 PROMPT/FOOTER: {text:?}"
    );
    assert!(text.contains('─'), "raw feed 应含盒线: {text:?}");
}

/// S4→F2：20 个全屏帧一次合并，raw feed 演到末帧（不 reset、不丢中间帧）。
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
    assert_eq!(trace.resets, 0, "CUP 风暴不得 reset（白屏）");
    assert_eq!(trace.feeds, 1, "CUP 风暴应只 feed 一次");
}

#[test]
fn render_e2e_s3_s4() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let view = PaneView::new(1, &theme(), &FontSettings::default(), true, 10_000);
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
        scroll_up_reveals_vte_scrollback(&view);
        view.ensure_grid_size(80, 24);
        pump_main_loop(80);
        codex_tui_fixture_keeps_header_and_prompt(&view);
        view.clear_render_trace();
        cup_half_frames_keep_header_and_prompt(&view);
        mini_vte_input_routes_to_send_input(&view);
        surface_typing_overwrites_in_place(&view);
        surface_live_feed_does_not_reset(&view);
        surface_codex_fixture_raw_feed(&view);

        win.close();
        win.destroy();
        pump_main_loop(40);
    });
}
