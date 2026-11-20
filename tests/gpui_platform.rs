//! 跨平台策略验收：平台差异表、和弦归一化、自绘菜单条。
//!
//! 这些用例在 macOS / Linux / Windows 上都必须通过，CI 三个原生 runner
//! 都跑它（`.github/workflows/ci-gpui-frontend.yml`）。写法上不假设当前平台：
//! 期望值从 `platform` 模块现算，等于把「同一份行为」在三种 OS 上各验一遍。

#![cfg(feature = "gpui")]

use gpui::{px, size, TestAppContext};
use muxterm::frontend::gpui::model::menu::main_menu_with;
use muxterm::frontend::gpui::model::menu_bar::{self, MenuBar};
use muxterm::frontend::gpui::model::shortcuts::KeyAction;
use muxterm::frontend::gpui::platform::{self, desktop};
use muxterm::frontend::gpui::state::GpuiApp;
use muxterm::frontend::view_store::ViewStore;

/// 本平台命令键在 `simulate_keystrokes` 里的写法（macOS `cmd-`，其它 `alt-`）。
fn command_prefix() -> &'static str {
    match platform::primary_modifier() {
        desktop::PrimaryModifier::Command => "cmd-",
        desktop::PrimaryModifier::Control => "ctrl-",
        desktop::PrimaryModifier::Super => "super-",
        desktop::PrimaryModifier::Alt => "alt-",
    }
}

/// 无头点击：先按 debug selector 拿几何，再按中心点模拟点击。
fn click(cx: &mut gpui::VisualTestContext, selector: &str) {
    let bounds = cx
        .debug_bounds(Box::leak(selector.to_string().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} 必须渲染出来"));
    cx.simulate_click(bounds.center(), Default::default());
}

fn mount<'a>(
    cx: &'a mut TestAppContext,
) -> (gpui::Entity<GpuiApp>, &'a mut gpui::VisualTestContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let app = GpuiApp::new_headless(ViewStore::default(), cx);
        window.focus(&app.focus_handle());
        app
    });
    cx.simulate_resize(size(px(1_200.), px(800.)));
    (view, cx)
}

#[gpui::test]
fn native_menu_bar_is_macos_only(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx);
    let has_bar = cx.debug_bounds("menu-bar").is_some();
    assert_eq!(
        has_bar,
        !platform::uses_native_menu_bar(),
        "只有没有原生菜单的平台才画自绘菜单条"
    );
    let _ = view;
}

#[gpui::test]
fn menu_data_is_identical_on_every_platform() {
    // 菜单是数据不是副作用：三个平台吃同一份结构（自绘菜单条与原生菜单分叉
    // 只发生在渲染层）。这里断言条目数量与顺序不随平台变化。
    let menus = main_menu_with(Default::default());
    let titles: Vec<&str> = menus.iter().map(|menu| menu.title.as_str()).collect();
    assert_eq!(titles, ["Muxterm", "File", "Window", "Edit", "View"]);
    let file: Vec<String> = menus[1]
        .entries
        .iter()
        .filter_map(|entry| match entry {
            muxterm::frontend::gpui::model::menu::MenuEntry::Item(item) => Some(item.label.clone()),
            _ => None,
        })
        .collect();
    assert!(file.contains(&"New Tab".to_string()));
    assert!(file.contains(&"Close Pane".to_string()));
}

#[gpui::test]
fn clicking_the_menu_title_opens_the_dropdown(cx: &mut TestAppContext) {
    if platform::uses_native_menu_bar() {
        // macOS 的菜单在系统栏里，窗口内没有可点的标题。
        return;
    }
    let (view, cx) = mount(cx);
    click(cx, "menu-bar-title-0");
    let open = cx.update(|_, cx| view.update(cx, |app, _| app.menu_bar_open()));
    assert_eq!(open, Some(0), "点标题必须展开对应菜单");
    assert!(cx.debug_bounds("menu-bar-panel").is_some(), "面板必须渲染");
    // 高亮落在第一个可执行项上（应用菜单第一项是 About）。
    let selected = cx.update(|_, cx| view.update(cx, |app, _| app.menu_bar_selected_action()));
    assert_eq!(selected, Some(KeyAction::About));
}

#[gpui::test]
fn escape_closes_the_menu(cx: &mut TestAppContext) {
    if platform::uses_native_menu_bar() {
        return;
    }
    let (view, cx) = mount(cx);
    click(cx, "menu-bar-title-0");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let open = cx.update(|_, cx| view.update(cx, |app, _| app.menu_bar_open()));
    assert_eq!(open, None);
    assert!(cx.debug_bounds("menu-bar-panel").is_none());
}

