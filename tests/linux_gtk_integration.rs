//! Linux GTK4 前端集成测试。
//!
//! 使用 `gtk_test_*`（`test_register_all_types` / `test_list_all_types` /
//! `test_widget_wait_for_draw`）+ `EventControllerKey` 发 `key-pressed`
//! 模拟 Alt+T / Alt+S / Alt+V / Alt+1/2。
//!
//! 2tab3pane 流程（先在 TUI `tui_build_2tab3pane_via_keys_and_echo` 跑通）：
//! Alt+S → Alt+V → Alt+T → Alt+1/2；每步后 `echo` 校验核心缓冲与 VTE 可见文本。
//!
//! 无 DISPLAY 时跳过。本地：`xvfb-run -a cargo test --features gtk --test linux_gtk_integration`

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gtk4::gdk;
use gtk4::glib;
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use gtk4::{EventControllerKey, Orientation, Paned, ToggleButton, Widget};

use muxterm::core::config::{Config, Theme};
use muxterm::platform::linux::ffi_bridge::{BridgeLayout, BridgeTab, SshHostEntry};
use muxterm::platform::linux::keymap::KeyMap;
use muxterm::platform::linux::layout_host::LayoutHost;
use muxterm::platform::linux::quickconnect::font::FontSettings;
use muxterm::platform::linux::quickconnect::store::QuickConnectStore;
use muxterm::platform::linux::tab_bar::TabBar;
use muxterm::platform::linux::target_config_window;
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

fn rand_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

/// 同进程内是否已跑过含 AppWindow 的重用例。
static HEAVY_GTK_UI_DONE: AtomicBool = AtomicBool::new(false);

/// 同进程内是否已跑过 target-config 窗口用例。
/// 与 AppWindow 一样，重复建/析构 GTK 窗口会触发二次析构堆损坏。
static HEAVY_TARGET_CONFIG_DONE: AtomicBool = AtomicBool::new(false);

fn gtk_test_framework_smoke() {
    gtk4::test_register_all_types();
    let types = gtk4::test_list_all_types();
    assert!(!types.is_empty(), "gtk_test_list_all_types 应非空");
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

fn count_paned(root: &impl IsA<Widget>) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.is::<Paned>());
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_paned(&c);
        child = c.next_sibling();
    }
    n
}

fn has_nested_paned(root: &impl IsA<Widget>) -> bool {
    let Some(outer) = find_first_paned(root) else {
        return false;
    };
    outer.start_child().is_some_and(|c| c.is::<Paned>())
        || outer.end_child().is_some_and(|c| c.is::<Paned>())
}

