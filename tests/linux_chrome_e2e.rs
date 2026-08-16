//! 统一 status bar e2e（普通 GTK Window，无 AppWindow）。
//!
//! LINUX-PLAN §5.4 S5 / S6 / S13a。本机 xvfb/Mesa 在第二个 GTK 窗口 present
//! 时崩溃，因此整个 crate 只用一个 Window，场景函数名与计划一致、顺序执行。

#![cfg(feature = "gtk")]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::config::Theme;
use muxterm::platform::linux::quickconnect::status_style::{
    StatusBarMode, StatusBarSnapshot, StatusBarWindow,
};
use muxterm::platform::linux::status_bar::StatusBar;

fn theme() -> Theme {
    Theme::load("light").unwrap_or_else(|_| Theme {
        name: "test".into(),
        background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
        foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
        cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
        colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
    })
}

fn snapshot(left: &str, right: &str, windows: Vec<StatusBarWindow>) -> StatusBarSnapshot {
    StatusBarSnapshot {
        enabled: true,
        position: "bottom".into(),
        justify: "left".into(),
        interval: 1,
        left: left.into(),
        right: right.into(),
        left_length: 40,
        right_length: 40,
        status_style: String::new(),
        left_style: String::new(),
        right_style: String::new(),
        separator: " ".into(),
        window_format: String::new(),
        window_current_format: String::new(),
        window_style: String::new(),
        window_current_style: String::new(),
        windows,
        error: None,
    }
}

fn wnd(id: u32, name: &str, current: bool) -> StatusBarWindow {
    StatusBarWindow {
        window_id: id,
        index: id,
        name: name.into(),
        flags: if current { "*".into() } else { String::new() },
        current,
        text: name.into(),
    }
}

/// S5：一条 status bar，左/中/右 + 三个 chrome 按钮，没有第二条 tab-bar。
fn status_bar_has_left_center_right_and_chrome_buttons(bar: &StatusBar, win: &gtk4::Window) {
    let root = find_by_name(win, "muxterm-status-bar").expect("status bar 应存在");
    assert!(root.is_visible(), "status bar 应可见");
    assert!(
        find_by_name(win, "muxterm-status-dot").is_some(),
        "状态点按钮应存在"
    );
    assert!(
        find_by_name(win, "muxterm-status-notify").is_some(),
        "通知按钮应存在"
    );
    assert!(
        find_by_name(win, "muxterm-new-tab").is_some(),
        "新建 tab 按钮应存在"
    );
    assert_eq!(
        count_css_class(win, "tab-bar"),
        0,
        "窗口内不应有第二条 tab-bar 带子"
    );

    bar.apply(&snapshot(
        "L",
        "R",
        vec![wnd(18, "code", true), wnd(21, "other", false)],
    ));
    pump_main_loop(40);

    let left = find_by_name(win, "muxterm-status-left")
        .expect("left 应存在")
        .downcast::<gtk4::Label>()
        .expect("Label 类型");
    assert!(left.text().contains('L'), "left 应含 L: {}", left.text());
    let right = find_by_name(win, "muxterm-status-right")
        .expect("right 应存在")
        .downcast::<gtk4::Label>()
        .expect("Label 类型");
    assert!(right.text().contains('R'), "right 应含 R: {}", right.text());
    assert!(
        find_by_name(win, "muxterm-status-tab-18").is_some(),
        "中区应有 tab-18"
    );
    assert!(
        find_by_name(win, "muxterm-status-tab-21").is_some(),
        "中区应有 tab-21"
    );
}

/// S6：通知按钮 n>0 显示数字，点击回调一次。
fn notify_button_invokes_attention_callback_when_n_positive(bar: &StatusBar, win: &gtk4::Window) {
    let clicks = Rc::new(RefCell::new(0usize));
    let c = clicks.clone();
    bar.connect_attention_activate(move || *c.borrow_mut() += 1);

    bar.set_attention(2);
    pump_main_loop(40);
    let notify = find_by_name(win, "muxterm-status-notify")
        .expect("通知按钮应存在")
        .downcast::<gtk4::Button>()
        .expect("Button 类型");
    assert!(
        notify
            .label()
            .map(|l| l.to_string())
            .unwrap_or_default()
            .contains('2'),
        "n=2 时按钮文本应含 2: {:?}",
        notify.label()
    );
    let _: () = notify.emit_by_name("clicked", &[]);
    pump_main_loop(40);
    assert_eq!(*clicks.borrow(), 1, "点击应触发回调一次");
}

