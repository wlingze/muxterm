//! 配置页（LINUX-PLAN §12 C4.2）：读写 `config.toml`（唯一事实源）。
//!
//! 普通 `gtk4::Window`（name=`muxterm-prefs-window`），不构造 AppWindow。
//! 保存用 `toml_edit` 保注释与未知键；`scrollback.lines` 只影响之后新建的
//! TerminalState（窗口内写明）。

use std::path::PathBuf;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, ComboBoxText, Entry, Label, ListBox, ListBoxRow, Orientation,
    ScrolledWindow, SpinButton, TextView, Window,
};

use crate::core::config::Config;
use crate::core::config_edit::set_dotted_key;
use crate::platform::i18n::{self, Key as TextKey};

/// 打开配置页；`on_saved` 在保存或文件变化后调用（热加载）。
pub fn show(
    parent: &impl IsA<Window>,
    config_path: PathBuf,
    on_saved: Box<dyn Fn() + 'static>,
) -> Window {
    let win = Window::builder()
        .title(i18n::tr(TextKey::CmdPreferences))
        .default_width(560)
        .default_height(640)
        .modal(true)
        .transient_for(parent)
        .build();
    win.set_widget_name("muxterm-prefs-window");

    let cfg = Config::load().unwrap_or_default();
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    // 主题
    let theme = ComboBoxText::new();
    theme.append(Some("light"), "light");
    theme.append(Some("dark"), "dark");
    theme.set_active_id(Some(&cfg.theme.name.to_ascii_lowercase()));

    // 字体
    let font_family = Entry::new();
    font_family.set_text(&cfg.font.family);
    let font_size = SpinButton::with_range(1.0, 72.0, 0.5);
    font_size.set_value(f64::from(cfg.font.size));

    // 状态栏模式
    let status_mode = ComboBoxText::new();
    status_mode.append(Some("tmux"), "tmux");
    status_mode.append(Some("theme"), "theme");
    status_mode.set_active_id(Some(&cfg.statusbar.mode));

    // scrollback
    let scrollback_lines = SpinButton::with_range(100.0, 1_000_000.0, 100.0);
    scrollback_lines.set_value(f64::from(cfg.scrollback.lines));

    // attention
    let debounce_ms = SpinButton::with_range(1.0, 10_000.0, 10.0);
    debounce_ms.set_value(cfg.attention.debounce_ms as f64);
    let regex_view = TextView::new();
    regex_view.set_wrap_mode(gtk4::WrapMode::Word);
    regex_view.set_size_request(-1, 80);
    regex_view
        .buffer()
        .set_text(&cfg.attention.blocked_regex.join("\n"));

    // 键位只读列表
    let keys = ListBox::new();
    keys.add_css_class("prefs-keys");
    for kb in &cfg.keybindings {
        let row = ListBoxRow::new();
        let label = Label::new(Some(&format!(
            "{} + {} → {}",
            kb.mods.join("+"),
            kb.key,
            kb.action
        )));
        label.set_halign(Align::Start);
        row.set_child(Some(&label));
        keys.append(&row);
    }
    let keys_sw = ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&keys)
        .build();
    keys_sw.set_size_request(-1, 140);

    root.append(&field_row("Theme", &theme));
    root.append(&field_row("Font family", &font_family));
    root.append(&field_row("Font size", &font_size));
    root.append(&field_row("Status bar mode", &status_mode));
    root.append(&field_row("Scrollback lines", &scrollback_lines));
    root.append(&field_row("Attention debounce (ms)", &debounce_ms));
    root.append(&field_row("Blocked regex (one per line)", &regex_view));
    root.append(&field_row("Keybindings (read-only)", &keys_sw));

    let save = gtk4::Button::with_label(&i18n::tr(TextKey::Save));
    save.set_halign(Align::End);
    root.append(&save);

    win.set_child(Some(&root));

    let on_saved = std::rc::Rc::new(on_saved);
    save.connect_clicked({
        let on_saved = on_saved.clone();
        let win = win.clone();
        let config_path = config_path.clone();
        let theme = theme.clone();
        let font_family = font_family.clone();
        let font_size = font_size.clone();
        let status_mode = status_mode.clone();
        let scrollback_lines = scrollback_lines.clone();
        let debounce_ms = debounce_ms.clone();
        let regex_view = regex_view.clone();
        move |_| {
            let raw = std::fs::read_to_string(&config_path).unwrap_or_default();
            let mut out = raw;
            let mut apply = |dotted: &str, value: toml_edit::Item| {
                if let Ok(next) = set_dotted_key(&out, dotted, value) {
                    out = next;
                }
            };
            let theme_id = theme
                .active_id()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "light".into());
            apply("theme.name", toml_edit::value(theme_id));
            apply(
                "font.family",
                toml_edit::value(font_family.text().to_string()),
            );
            apply("font.size", toml_edit::value(font_size.value()));
            let mode_id = status_mode
                .active_id()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "tmux".into());
            apply("statusbar.mode", toml_edit::value(mode_id));
            apply(
                "scrollback.lines",
                toml_edit::value(scrollback_lines.value() as i64),
            );
            apply(
                "attention.debounce_ms",
                toml_edit::value(debounce_ms.value() as i64),
            );
            let buf = regex_view.buffer();
            let regexes: Vec<String> = buf
                .text(&buf.start_iter(), &buf.end_iter(), false)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            apply(
                "attention.blocked_regex",
                toml_edit::value(toml_edit::Array::from_iter(regexes)),
            );
            if let Some(dir) = config_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&config_path, out).is_ok() {
                on_saved();
                win.close();
            }
        }
    });

    // 热加载：文件变化（含外部编辑）→ on_saved。
    if let Ok(monitor) = gtk4::gio::File::for_path(&config_path).monitor_file(
        gtk4::gio::FileMonitorFlags::NONE,
        gtk4::gio::Cancellable::NONE,
    ) {
        monitor.connect_changed(move |_, _, _, _| {
            on_saved();
        });
    }

    win.present();
    win
}

fn field_row(label: &str, widget: &impl IsA<gtk4::Widget>) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    let l = Label::new(Some(label));
    l.set_halign(Align::Start);
    l.set_hexpand(true);
    row.append(&l);
    row.append(widget);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_window_is_named() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("skip: 无 DISPLAY");
            return;
        }
        gtk4::test_synced(|| {
            let parent = gtk4::Window::builder().build();
            let win = show(
                &parent,
                PathBuf::from("/tmp/nonexistent-config.toml"),
                Box::new(|| {}),
            );
            assert_eq!(win.widget_name(), "muxterm-prefs-window");
            win.close();
            win.destroy();
            parent.destroy();
        });
    }
}
