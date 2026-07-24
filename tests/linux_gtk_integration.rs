//! Linux GTK4 前端集成测试。
//!
//! 使用 GTK 测试工具（C API `gtk_test_*` 的 gtk4-rs 绑定）：
//! - [`gtk4::test_register_all_types`] / [`gtk4::test_list_all_types`]
//! - [`gtk4::test_widget_wait_for_draw`]
//!
//! 键盘：GTK4 已移除 `gtk_test_widget_send_key`，改为对
//! `EventControllerKey` 发射 `key-pressed` 信号模拟按键。
//!
//! UI 场景放在**同一次** `gtk4::test_synced` 里串行跑，避免多次
//! CoreBridge/VTE 生命周期交错导致的 GObject 堆损坏。
//!
//! 无 `DISPLAY`/`WAYLAND_DISPLAY` 时跳过（CI 无显示器）。
//! 本地验证：`xvfb-run -a cargo test --features gtk --test linux_gtk_integration`

use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::glib;
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use gtk4::{Button, EventControllerKey, Orientation, Paned, Widget};

use muxterm::core::config::{Config, Theme};
use muxterm::platform::linux::ffi_bridge::{BridgeLayout, BridgeTab};
use muxterm::platform::linux::keymap::KeyMap;
use muxterm::platform::linux::layout_host::LayoutHost;
use muxterm::platform::linux::tab_bar::TabBar;
use muxterm::platform::linux::window::AppWindow;

fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn skip_no_display() -> bool {
    if has_display() {
        return false;
    }
    eprintln!("skip: 无 DISPLAY/WAYLAND_DISPLAY（可用 xvfb-run -a cargo test --features gtk）");
    true
}

fn gtk_test_framework_smoke() {
    gtk4::test_register_all_types();
    let types = gtk4::test_list_all_types();
    assert!(
        !types.is_empty(),
        "gtk_test_list_all_types 应返回已注册类型"
    );
}

fn load_theme() -> Theme {
    Theme::load("light").unwrap_or_else(|_| Theme {
        name: "test".into(),
        background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
        foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
        cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
        colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
    })
}