/// S13a：点 tab-21 回调收到 21；同一 snapshot 再 apply 不重建按钮。
fn click_status_tab_invokes_switch_with_window_id(bar: &StatusBar, win: &gtk4::Window) {
    let switched = Rc::new(RefCell::new(Vec::<u32>::new()));
    let s = switched.clone();
    bar.connect_window_activate(move |id| s.borrow_mut().push(id));

    let snap = snapshot(
        "L",
        "R",
        vec![wnd(18, "code", true), wnd(21, "other", false)],
    );
    bar.apply(&snap);
    pump_main_loop(40);

    let tab21 = find_by_name(win, "muxterm-status-tab-21")
        .expect("tab-21 应存在")
        .downcast::<gtk4::Button>()
        .expect("Button 类型");
    let _: () = tab21.emit_by_name("clicked", &[]);
    pump_main_loop(40);
    assert_eq!(*switched.borrow(), vec![21], "回调应收到 21 而不是 1");

    // 同一 snapshot 再 apply：签名不变，按钮不重建（widget 指针不变）。
    let before = find_by_name(win, "muxterm-status-tab-21")
        .expect("apply 前 tab-21 应存在")
        .as_ptr() as usize;
    bar.apply(&snap);
    pump_main_loop(40);
    let after = find_by_name(win, "muxterm-status-tab-21")
        .expect("apply 后 tab-21 应仍在（不重建）")
        .as_ptr() as usize;
    assert_eq!(before, after, "签名不变时按钮指针应保持不变");
}

/// S7（C8.4）：点状态点（emit clicked）打开 popover，SSH 摘要 + 真实颜色。
fn status_dot_click_opens_popover_with_ssh_summary(bar: &StatusBar, win: &gtk4::Window) {
    use muxterm::platform::linux::status_bar::ConnectionSummary;
    bar.set_connection_summary(&ConnectionSummary {
        kind: "ssh".into(),
        host: Some("127.0.0.1".into()),
        status: "connected".into(),
        down: 1536,
        up: 56,
        down_rate: 1536,
        up_rate: 56,
    });
    pump_main_loop(40);

    let dot = find_by_name(win, "muxterm-status-dot")
        .expect("状态点应存在")
        .downcast::<gtk4::Button>()
        .expect("Button 类型");
    assert!(dot.has_css_class("status-ok"), "connected 应为 status-ok");
    let popover = find_by_name(win, "muxterm-status-popover")
        .expect("popover 应存在")
        .downcast::<gtk4::Popover>()
        .expect("Popover 类型");
    // 必须 emit clicked（禁止直接调 popover 的 popup 冒充点击）。
    let _: () = dot.emit_by_name("clicked", &[]);
    pump_main_loop(40);
    assert!(popover.is_visible(), "点状态点后 popover 应可见");
    let label = find_by_name(win, "muxterm-status-popover-label")
        .expect("popover label 应存在")
        .downcast::<gtk4::Label>()
        .expect("Label 类型");
    let text = label.text().to_string();
    assert!(text.contains("type=ssh"), "应含 type=ssh: {text}");
    assert!(text.contains("host=127.0.0.1"), "应含 host: {text}");
    assert!(text.contains("status=connected"), "应含 status: {text}");
    assert!(
        !text.contains("1536B/s") && !text.contains("1234B/s"),
        "禁止把累计字节标成 B/s: {text}"
    );
    assert!(
        text.contains("1.5 KB/s"),
        "必须有人类可读速率 1.5 KB/s: {text}"
    );
    assert!(
        text.contains("1.5 KB") && text.contains("56 B"),
        "必须有人类可读累计（1.5 KB 和 56 B）: {text}"
    );
    popover.popdown();

    // CSS 数据必须含真实颜色（status-ok 绿）。
    let css = muxterm::platform::linux::status_bar::status_dot_css();
    assert!(css.contains("#27ae60"), "status-ok 应有绿色: {css}");
    assert!(css.contains("#f39c12"), "status-warn 应有黄色: {css}");
    assert!(css.contains("#c0392b"), "status-err 应有红色: {css}");
}

/// S13a 的签名部分：独立断言函数名（与计划一致）。
fn status_bar_does_not_rebuild_buttons_when_tab_signature_unchanged(
    bar: &StatusBar,
    win: &gtk4::Window,
) {
    let snap = snapshot(
        "L",
        "R",
        vec![wnd(18, "code", true), wnd(21, "other", false)],
    );
    bar.apply(&snap);
    pump_main_loop(40);
    let before = find_by_name(win, "muxterm-status-tab-21")
        .expect("tab-21 应存在")
        .as_ptr() as usize;

    for _ in 0..3 {
        bar.apply(&snap);
        pump_main_loop(20);
    }
    let after = find_by_name(win, "muxterm-status-tab-21")
        .expect("tab-21 应仍在")
        .as_ptr() as usize;
    assert_eq!(before, after, "签名不变不得重建按钮");
}

#[test]
fn chrome_e2e_s5_s6_s13a() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let bar = StatusBar::new(StatusBarMode::Theme, theme());
        let win = gtk4::Window::builder()
            .title("chrome-e2e")
            .default_width(800)
            .default_height(60)
            .child(&bar.container)
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);

        status_bar_has_left_center_right_and_chrome_buttons(&bar, &win);
        notify_button_invokes_attention_callback_when_n_positive(&bar, &win);
        click_status_tab_invokes_switch_with_window_id(&bar, &win);
        status_bar_does_not_rebuild_buttons_when_tab_signature_unchanged(&bar, &win);
        status_dot_click_opens_popover_with_ssh_summary(&bar, &win);

        drop(bar);
        win.close();
        win.destroy();
        pump_main_loop(40);
    });
}
