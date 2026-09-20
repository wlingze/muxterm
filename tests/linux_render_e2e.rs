//! 渲染 e2e（普通 GTK Window，无 AppWindow）：首屏走 replica 尾帧、CUP 风暴只提交末帧。
//!
//! LINUX-PLAN §5.4 S3 / S4。本机 xvfb/Mesa 在第二个 VTE 窗口 present 时崩溃，
//! 因此整个 crate 只用一个 Window，两个场景顺序执行（函数名与计划一致）。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;
use vte4::prelude::*;

use muxterm::test_support::frontend::linux::pane_view::PaneView;
use muxterm::test_support::frontend::linux::quickconnect::font::FontSettings;
use muxterm::test_support::frontend::linux::theme::Theme;

fn theme() -> Theme {
    load_theme()
}

fn primary_mouse_click_and_native_drag(view: &PaneView, win: &gtk4::Window) {
    if std::env::var("MUXTERM_XTEST_SELECTION").as_deref() != Ok("1") {
        return;
    }
    use std::{cell::RefCell, rc::Rc};
    let input = Rc::new(RefCell::new(Vec::new()));
    let captured = input.clone();
    view.connect_input(move |_, bytes| captured.borrow_mut().extend_from_slice(bytes));
    view.set_header_visible(true);
    view.feed_output(
        b"\x1b[?1049l\x1b[0m\x1b[2J\x1b[3J\x1b[HSELECTABLE_GROK_TEXT\x1b[?1003h\x1b[?1006h",
    );
    view.flush_pending_feed();
    pump_main_loop(100);
    let term = view.terminal();
    #[allow(deprecated)]
    let padding = term.style_context().padding();
    let point = term
        .compute_point(
            win,
            &gtk4::graphene::Point::new(
                (term.char_width() * 4 + term.char_width() / 2 + i64::from(padding.left())) as f32,
                (term.char_height() / 2 + i64::from(padding.top())) as f32,
            ),
        )
        .unwrap();
    input.borrow_mut().clear();
    xtest_pointer(point.x() as i64, point.y() as i64, 1);
    xtest_pointer(point.x() as i64, point.y() as i64, -1);
    let reports = input.borrow().clone();
    assert!(
        reports
            .windows(b"\x1b[<0;5;1M".len())
            .any(|b| b == b"\x1b[<0;5;1M"),
        "click must exclude title bar and padding: {reports:?}"
    );
    assert!(reports.ends_with(b"\x1b[<0;5;1m"));
    input.borrow_mut().clear();
    xtest_pointer(point.x() as i64, point.y() as i64, 1);
    xtest_pointer(
        point.x() as i64 + term.char_width() * 10,
        point.y() as i64,
        0,
    );
    xtest_pointer(
        point.x() as i64 + term.char_width() * 10,
        point.y() as i64,
        -1,
    );
    assert!(
        term.has_selection(),
        "Grok primary-screen drag must select text"
    );
    assert!(view.selected_text().is_some_and(|text| !text.is_empty()));
    assert!(
        input.borrow().is_empty(),
        "drag must not also click in the app"
    );
    term.unselect_all();
    view.feed_output(b"\x1b[?1003l\x1b[?1006l");
    view.flush_pending_feed();
    view.set_header_visible(false);
    pump_main_loop(50);
}

