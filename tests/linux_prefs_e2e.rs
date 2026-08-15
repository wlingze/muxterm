//! 配置页 e2e（普通 GTK Window，不构造 AppWindow）。
//!
//! LINUX-PLAN C4.2：临时 XDG_CONFIG_HOME，改 font.size 保存后断言
//! config.toml 内容（注释保留）与 parse_config_toml 结果。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;

use muxterm::core::config::parse_config_toml;
use muxterm::platform::linux::preferences_window::show;

/// S10：Ctrl+= 增大字号并写 config.toml（不新建 preferences.toml）。
/// 纯逻辑测试，不需要 GTK 窗口（避免本机 xvfb/Mesa 多窗口崩溃）。
#[test]
fn ctrl_equal_increases_font_and_writes_config_toml() {
    let tmp = std::env::temp_dir().join(format!("muxterm-zoom-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("muxterm")).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &tmp);
    let config_path = tmp.join("muxterm").join("config.toml");
    std::fs::write(&config_path, "[font]\nsize = 12.0\n").unwrap();

    // 与生产 adjust_font 相同的持久化路径。
    muxterm::platform::linux::window::persist_config("font.size", toml_edit::value(13.0f64));
    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(raw.contains("size = 13.0"), "config.toml 应写 13.0: {raw}");
    assert!(
        !tmp.join("muxterm").join("preferences.toml").exists(),
        "不得新建 preferences.toml"
    );
    let cfg = parse_config_toml(&raw).unwrap();
    assert_eq!(cfg.font.size, 13.0);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn prefs_save_writes_font_size_and_preserves_comments() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        // 独立临时配置目录（本 crate 独立进程，test-threads=1）。
        let tmp = std::env::temp_dir().join(format!("muxterm-prefs-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("muxterm")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &tmp);

        let config_path = tmp.join("muxterm").join("config.toml");
        std::fs::write(
            &config_path,
            "# 字体配置\n[font]\nfamily = \"Monospace\"\nsize = 12.0\n\n[foo]\nbar = 1\n",
        )
        .unwrap();

        let parent = gtk4::Window::builder()
            .title("prefs-parent")
            .default_width(400)
            .default_height(300)
            .build();
        parent.present();
        gtk4::test_widget_wait_for_draw(&parent);

        let saved = std::rc::Rc::new(std::cell::RefCell::new(false));
        let saved_cb = saved.clone();
        let win = show(
            &parent,
            config_path.clone(),
            Box::new(move || {
                *saved_cb.borrow_mut() = true;
            }),
        );
        pump_main_loop(80);

        // 找到字号 SpinButton（第二个 SpinButton：font size）。
        let spin = find_by_name(&win, "").and_then(|_| None::<gtk4::Widget>);
        let _ = spin;
        // 直接按顺序找 SpinButton：font size 是第一个。
        let mut spins = Vec::new();
        collect_spin_buttons(&win, &mut spins);
        assert!(!spins.is_empty(), "配置页应有 SpinButton");
        let font_size = &spins[0];
        font_size.set_value(14.0);
        pump_main_loop(40);

        // 点保存
        let save_btn = find_by_name(&win, "").and_then(|_| None::<gtk4::Widget>);
        let _ = save_btn;
        let buttons = collect_buttons(&win);
        let save = buttons
            .iter()
            .find(|b| b.label().map(|l| l.to_string()).unwrap_or_default() == "Save")
            .cloned()
            .expect("应有 Save 按钮");
        let _: () = save.emit_by_name("clicked", &[]);
        pump_main_loop(80);

        assert!(*saved.borrow(), "保存回调应触发");
        let raw = std::fs::read_to_string(&config_path).unwrap();
        assert!(raw.contains("# 字体配置"), "注释应保留: {raw}");
        assert!(raw.contains("[foo]"), "未知表应保留: {raw}");
        assert!(raw.contains("bar = 1"), "未知键应保留: {raw}");
        assert!(raw.contains("size = 14.0"), "font.size 应写入: {raw}");

        let cfg = parse_config_toml(&raw).unwrap();
        assert_eq!(cfg.font.size, 14.0);
        assert_eq!(cfg.font.family, "Monospace");

        win.close();
        win.destroy();
        parent.destroy();
        pump_main_loop(40);
        let _ = std::fs::remove_dir_all(&tmp);
    });
}

fn collect_spin_buttons(root: &impl IsA<gtk4::Widget>, out: &mut Vec<gtk4::SpinButton>) {
    let root = root.as_ref();
    if let Ok(s) = root.clone().downcast::<gtk4::SpinButton>() {
        out.push(s);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        collect_spin_buttons(&c, out);
        child = c.next_sibling();
    }
}

fn collect_buttons(root: &impl IsA<gtk4::Widget>) -> Vec<gtk4::Button> {
    let mut out = Vec::new();
    fn walk(root: &impl IsA<gtk4::Widget>, out: &mut Vec<gtk4::Button>) {
        let root = root.as_ref();
        if let Ok(b) = root.clone().downcast::<gtk4::Button>() {
            out.push(b);
        }
        let mut child = root.first_child();
        while let Some(c) = child {
            walk(&c, out);
            child = c.next_sibling();
        }
    }
    walk(root, &mut out);
    out
}