fn widget_label_texts(root: &impl IsA<Widget>) -> Vec<String> {
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

fn find_toggle_with_title(root: &impl IsA<Widget>, title: &str) -> Option<ToggleButton> {
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

/// 复现：新建 Project 打开后 debounce 已触发，再点 SSH 卡片。
/// 旧实现会对已完成的 `SourceId` 再 `remove()`，在 toggled trampoline 里 abort。
fn assert_target_config_ssh_toggle_after_debounce() {
    let parent = gtk4::Window::builder()
        .title("target-config-parent")
        .default_width(320)
        .default_height(200)
        .build();
    parent.present();
    gtk4::test_widget_wait_for_draw(&parent);

    let dialog = target_config_window::show(
        &parent,
        None,
        QuickConnectStore::new(None),
        vec![SshHostEntry {
            alias: "ryzen".into(),
            hostname: "192.168.5.6".into(),
            port: 22,
            user: "wlz".into(),
        }],
        |_| {},
        || {},
    );
    gtk4::test_widget_wait_for_draw(&dialog);
    // 等过 120ms listing debounce，让旧 SourceId 自行失效
    pump_main_loop(200);

    let ssh = find_toggle_with_title(&dialog, "ssh").expect("SSH 卡片");
    let local = find_toggle_with_title(&dialog, "local").expect("Local 卡片");
    assert!(local.is_active());
    assert!(!ssh.is_active());
    ssh.set_active(true);
    pump_main_loop(80);
    assert!(ssh.is_active(), "点 SSH 后应保持选中");
    assert!(!local.is_active(), "Local 应取消选中");

    dialog.close();
    dialog.destroy();
    parent.set_child(None::<&Widget>);
    parent.destroy();
    pump_main_loop(40);
}

fn window_key_controller(window: &impl IsA<Widget>) -> Option<EventControllerKey> {
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
fn simulate_key_press(ctrl: &EventControllerKey, key: gdk::Key, mods: gdk::ModifierType) {
    let keyval: u32 = key.into_glib();
    let keycode: u32 = 0;
    let _handled: bool = ctrl.emit_by_name("key-pressed", &[&keyval, &keycode, &mods]);
}

fn wait_until(app: &AppWindow, ms: u64, mut pred: impl FnMut(&AppWindow) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        app.test_poll_once();
        while glib::MainContext::default().iteration(false) {}
        if pred(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

/// 短唯一标记，避免窄 pane 截断。
fn unique_marker(tag: &str) -> String {
    let s = rand_suffix();
    format!("{tag}{}", &s[s.len().saturating_sub(5)..])
}

/// 向当前激活 pane 发 `echo`，断言核心缓冲与 VTE 可见文本都出现标记。
///
/// 只查状态栏 pane 数无法发现分割后黑屏；必须走真实输入→输出。
fn assert_active_pane_echo(app: &AppWindow, step: &str) {
    let marker = unique_marker(step);
    app.test_send_input(format!("echo {marker}\n").as_bytes());
    let ok = wait_until(app, 5000, |a| {
        let core_buf = a.test_active_pane_output();
        let core = String::from_utf8_lossy(&core_buf);
        let vte = a.test_active_pane_vte_text();
        core.contains(&marker) && vte.contains(&marker)
    });
    let core_buf = app.test_active_pane_output();
    let core = String::from_utf8_lossy(&core_buf);
    let vte = app.test_active_pane_vte_text();
    assert!(
        ok,
        "{step}: echo '{marker}' 应同时出现在核心缓冲与 VTE 可见文本\ncore={core}\nvte={vte}"
    );
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
    assert_eq!(count_css_class(&tabs.container, "tab-button"), 2);
    assert_eq!(
        count_css_class(&tabs.container, "tab-active"),
        1,
        "当前 tab 应有且仅有一个 tab-active 标识"
    );
    let labels = widget_label_texts(&tabs.container);
    assert!(
        labels.iter().any(|t| t == "1:shell"),
        "第 1 个 tab 应标 1: 以对应 Alt+1，got={labels:?}"
    );
    assert!(
        labels.iter().any(|t| t == "2:build"),
        "第 2 个 tab 应标 2: 以对应 Alt+2，got={labels:?}"
    );
    win.set_child(None::<&Widget>);
    win.destroy();
    pump_main_loop(40);
}

fn assert_pane_layout_widget() {
    let mut host = LayoutHost::new(load_theme(), FontSettings::default(), false);
    let layout = BridgeLayout::Split {
        horizontal: true,
        ratio: 500,
        first: Box::new(BridgeLayout::Leaf { pane_id: 1 }),
        second: Box::new(BridgeLayout::Leaf { pane_id: 2 }),
    };
    let on_input = |_pid: u32, _data: &[u8]| {};
    assert!(host.apply_layout(&layout, &on_input));
    let win = gtk4::Window::builder()
        .title("pane-layout-test")
        .default_width(640)
        .default_height(400)
        .child(&host.root_box)
        .build();
    win.present();
    gtk4::test_widget_wait_for_draw(&win);
    let paned = find_first_paned(&host.root_box).expect("Paned");
    assert_eq!(paned.orientation(), Orientation::Horizontal);
    while let Some(child) = host.root_box.first_child() {
        host.root_box.remove(&child);
    }
    host.panes_mut().clear();
    win.set_child(None::<&Widget>);
    win.destroy();
    pump_main_loop(40);
}

/// 经 EventControllerKey 搭 2tab3pane；每步结构变化后 `echo` 校验核心+VTE 输出。
///
/// 键位与 TUI 一致：Alt+S 水平、Alt+V 竖直（激活侧=右侧新 pane）、Alt+T 新 tab。
fn assert_build_2tab3pane_via_keys() {
    let km = KeyMap::from_bindings(&muxterm::core::config::default_keybindings());
    assert_eq!(
        km.lookup(gdk::Key::s, gdk::ModifierType::ALT_MASK),
        Some(muxterm::core::config::Action::NewPane)
    );
    assert_eq!(
        km.lookup(gdk::Key::v, gdk::ModifierType::ALT_MASK),
        Some(muxterm::core::config::Action::NewPaneVertical)
    );
    assert_eq!(
        km.lookup(gdk::Key::t, gdk::ModifierType::ALT_MASK),
        Some(muxterm::core::config::Action::NewTab)
    );

    let app = AppWindow::new(Config::default(), load_theme());
    app.window.set_title(Some("muxterm-gtk-2tab3pane-keys"));
    app.window.present();
    gtk4::test_widget_wait_for_draw(&app.window);
    pump_main_loop(150);

    let ctrl = window_key_controller(&app.window).expect("窗口应有 EventControllerKey");

    // 启动后单 pane 先确认 I/O 通路
    assert_active_pane_echo(&app, "boot");

    // Alt+S → 水平 2 panes
    simulate_key_press(&ctrl, gdk::Key::s, gdk::ModifierType::ALT_MASK);
    assert!(
        wait_until(&app, 2500, |a| a.test_tab_and_pane_counts().1 >= 2
            || a.test_status_text().contains("2 panes")),
        "Alt+S 后应有 2 panes，got={:?} status={}",
        app.test_tab_and_pane_counts(),
        app.test_status_text()
    );
    assert_active_pane_echo(&app, "s2");

    // Alt+V → 在激活（右侧）pane 竖直分割 → 3 panes
    simulate_key_press(&ctrl, gdk::Key::v, gdk::ModifierType::ALT_MASK);
    assert!(
        wait_until(&app, 2500, |a| a.test_tab_and_pane_counts().1 >= 3
            || a.test_status_text().contains("3 panes")),
        "Alt+V 后应有 3 panes，got={:?} status={}",
        app.test_tab_and_pane_counts(),
        app.test_status_text()
    );

    let root = app.window.child().expect("root");
    assert!(
        count_paned(&root) >= 2 && has_nested_paned(&root),
        "3-pane 应为嵌套 Paned"
    );
    assert_active_pane_echo(&app, "v3");

    // Alt+T → 第 2 个 tab
    simulate_key_press(&ctrl, gdk::Key::t, gdk::ModifierType::ALT_MASK);
    assert!(
        wait_until(&app, 2500, |a| {
            let (tabs, panes) = a.test_tab_and_pane_counts();
            tabs >= 2 && (panes == 1 || a.test_status_text().contains("1 pane"))
        }),
        "Alt+T 后应有 2 tabs 且当前 1 pane，got={:?} status={}",
        app.test_tab_and_pane_counts(),
        app.test_status_text()
    );
    let root = app.window.child().expect("root");
    assert_eq!(
        count_css_class(&root, "tab-button"),
        2,
        "tab 栏应显示 2 个 tab"
    );
    assert_active_pane_echo(&app, "t2");

    // Alt+1 → 回到 3-pane tab
    simulate_key_press(&ctrl, gdk::Key::_1, gdk::ModifierType::ALT_MASK);
    assert!(
        wait_until(&app, 2500, |a| a.test_tab_and_pane_counts().1 == 3
            || a.test_status_text().contains("3 panes")),
        "Alt+1 后应 3 panes，got={:?} status={}",
        app.test_tab_and_pane_counts(),
        app.test_status_text()
    );
    assert_active_pane_echo(&app, "a1");

    // Alt+2 → 单 pane tab
    simulate_key_press(&ctrl, gdk::Key::_2, gdk::ModifierType::ALT_MASK);
    assert!(
        wait_until(&app, 2500, |a| a.test_tab_and_pane_counts().1 == 1
            || a.test_status_text().contains("1 pane")),
        "Alt+2 后应 1 pane，got={:?} status={}",
        app.test_tab_and_pane_counts(),
        app.test_status_text()
    );
    assert_active_pane_echo(&app, "a2");

    app.shutdown();
    pump_main_loop(250);
}

#[test]
fn gtk_test_types_registered() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(gtk_test_framework_smoke);
}

#[test]
fn gtk_linux_ui_integration() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        assert_tab_bar_renders();
        assert_pane_layout_widget();
        // 与 AppWindow 相同，target-config 窗口用例同进程只跑一次，
        // 重复建/析构会触发二次析构堆损坏。
        if !HEAVY_TARGET_CONFIG_DONE.load(Ordering::SeqCst) {
            assert_target_config_ssh_toggle_after_debounce();
            HEAVY_TARGET_CONFIG_DONE.store(true, Ordering::SeqCst);
        }
        // 若 gtk_build_* 已跑过 AppWindow，跳过重段避免二次析构堆损坏
        if !HEAVY_GTK_UI_DONE.load(Ordering::SeqCst) {
            assert_build_2tab3pane_via_keys();
            HEAVY_GTK_UI_DONE.store(true, Ordering::SeqCst);
        }
    });
}

/// 可单独过滤：新建 Project 点 SSH（debounce 已触发后不得 abort）。
#[test]
fn gtk_target_config_ssh_toggle_after_debounce() {
    if skip_no_display() {
        return;
    }
    if HEAVY_TARGET_CONFIG_DONE.load(Ordering::SeqCst) {
        eprintln!("skip: 同进程已由 gtk_linux_ui_integration 覆盖");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        assert_target_config_ssh_toggle_after_debounce();
        HEAVY_TARGET_CONFIG_DONE.store(true, Ordering::SeqCst);
    });
}

