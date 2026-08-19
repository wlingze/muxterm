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

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

fn count_widget_names(root: &impl IsA<Widget>, prefix: &str) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.widget_name().starts_with(prefix));
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_widget_names(&c, prefix);
        child = c.next_sibling();
    }
    n
}
use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Orientation, Widget};

use muxterm::core::config::Config;
use muxterm::core::quickconnect::model::TargetRuntime;
use muxterm::platform::linux::ffi_bridge::{BridgeTab, SshHostEntry};
use muxterm::platform::linux::keymap::KeyMap;
use muxterm::platform::linux::layout_host::LayoutHost;
use muxterm::platform::linux::quickconnect::font::FontSettings;
use muxterm::platform::linux::quickconnect::store::QuickConnectStore;
use muxterm::platform::linux::tab_bar::TabBar;
use muxterm::platform::linux::target_config_window;
use muxterm::platform::linux::window::AppWindow;

use support::linux_gtk::*;

/// 同进程内是否已跑过含 AppWindow 的重用例。
static HEAVY_GTK_UI_DONE: AtomicBool = AtomicBool::new(false);

/// 同进程内是否已跑过 target-config 窗口用例。
/// 与 AppWindow 一样，重复建/析构 GTK 窗口会触发二次析构堆损坏。
static HEAVY_TARGET_CONFIG_DONE: AtomicBool = AtomicBool::new(false);

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
        muxterm::core::catalog::Catalog::with_builtins().runtime_list(),
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

/// W20g：新建项目有 Herdr runtime 卡；点它保存后 on_save 收到 Herdr。
fn assert_target_config_herdr_card_saves() {
    let parent = gtk4::Window::builder()
        .title("target-config-herdr-parent")
        .default_width(320)
        .default_height(200)
        .build();
    parent.present();
    gtk4::test_widget_wait_for_draw(&parent);

    let saved = Rc::new(RefCell::new(None::<TargetRuntime>));
    let s = saved.clone();
    let dialog = target_config_window::show(
        &parent,
        None,
        QuickConnectStore::new(None),
        vec![],
        muxterm::core::catalog::Catalog::with_builtins().runtime_list(),
        move |cfg| {
            *s.borrow_mut() = Some(cfg.runtime);
        },
        || {},
    );
    gtk4::test_widget_wait_for_draw(&dialog);
    pump_main_loop(80);

    let herdr = find_by_name(&dialog, "muxterm-runtime-herdr")
        .expect("新建项目必须有 muxterm-runtime-herdr 卡")
        .downcast::<gtk4::ToggleButton>()
        .expect("Herdr 卡应是 ToggleButton");
    assert!(!herdr.is_active());
    herdr.set_active(true);
    pump_main_loop(40);
    assert!(herdr.is_active(), "点 Herdr 卡后应保持选中");

    let save = find_by_name(&dialog, "muxterm-target-config-save")
        .expect("保存按钮应存在")
        .downcast::<gtk4::Button>()
        .expect("Button 类型");
    save.emit_clicked();
    pump_main_loop(40);
    assert_eq!(
        *saved.borrow(),
        Some(TargetRuntime::Herdr),
        "保存后 on_save 必须收到 TargetRuntime::Herdr"
    );

    dialog.close();
    dialog.destroy();
    parent.set_child(None::<&Widget>);
    parent.destroy();
    pump_main_loop(40);
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
    let mut host = LayoutHost::new(load_theme(), FontSettings::default(), true, 10_000);
    use muxterm::core::model::layout::{LayoutNode, SplitDir};
    use muxterm::core::types::PaneId;
    let layout = LayoutNode::Split {
        dir: SplitDir::Horizontal,
        ratio: 500,
        first: std::boxed::Box::new(LayoutNode::Leaf(PaneId(1))),
        second: std::boxed::Box::new(LayoutNode::Leaf(PaneId(2))),
    };
    let on_input = |_pid: u32, _data: &[u8]| {};
    assert!(host.apply_layout(1, &layout, &on_input));
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

    // 两个 tab 的完整 GTK 根必须同时常驻；切换只改 Stack visible child，
    // 不能 unparent/reparent VTE 后把已显示内容弄空。
    host.pane(1)
        .expect("tab 1 pane")
        .feed_output(b"TAB_ONE_SURFACE\r\n");
    host.flush_all_feeds();
    pump_main_loop(40);
    assert!(host
        .pane(1)
        .expect("tab 1 pane")
        .visible_text()
        .contains("TAB_ONE_SURFACE"));
    let tab2 = LayoutNode::Leaf(PaneId(3));
    assert!(host.apply_layout(2, &tab2, &on_input));
    pump_main_loop(40);
    host.pane(3)
        .expect("tab 2 pane")
        .feed_output(b"TAB_TWO_SURFACE\r\n");
    host.flush_all_feeds();
    pump_main_loop(40);
    // Surface 回归：镜像 pane 已有内容后发生后端网格变化，再切 tab，
    // 不能清空 VTE 并依赖偶然的下一轮事件把内容补回来。
    host.pane(1).expect("tab 1 pane").ensure_grid_size(100, 30);
    assert!(!host.apply_layout(1, &layout, &on_input));
    pump_main_loop(40);
    host.flush_all_feeds();
    assert!(
        host.pane(1)
            .expect("tab 1 pane")
            .visible_text()
            .contains("TAB_ONE_SURFACE"),
        "切回 tab 1 后 VTE 内容必须保留"
    );
    host.reset(false);
    while let Some(child) = host.root_box.first_child() {
        host.root_box.remove(&child);
    }
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
    // 唯一 status bar：中区 tab 按钮（muxterm-status-tab-*），没有第二条 TabBar。
    let tab_buttons = count_widget_names(&root, "muxterm-status-tab-");
    assert_eq!(tab_buttons, 2, "status bar 中区应显示 2 个 tab");
    assert_eq!(count_css_class(&root, "tab-bar"), 0, "不应有第二条 tab-bar");
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
            assert_target_config_herdr_card_saves();
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