fn pump_main_loop(ms: u64) {
    let start = Instant::now();
    let ctx = glib::MainContext::default();
    while start.elapsed() < Duration::from_millis(ms) {
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn find_by_css_class(root: &impl IsA<Widget>, class: &str) -> Option<Widget> {
    let root = root.as_ref();
    if root.has_css_class(class) {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_by_css_class(&c, class) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn count_css_class(root: &impl IsA<Widget>, class: &str) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.has_css_class(class));
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_css_class(&c, class);
        child = c.next_sibling();
    }
    n
}

fn find_first_paned(root: &impl IsA<Widget>) -> Option<Paned> {
    let root = root.as_ref();
    if let Ok(p) = root.clone().downcast::<Paned>() {
        return Some(p);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_first_paned(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn window_key_controller(window: &impl IsA<Widget>) -> Option<EventControllerKey> {
    let list = window.as_ref().observe_controllers();
    let n = list.n_items();
    for i in 0..n {
        if let Some(obj) = list.item(i) {
            if let Ok(ctrl) = obj.downcast::<EventControllerKey>() {
                return Some(ctrl);
            }
        }
    }
    None
}

/// 模拟按键（替代已移除的 `gtk_test_widget_send_key`）。
fn simulate_key_press(ctrl: &EventControllerKey, key: gdk::Key, mods: gdk::ModifierType) {
    let keyval: u32 = key.into_glib();
    let keycode: u32 = 0;
    let _handled: bool = ctrl.emit_by_name("key-pressed", &[&keyval, &keycode, &mods]);
}

fn assert_tab_bar_renders() {
    let tabs = TabBar::new(28);
    let win = gtk4::Window::builder()
        .title("tab-bar-test")
        .default_width(400)
        .default_height(80)
        .child(&tabs.container)
        .build();
    win.present();

    tabs.set_tabs(&[
        BridgeTab {
            id: 1,
            name: "shell".into(),
            is_active: true,
        },
        BridgeTab {
            id: 2,
            name: "build".into(),
            is_active: false,
        },
    ]);

    gtk4::test_widget_wait_for_draw(&win);

    assert!(
        tabs.container.has_css_class("tab-bar"),
        "容器应有 tab-bar class"
    );
    assert_eq!(count_css_class(&tabs.container, "tab-button"), 2);

    let mut labels = Vec::new();
    let mut active_count = 0usize;
    let mut child = tabs.container.first_child();
    while let Some(c) = child {
        let btn = c
            .downcast_ref::<Button>()
            .expect("tab-bar 子控件应为 Button");
        labels.push(btn.label().unwrap_or_default().to_string());
        if btn.has_css_class("active") {
            active_count += 1;
        }
        child = c.next_sibling();
    }
    assert_eq!(labels, vec!["shell".to_string(), "build".to_string()]);
    assert_eq!(active_count, 1, "应有且仅有一个 active tab");

    win.set_child(None::<&Widget>);
    win.destroy();
    pump_main_loop(40);
}

fn assert_pane_layout() {
    let theme = load_theme();
    let mut host = LayoutHost::new(theme);
    let layout = BridgeLayout::Split {
        horizontal: true,
        ratio: 500,
        first: Box::new(BridgeLayout::Leaf { pane_id: 1 }),
        second: Box::new(BridgeLayout::Leaf { pane_id: 2 }),
    };
    let on_input = |_pid: u32, _data: &[u8]| {};
    assert!(host.apply_layout(&layout, &on_input), "首次布局应重建");

    let win = gtk4::Window::builder()
        .title("pane-layout-test")
        .default_width(640)
        .default_height(400)
        .child(&host.root_box)
        .build();
    win.present();
    gtk4::test_widget_wait_for_draw(&win);

    let paned = find_first_paned(&host.root_box).expect("水平 split 应产生 Paned");
    assert_eq!(paned.orientation(), Orientation::Horizontal);
    assert!(paned.start_child().is_some(), "Paned 应有 start child");
    assert!(paned.end_child().is_some(), "Paned 应有 end child");
    assert!(host.pane(1).is_some());
    assert!(host.pane(2).is_some());
    assert!(!host.apply_layout(&layout, &on_input));

    while let Some(child) = host.root_box.first_child() {
        host.root_box.remove(&child);
    }
    host.panes_mut().clear();
    win.set_child(None::<&Widget>);
    win.destroy();
    pump_main_loop(40);
}

fn assert_app_window_and_keyboard() {
    let km = KeyMap::from_bindings(&muxterm::core::config::default_keybindings());
    assert_eq!(
        km.lookup(gdk::Key::n, gdk::ModifierType::ALT_MASK),
        Some(muxterm::core::config::Action::NewWindow)
    );

    let cfg = Config::default();
    let theme = load_theme();
    let app = AppWindow::new(cfg, theme);
    app.window.set_title(Some("muxterm-gtk-test"));
    app.window.present();

    gtk4::test_widget_wait_for_draw(&app.window);
    pump_main_loop(120);

    assert!(app.window.is_visible(), "present + wait_for_draw 后应可见");
    assert_eq!(app.window.title().as_deref(), Some("muxterm-gtk-test"));

    let root = app.window.child().expect("AppWindow 应有根 child");
    assert!(
        root.has_css_class("muxterm-root") || find_by_css_class(&root, "muxterm-root").is_some(),
        "根容器应带 muxterm-root"
    );
    assert!(
        find_by_css_class(&root, "tab-bar").is_some(),
        "应渲染 tab-bar"
    );
    assert!(
        find_by_css_class(&root, "status-bar").is_some(),
        "应有 status-bar"
    );

    let before = count_css_class(&root, "tab-button");
    assert!(before >= 1, "local 后端启动后至少 1 个 tab，实际 {before}");

    let ctrl = window_key_controller(&app.window).expect("窗口应挂有 EventControllerKey");
    simulate_key_press(&ctrl, gdk::Key::n, gdk::ModifierType::ALT_MASK);
    pump_main_loop(400);

    let root = app.window.child().expect("root");
    let after = count_css_class(&root, "tab-button");
    assert!(
        after > before,
        "Alt+n 后 tab 数应增加：before={before} after={after}"
    );

    app.shutdown();
    // 等待 local pty 读线程与 GObject 析构收尾
    pump_main_loop(200);
}

/// 冒烟：`gtk_test_register_all_types` / `gtk_test_list_all_types`。
#[test]
fn gtk_test_types_registered() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
    });
}

/// 启动窗口、tab 栏、pane 布局、键盘模拟（同一次 GTK 主线程会话）。
#[test]
fn gtk_linux_ui_integration() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        assert_tab_bar_renders();
        assert_pane_layout();
        assert_app_window_and_keyboard();
    });
}