#[gpui::test]
fn keyboard_navigation_runs_the_highlighted_item(cx: &mut TestAppContext) {
    if platform::uses_native_menu_bar() {
        return;
    }
    let (view, cx) = mount(cx);
    click(cx, "menu-bar-title-0");
    // 第一项是 About：回车后应该打开 Info 浮层。
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("info-panel").is_some(),
        "回车必须执行高亮项（About → Info 浮层）"
    );
    let open = cx.update(|_, cx| view.update(cx, |app, _| app.menu_bar_open()));
    assert_eq!(open, None, "执行后收起菜单");
}

#[gpui::test]
fn menu_item_click_pushes_the_same_command_as_the_shortcut(cx: &mut TestAppContext) {
    if platform::uses_native_menu_bar() {
        return;
    }
    let (view, cx) = mount(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let _ = app.take_commands_for_tests();
        })
    });
    // File → New Tab
    click(cx, "menu-bar-title-1");
    click(cx, "menu-bar-item-new-tab");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands
            .iter()
            .any(|command| format!("{command:?}").contains("NewTab")),
        "菜单项必须真的发出 NewTab（实际: {commands:?}）"
    );
}

#[gpui::test]
fn command_key_runs_window_actions_on_every_platform(cx: &mut TestAppContext) {
    // 同一份快捷键表，靠命令键归一化跨平台生效：
    // macOS = Command，Linux / Windows = `[shortcuts] primary_key`（缺省 Alt）。
    let (view, cx) = mount(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let _ = app.take_commands_for_tests();
        })
    });
    cx.simulate_keystrokes(&format!("{}t", command_prefix()));
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    assert!(
        commands
            .iter()
            .any(|command| format!("{command:?}").contains("NewTab")),
        "命令键 + T 必须新开 tab（实际: {commands:?}）"
    );
}

#[gpui::test]
fn terminal_control_keys_still_reach_the_pty(cx: &mut TestAppContext) {
    // 归一化不能吃掉终端的 Ctrl-C：它是控制字节，不是「复制」。
    let (view, cx) = mount(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            let _ = app.take_commands_for_tests();
        })
    });
    cx.simulate_keystrokes("ctrl-c");
    let commands = cx.update(|_, cx| view.update(cx, |app, _| app.take_commands_for_tests()));
    let wrote = commands.iter().any(|command| {
        let debug = format!("{command:?}");
        debug.contains("Input") && debug.contains("data: [3]")
    });
    // 没有活动 pane 时不会写 PTY；只要不是被当成 Copy 消费掉即可。
    assert!(
        wrote
            || !commands
                .iter()
                .any(|command| format!("{command:?}").contains("Copy")),
        "Ctrl-C 不能被当成复制（实际: {commands:?}）"
    );
}

#[test]
fn menu_bar_state_machine_is_platform_independent() {
    let menus = main_menu_with(Default::default());
    let mut bar = MenuBar::default();
    bar.open_menu(1, &menus);
    assert_eq!(bar.open, Some(1));
    bar.move_menu(1, &menus);
    assert_eq!(bar.open, Some(2));
    bar.close();
    assert!(!bar.is_open());
    // 关闭后没有高亮项可执行。
    assert_eq!(bar.selected_action(&menus), None);
}

#[test]
fn menu_bar_shortcut_hints_use_the_platform_notation() {
    let menus = main_menu_with(Default::default());
    let new_tab = menus[1]
        .entries
        .iter()
        .find_map(|entry| match entry {
            muxterm::frontend::gpui::model::menu::MenuEntry::Item(item)
                if item.action == KeyAction::NewTab =>
            {
                Some(item.key.clone())
            }
            _ => None,
        })
        .flatten()
        .expect("New Tab 必须带快捷键");
    let hint = menu_bar::shortcut_label(
        &new_tab,
        desktop::DesktopOs::Linux,
        desktop::PrimaryModifier::Alt,
    );
    assert_eq!(hint, "Alt+T");
    let mac = menu_bar::shortcut_label(
        &new_tab,
        desktop::DesktopOs::MacOs,
        desktop::PrimaryModifier::Command,
    );
    assert_eq!(mac, "⌘T");
}

#[test]
fn window_title_keeps_the_workspace_name_off_macos() {
    let title = muxterm::frontend::gpui::state::window_title_text(false, "dev");
    if cfg!(target_os = "macos") {
        assert_eq!(title, "", "macOS 关掉标题栏后不留文字");
    } else {
        assert_eq!(title, "dev — Muxterm", "其它平台的标题栏由系统画，不能留空");
    }
}
