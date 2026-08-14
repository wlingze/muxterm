//! Linux GTK4 测试共享助手。
//!
//! 从 `tests/linux_gtk_integration.rs` 抽出，供所有 GTK e2e 复用：
//! 无 DISPLAY 跳过、主循环推进、widget 树查找/断言、按键模拟。
//! 本模块不构造 `AppWindow`；含 AppWindow 的用例由各 e2e crate 自行管理。

#![allow(dead_code)]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gtk4::gdk;
use gtk4::glib;
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use gtk4::{EventControllerKey, Paned, ToggleButton, Widget};

use muxterm::core::config::Theme;

/// 是否有可用的显示服务（X11 或 Wayland）。
pub fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// 无 DISPLAY 时打印跳过原因并返回 true（调用方直接 return）。
pub fn skip_no_display() -> bool {
    if has_display() {
        return false;
    }
    eprintln!("skip: 无 DISPLAY/WAYLAND_DISPLAY（可用 xvfb-run -a cargo test --features gtk）");
    true
}

/// 短唯一后缀，用于隔离 tmux socket 名与输出标记。
pub fn rand_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

/// GTK 测试类型注册冒烟：确保 `gtk_test_*` 可用。
pub fn gtk_test_framework_smoke() {
    gtk4::test_register_all_types();
    let types = gtk4::test_list_all_types();
    assert!(!types.is_empty(), "gtk_test_list_all_types 应非空");
}

/// 加载 light 主题；失败时退回测试用固定主题。
pub fn load_theme() -> Theme {
    Theme::load("light").unwrap_or_else(|_| Theme {
        name: "test".into(),
        background: muxterm::core::config::Rgb(0x1e, 0x1e, 0x2e),
        foreground: muxterm::core::config::Rgb(0xcd, 0xd6, 0xf4),
        cursor: muxterm::core::config::Rgb(0xf5, 0xe0, 0xdc),
        colors: [muxterm::core::config::Rgb(0, 0, 0); 16],
    })
}

/// 推进 GTK 主循环约 `ms` 毫秒（iteration + 5ms sleep）。
pub fn pump_main_loop(ms: u64) {
    let start = Instant::now();
    let ctx = glib::MainContext::default();
    while start.elapsed() < Duration::from_millis(ms) {
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 在 deadline 内推进主循环并轮询 `pred`，返回是否在超时前满足。
pub fn wait_until_widget(ms: u64, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let ctx = glib::MainContext::default();
    while start.elapsed() < Duration::from_millis(ms) {
        while ctx.iteration(false) {}
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

/// 递归统计 widget 树中带指定 CSS class 的节点数。
pub fn count_css_class(root: &impl IsA<Widget>, class: &str) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.has_css_class(class));
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_css_class(&c, class);
        child = c.next_sibling();
    }
    n
}

/// 递归查找第一个 `Paned`。
pub fn find_first_paned(root: &impl IsA<Widget>) -> Option<Paned> {
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

/// 递归统计 `Paned` 数量。
pub fn count_paned(root: &impl IsA<Widget>) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.is::<Paned>());
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_paned(&c);
        child = c.next_sibling();
    }
    n
}

/// 是否存在嵌套 Paned（外层任一侧是 Paned）。
pub fn has_nested_paned(root: &impl IsA<Widget>) -> bool {
    let Some(outer) = find_first_paned(root) else {
        return false;
    };
    outer.start_child().is_some_and(|c| c.is::<Paned>())
        || outer.end_child().is_some_and(|c| c.is::<Paned>())
}

/// 递归收集所有 `Label` 文本。
pub fn widget_label_texts(root: &impl IsA<Widget>) -> Vec<String> {
    let root = root.as_ref();
    let mut out = Vec::new();
    if let Ok(label) = root.clone().downcast::<gtk4::Label>() {
        out.push(label.text().to_string());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        out.extend(widget_label_texts(&c));
        child = c.next_sibling();
    }
    out
}

/// 递归查找 `widget_name` 等于 `name` 的控件（LINUX-PLAN §0.5 契约）。
pub fn find_by_name(root: &impl IsA<Widget>, name: &str) -> Option<Widget> {
    let root = root.as_ref();
    if root.widget_name() == name {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_by_name(&c, name) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// 递归查找标题文本等于 `title` 的 ToggleButton。
pub fn find_toggle_with_title(root: &impl IsA<Widget>, title: &str) -> Option<ToggleButton> {
    let root = root.as_ref();
    if let Ok(btn) = root.clone().downcast::<ToggleButton>() {
        if widget_label_texts(root).iter().any(|t| t == title) {
            return Some(btn);
        }
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_toggle_with_title(&c, title) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// 取窗口上挂的 `EventControllerKey`（用于模拟按键）。
pub fn window_key_controller(window: &impl IsA<Widget>) -> Option<EventControllerKey> {
    let list = window.as_ref().observe_controllers();
    for i in 0..list.n_items() {
        if let Some(obj) = list.item(i) {
            if let Ok(ctrl) = obj.downcast::<EventControllerKey>() {
                return Some(ctrl);
            }
        }
    }
    None
}

/// 模拟按键（GTK4 已移除 `gtk_test_widget_send_key`）。
pub fn simulate_key_press(ctrl: &EventControllerKey, key: gdk::Key, mods: gdk::ModifierType) {
    let keyval: u32 = key.into_glib();
    let keycode: u32 = 0;
    let _handled: bool = ctrl.emit_by_name("key-pressed", &[&keyval, &keycode, &mods]);
}

/// 短唯一标记，避免窄 pane 截断。
pub fn unique_marker(tag: &str) -> String {
    let s = rand_suffix();
    format!("{tag}{}", &s[s.len().saturating_sub(5)..])
}