fn full_frame_clears_with_default_background(view: &PaneView, win: &gtk4::Window) {
    view.feed_output(b"\x1b[0m\x1b[42mGREEN");
    view.flush_pending_feed();
    pump_main_loop(50);
    view.feed_full(b"\x1b[H\x1b[0mHTOP\x1b[2;1HAFTER_CLEAR");
    view.flush_pending_feed();
    pump_main_loop(50);
    let html = view.terminal().text_format(vte4::Format::Html).unwrap();
    // 新帧未覆盖的空格也不能继承上一帧 htop 选中行的绿色背景。
    assert!(
        !html.contains("background-color:"),
        "full-frame clear leaked old green background: {html}"
    );
    // HTML 不包含行尾空格的底色，必须看真正的空白区域像素。
    gtk4::test_widget_wait_for_draw(win);
    let terminal = view.terminal();
    let paintable = gtk4::WidgetPaintable::new(Some(terminal));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, terminal.width() as f64, terminal.height() as f64);
    let texture = win
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None);
    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);
    let offset = texture.height() as usize / 2 * stride + texture.width() as usize / 2 * 4;
    let pixel = &pixels[offset..offset + 4];
    assert!(
        pixel[1] <= pixel[0].saturating_add(20) || pixel[1] <= pixel[2].saturating_add(20),
        "full-frame blank region retained previous green background: {pixel:?}"
    );
}

fn reverse_video_preserves_application_background(view: &PaneView) {
    view.feed_full(b"\x1b[0;38;2;248;248;180m\x1b[7mREVERSED\x1b[27mNORMAL\x1b[0m");
    view.flush_pending_feed();
    pump_main_loop(50);
    let html = view.terminal().text_format(vte4::Format::Html).unwrap();
    assert!(
        html.contains("background-color:#F8F8B4"),
        "reverse background must use original application color: {html}"
    );
    assert!(
        !html.contains("background-color:#747454"),
        "foreground correction must not paint a background: {html}"
    );
    assert!(
        html.contains("color=\"#747454\""),
        "normal foreground must remain readable: {html}"
    );
}

fn attach_history_preserves_authoritative_cursor_and_partial_live_csi(view: &PaneView) {
    let mut snapshot = b"\x1b[0m".to_vec();
    for _ in 0..100 {
        snapshot.extend_from_slice(b"old history\r\n");
    }
    snapshot.extend_from_slice(b"\x1b[2J\x1b[HHEADER\x1b[10;1HINPUT\x1b[15;1HMODEL_STATUS\x1b[10;");
    let (cols, rows) = view.allocated_grid_size();
    view.seed_raw(&snapshot, cols, rows);
    // 旧后端/异常事件即使晚发历史，也不能在半条 CSI 中插入回填。
    view.prepend_history(b"LATE_HISTORY_MUST_NOT_REPAINT\n");
    view.feed_output(b"3HX");
    view.flush_pending_feed();
    pump_main_loop(80);
    let screen = view.screen_text();
    assert!(
        screen
            .lines()
            .nth(9)
            .unwrap_or_default()
            .starts_with("INXUT"),
        "attach must not inject CUP into unfinished live CSI: {screen:?}"
    );
    assert!(
        !screen.contains("3HX"),
        "CSI parameters leaked into text: {screen:?}"
    );
    assert!(
        screen
            .lines()
            .nth(14)
            .unwrap_or_default()
            .starts_with("MODEL_STATUS"),
        "{screen:?}"
    );
}

fn sparkle_updates_keep_status_row(view: &PaneView) {
    let terminal = view.terminal();
    let rows = terminal.row_count();
    view.feed_output(format!("\x1b[0m\x1b[2J\x1b[3J\x1b[HSELECTABLE_HISTORY\x1b[{};1H\x1b[48;2;31;31;31mCOMPOSER\x1b[0m\x1b[{};1HMODEL_STATUS", rows - 1, rows).as_bytes());
    view.flush_pending_feed();
    pump_main_loop(80);
    // Xvfb 下真实拖选第一行，后续星光只更新倒数第二行。
    // 只在调用方显式提供的隔离 Xvfb 中发送真实指针事件，不能碰桌面。
    let selected = std::env::var("MUXTERM_XTEST_SELECTION").as_deref() == Ok("1");
    if selected {
        let y = terminal.char_height() / 2;
        xtest_pointer(3, y, 1);
        xtest_pointer(terminal.char_width() * 18, y, 0);
        xtest_pointer(terminal.char_width() * 18, y, -1);
        assert!(
            terminal.has_selection(),
            "native drag must establish selection"
        );
    }
    let selection = view.selected_text();
    let before = terminal.cursor_position();
    view.clear_render_trace();
    for column in 10..20 {
        view.feed_output(
            format!(
                "\x1b7\x1b[{};{column}H\x1b[48;2;31;31;31m\x1b[38;2;40;40;40m⠁\x1b8",
                rows - 1
            )
            .as_bytes(),
        );
        view.flush_pending_feed();
        pump_main_loop(10);
    }
    assert_eq!(terminal.cursor_position(), before);
    let text = view.screen_text();
    assert!(
        text.lines()
            .last()
            .unwrap_or_default()
            .contains("MODEL_STATUS"),
        "{text:?}"
    );
    assert!(text.contains("SELECTABLE_HISTORY"), "{text:?}");
    assert_eq!(view.render_trace().resets, 0);
    assert_eq!(view.render_trace().seeds, 0);
    if selected {
        assert!(
            terminal.has_selection(),
            "animation outside selection must not clear it"
        );
        assert_eq!(view.selected_text(), selection);
        terminal.unselect_all();
    }
}