/// 可单独过滤：`… gtk_z_build_2tab3pane_via_keys -- --exact`
/// 名字排在 `gtk_linux_ui_integration` 之后，全量跑时由前者覆盖后 skip。
#[test]
fn gtk_z_build_2tab3pane_via_keys() {
    if skip_no_display() {
        return;
    }
    if HEAVY_GTK_UI_DONE.load(Ordering::SeqCst) {
        eprintln!("skip: 同进程已由 gtk_linux_ui_integration 覆盖");
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        assert_build_2tab3pane_via_keys();
        HEAVY_GTK_UI_DONE.store(true, Ordering::SeqCst);
    });
}

#[cfg(test)]
mod keymap_alt_s_v {
    use super::*;
    use muxterm::core::config::{default_keybindings, Action};

    #[test]
    fn default_bindings_include_alt_s_and_alt_v() {
        let km = KeyMap::from_bindings(&default_keybindings());
        // 不依赖 DISPLAY：用 lookup_str
        assert_eq!(km.lookup_str("s", &["alt"]), Some(Action::NewPane));
        assert_eq!(km.lookup_str("v", &["alt"]), Some(Action::NewPaneVertical));
        assert_eq!(km.lookup_str("t", &["alt"]), Some(Action::NewTab));
        assert_eq!(km.lookup_str("q", &["control"]), Some(Action::Quit));
        assert_eq!(
            km.lookup_str("c", &["control", "shift"]),
            Some(Action::Copy)
        );
        assert_eq!(
            km.lookup_str("v", &["control", "shift"]),
            Some(Action::Paste)
        );
    }
}
