//! Wave 3 的无头验收：宽字符、历史视口、滚轮路由都要能脱离真窗口验证。
//!
//! 这些断言对应 macOS `ChromeTests/FlatChromeTests.swift` 的选词 / 滚轮 /
//! `ScreenText` 组，以及 `Terminal/TerminalView.swift` 的滚轮分流。

#![cfg(feature = "gpui")]

use gpui::{
    div, prelude::*, px, size, Context, Modifiers, Point, Render, ScrollDelta, ScrollWheelEvent,
    TouchPhase, Window,
};
use muxterm::frontend::gpui::model::scroll::{
    route_wheel, wheel_lines, wheel_outcome, WheelOutcome, WheelRoute,
};
use muxterm::frontend::gpui::terminal::grid::{char_width, FILLER};
use muxterm::frontend::gpui::terminal::mouse::{self, Button, MouseEventKind, MouseMode};
use muxterm::frontend::gpui::terminal::Surface;

/// 一个只画历史视口首行的小视图：用来证明滚轮真的改了视口。
struct PaneProbe {
    surface: Surface,
    accumulator: f32,
    last_outcome: Option<WheelOutcome>,
}

impl Render for PaneProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let top: String = self
            .surface
            .visible_row(0)
            .map(|cells| cells.iter().map(|cell| cell.ch).collect())
            .unwrap_or_default();
        let live: String = self
            .surface
            .grid()
            .screen_lines()
            .first()
            .cloned()
            .unwrap_or_default();
        div()
            .id("pane-probe")
            .size_full()
            .child(format!("top={}", top.trim_end()))
            .child(format!("live={live}"))
    }
}

fn probe(cols: usize, rows: usize) -> PaneProbe {
    let mut surface = Surface::new(cols, rows);
    surface.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
    PaneProbe {
        surface,
        accumulator: 0.0,
        last_outcome: None,
    }
}

fn wheel_event(line_delta: f32) -> ScrollWheelEvent {
    ScrollWheelEvent {
        position: Point::new(px(1.), px(1.)),
        delta: ScrollDelta::Lines(Point::new(0.0, line_delta)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    }
}

/// 拿到无头窗口，送一次滚轮，视口必须真的往历史里走。
#[gpui::test]
fn wheel_scrolls_the_history_headlessly(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| probe(10, 3));
    cx.simulate_resize(size(px(400.), px(300.)));
    cx.simulate_mouse_move(Point::new(px(1.), px(1.)), None, Modifiers::default());

    let before = cx.update(|_, cx| {
        view.update(cx, |probe, cx| {
            let top = probe.surface.visible_row(0).unwrap()[0].ch;
            let live = probe.surface.scroll_offset();
            // 视图自己的滚轮处理：与 `GpuiApp::handle_wheel` 同一套决策。
            let lines = wheel_lines(1.0, false, 16.0, &mut probe.accumulator);
            let outcome = wheel_outcome(false, false, false, false, lines);
            probe.last_outcome = outcome;
            if let Some(WheelOutcome::History(lines)) = outcome {
                probe.surface.scroll_by(lines);
            }
            cx.notify();
            (top, live, probe.surface.scroll_offset())
        })
    });
    let (top_ch, before_offset, after_offset) = before;
    assert_eq!(before_offset, 0);
    assert_eq!(after_offset, 1, "滚轮一档就是一历史行");
    assert_ne!(
        top_ch,
        view.update(cx, |probe, _| probe.surface.visible_row(0).unwrap()[0].ch),
        "视口顶行必须换成历史行"
    );

    // 再送一次真实事件，确认事件路径也通（未接 handler 时不会 panic）。
    cx.simulate_event(wheel_event(1.0));
}

/// 宽字符：占两格，占位格不进可见文本。
#[gpui::test]
fn wide_glyphs_keep_their_second_column_out_of_screen_text(_cx: &mut gpui::TestAppContext) {
    // 12 列够放下 "中文 ok"（中/文 各两格），不会触发换行滚动。
    let mut surface = Surface::new(12, 1);
    surface.feed("中文 ok".as_bytes());
    assert_eq!(char_width('中'), 2);
    assert_eq!(surface.grid().cell(0, 1).unwrap().ch, FILLER);
    assert_eq!(surface.grid().screen_lines(), vec!["中文 ok".to_string()]);
}

/// 滚轮路由的矩阵：服务端 / 应用鼠标 / 方向键 / 本地历史。
#[gpui::test]
fn wheel_route_matrix(_cx: &mut gpui::TestAppContext) {
    assert_eq!(
        route_wheel(true, false, false, true),
        WheelRoute::ServerScroll
    );
    assert_eq!(
        route_wheel(true, false, false, false),
        WheelRoute::ApplicationMouse
    );
    assert_eq!(
        route_wheel(false, false, true, false),
        WheelRoute::ApplicationArrows
    );
    assert_eq!(
        route_wheel(false, false, false, false),
        WheelRoute::LocalHistory
    );
}

/// 鼠标上报：模式门禁 + SGR 编码，字节级对齐 xterm。
#[gpui::test]
fn mouse_reports_follow_the_application_mode(_cx: &mut gpui::TestAppContext) {
    let mut surface = Surface::new(10, 4);
    assert_eq!(
        mouse::encode(
            MouseEventKind::Press(Button::Left),
            0,
            0,
            surface.grid().mouse_mode(),
            true
        ),
        None,
        "应用没开鼠标时一个字节都不发"
    );

    surface.feed(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(surface.grid().mouse_mode(), MouseMode::Normal);
    assert!(surface.grid().sgr_mouse());
    assert_eq!(
        mouse::encode(
            MouseEventKind::Press(Button::Left),
            4,
            2,
            surface.grid().mouse_mode(),
            surface.grid().sgr_mouse()
        )
        .unwrap(),
        b"\x1b[<0;5;3M".to_vec()
    );
    assert_eq!(
        mouse::encode(
            MouseEventKind::Wheel(true, 2),
            0,
            0,
            surface.grid().mouse_mode(),
            surface.grid().sgr_mouse()
        )
        .unwrap(),
        b"\x1b[<64;1;1M\x1b[<64;1;1M".to_vec()
    );
}