fn xtest_pointer(x: i64, y: i64, button: i32) {
    let result = std::process::Command::new("python3").args(["-c", r#"
import ctypes, sys
x = ctypes.CDLL('libX11.so.6')
t = ctypes.CDLL('libXtst.so.6')
x.XOpenDisplay.restype = ctypes.c_void_p
x.XFlush.argtypes = [ctypes.c_void_p]
x.XCloseDisplay.argtypes = [ctypes.c_void_p]
t.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
t.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
d = x.XOpenDisplay(None)
assert d
t.XTestFakeMotionEvent(d, -1, int(sys.argv[1]), int(sys.argv[2]), 0)
button = int(sys.argv[3])
if button: t.XTestFakeButtonEvent(d, 1, int(button > 0), 0)
x.XFlush(d)
x.XCloseDisplay(d)
"#, &x.to_string(), &y.to_string(), &button.to_string()]).status().unwrap();
    assert!(result.success());
    pump_main_loop(40);
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
    use muxterm::test_support::frontend::url_opener::RecordingOpener;
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

/// Herdr `full=true` 仍是可直接送入唯一 Surface 的完整 ANSI 重绘。
/// 连续 full frame 必须原地替换旧画面，不能 reset VTE 或把旧 token 追加成历史行。
fn surface_consecutive_full_frames_replace_without_reset(view: &PaneView) {
    view.feed_output(b"\x1b[2J\x1b[HGTK_FULL_ONE");
    view.flush_pending_feed();
    view.clear_render_trace();
    view.feed_output(b"\x1b[HGTK_FULL_TWO");
    view.flush_pending_feed();
    pump_main_loop(80);

    let text = view.visible_text();
    assert!(text.contains("GTK_FULL_TWO"), "末帧必须可见: {text:?}");
    assert!(
        !text.contains("GTK_FULL_ONE"),
        "旧 full frame 不得残留: {text:?}"
    );
    let trace = view.render_trace();
    assert_eq!(trace.resets, 0, "full frame 不得 reset 唯一 Surface");
    assert_eq!(trace.feeds, 1, "第二个 full frame 应作为一次原始 feed");
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

/// W21b：主屏滚轮走生产 test_emit_scroll（禁止测试里 adj.set_value）。
fn wheel_scrolls_vte_history(view: &PaneView) {
    // 清掉前面场景累积的 scrollback，保证 200 行是唯一历史。
    view.terminal().reset(true, true);
    pump_main_loop(40);
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

    // 生产滚轮路径：一格 3 行，200 行历史需要多次滚轮事件。
    for _ in 0..100 {
        view.test_emit_scroll(-1.0);
    }
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-0"), "滚轮向上应出现 line-0: {text}");
    assert!(
        !text.contains("line-199"),
        "滚轮向上后不应再显示 line-199: {text}"
    );

    // 滚回底部。
    for _ in 0..100 {
        view.test_emit_scroll(1.0);
    }
    pump_main_loop(80);
    let text = view.visible_text();
    assert!(text.contains("line-199"), "滚轮向下应恢复 line-199: {text}");
}

/// W21c：alt-screen 滚轮走 input_cb 发 CSI A，禁止 vte.reset。
fn wheel_alt_screen_sends_csi_arrows(view: &PaneView) {
    use std::cell::RefCell;
    use std::rc::Rc;
    let received = Rc::new(RefCell::new(Vec::<u8>::new()));
    let r = received.clone();
    view.connect_input(move |_pid, data| {
        r.borrow_mut().extend_from_slice(data);
    });
    view.feed_output(b"\x1b[?1049h");
    view.flush_pending_feed();
    pump_main_loop(40);
    view.test_emit_scroll(-1.0);
    pump_main_loop(40);
    let got = received.borrow().clone();
    assert!(
        got.starts_with(b"\x1b[A"),
        "alt-screen 滚轮必须发 CSI A: {got:?}"
    );
    let trace = view.render_trace();
    assert_eq!(trace.resets, 0, "滚轮不得 reset VTE");
}

#[test]
fn render_e2e_s3_s4() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        muxterm::test_support::frontend::linux::font_registry::register_bundled_fonts().unwrap();

        // 本机 Monospace 12pt 下 VTE 实际网格只有 79x21，装不下 80x24 fixture
        // （头部会被滚出可见区）。用 10pt 让网格 >= 80x24，断言不变。
        let view = PaneView::new(
            1,
            &theme(),
            &FontSettings {
                family: "JetBrains Mono".into(),
                size: 10.0,
                fallback: Vec::new(),
            },
            true,
            10_000,
        );
        let win = gtk4::Window::builder()
            .title("render-e2e")
            .default_width(640)
            .default_height(640)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);
        pump_main_loop(80);
        win.set_default_size(640, view.terminal().char_height() as i32 * 36);
        pump_main_loop(80);
        assert_eq!(
            view.allocated_grid_size(),
            (
                view.terminal().column_count() as u16,
                view.terminal().row_count() as u16
            ),
            "reported geometry must exclude VTE padding"
        );
        sparkle_updates_keep_status_row(&view);
        primary_mouse_click_and_native_drag(&view, &win);
        full_frame_clears_with_default_background(&view, &win);
        reverse_video_preserves_application_background(&view);
        attach_history_preserves_authoritative_cursor_and_partial_live_csi(&view);
        view.feed_output(b"\x1b[0m\x1b[2J\x1b[3J\x1b[H");
        view.flush_pending_feed();
        pump_main_loop(80);
        view.clear_render_trace();
        win.set_default_size(640, 640);
        pump_main_loop(80);
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
        wheel_scrolls_vte_history(&view);
        wheel_alt_screen_sends_csi_arrows(&view);
        view.ensure_grid_size(80, 24);
        pump_main_loop(80);
        codex_tui_fixture_keeps_header_and_prompt(&view);
        view.clear_render_trace();
        cup_half_frames_keep_header_and_prompt(&view);
        mini_vte_input_routes_to_send_input(&view);
        surface_typing_overwrites_in_place(&view);
        surface_live_feed_does_not_reset(&view);
        surface_consecutive_full_frames_replace_without_reset(&view);
        surface_codex_fixture_raw_feed(&view);

        // 浅色主题中，旧 Codex 会话可能仍画黑底输入框。必须在真实 VTE
        // 中显示浅字，不改黑底，也不把输入框外的默认文字一起变白。
        view.feed_output(b"\x1b[0m\x1b[2J\x1b[H\x1b[48;2;20;20;20mBLACK_INPUT\x1b[49m NORMAL_TEXT");
        view.flush_pending_feed();
        pump_main_loop(80);
        let html = view
            .terminal()
            .text_format(vte4::Format::Html)
            .unwrap()
            .to_string();
        assert!(
            !html.contains("color=\"#1F2328\">BLACK_INPUT")
                && html.contains("background-color:#141414"),
            "black composer contrast: {html}"
        );
        assert!(
            html.contains("</font></span> NORMAL_TEXT"),
            "default foreground must restore: {html}"
        );

        // 真实 Codex 用 DEC 2026 包住整帧。属性过滤器不能等帧末才修色；
        // 同时覆盖 dim placeholder、浅黄代码、中文与星点的逐字节分包。
        let frame = "\x1b[?2026h\x1b[0m\x1b[2J\x1b[H\x1b[38;2;248;248;180mCODE\x1b[0m\r\n\x1b[48;2;31;31;31m\x1b[2mINPUT中文⠁\x1b[0m\x1b[?2026l";
        for byte in frame.as_bytes() {
            view.feed_output(&[*byte]);
            view.flush_pending_feed();
        }
        pump_main_loop(80);
        let html = view
            .terminal()
            .text_format(vte4::Format::Html)
            .unwrap()
            .to_string();
        assert!(
            html.contains("#747454") && !html.contains("#F8F8B4"),
            "synchronized light syntax: {html}"
        );
        assert!(
            html.contains("#888A8D") && html.contains("INPUT中文"),
            "synchronized dim composer: {html}"
        );
        assert!(
            html.contains("background-color:#1F1F1F"),
            "preserve application background: {html}"
        );

        // Pi/Codex primary-screen 输入框使用相对光标重绘。历史回填不能
        // 把屏幕内的光标误当 scrollback 绝对行，也不能吞掉首帧。
        view.feed_output(b"\x1b[?1049l\x1b[2J\x1b[H");
        view.flush_pending_feed();
        pump_main_loop(80);
        view.begin_attach_generation();
        view.prepend_history(b"older command\nolder result\n");
        view.seed_raw(b"\x1b[2J\x1b[H\x1b[38;2;12;123;234mHEADER\x1b[0m\x1b[10;1HINPUT_BOX\x1b[10;10H\x1b[33m", 80, 24);
        pump_main_loop(80);
        let html = view
            .terminal()
            .text_format(vte4::Format::Html)
            .unwrap()
            .to_string();
        assert!(
            html.contains("#0C7BEA"),
            "history must preserve live colors before any repaint: {html}"
        );
        view.feed_output(b"\r\x1b[2KINPUT_UPDATED ");
        // 刻意在 UTF-8 字符中间分包，覆盖星光/中文的字节边界。
        for byte in "✦ ✧ ⋆ ⠿ 中文".as_bytes() {
            view.feed_output(&[*byte]);
            view.flush_pending_feed();
        }
        pump_main_loop(80);
        let screen = view.screen_text();
        assert!(screen.contains("HEADER"), "{screen:?}");
        assert!(
            !screen.contains("INPUT_BOX"),
            "history moved the cursor: {screen:?}"
        );
        assert!(screen.contains("INPUT_UPDATED ✦ ✧ ⋆ ⠿ 中文"), "{screen:?}");
        view.feed_output(b"\x1b[15;1H");
        for byte in "\x1b[?1003h\x1b[38;2;50;100;150m⠁ ⠂ ⠄ ⠈ ⠐ ⠠ ⡀ ⢀\x1b[0m".as_bytes()
        {
            view.feed_output(&[*byte]);
            view.flush_pending_feed();
        }
        pump_main_loop(80);
        assert!(view.screen_text().contains("⠁ ⠂ ⠄ ⠈ ⠐ ⠠ ⡀ ⢀"));
        if let Some(path) = std::env::var_os("MUXTERM_RENDER_SCREENSHOT") {
            gtk4::test_widget_wait_for_draw(&win);
            let paintable = gtk4::WidgetPaintable::new(Some(&win));
            let snapshot = gtk4::Snapshot::new();
            paintable.snapshot(&snapshot, win.width() as f64, win.height() as f64);
            win.renderer()
                .unwrap()
                .render_texture(snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }

        win.close();
        win.destroy();
        pump_main_loop(40);
    });
}
