//! Schema-backed Linux settings window.
//!
//! The GTK layer owns layout and native controls only. Values are read from a
//! Core `SettingsService` snapshot and written through the same transactional
//! JSON-Patch API used by FFI and CLI. This keeps the window from growing a
//! second TOML parser or a second set of defaults.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, ComboBoxText, Entry, Label, ListBox, ListBoxRow, Orientation,
    ScrolledWindow, SearchEntry, SpinButton, TextView, Window,
};
use serde_json::Value;

use crate::core::config_service::{JsonPatchOperation, SettingsService};
use crate::platform::i18n::{self, Key as TextKey};

/// 打开配置页；`on_saved` 在保存或文件变化后调用（热加载）。
pub fn show(
    parent: &impl IsA<Window>,
    config_path: PathBuf,
    on_saved: Box<dyn Fn() + 'static>,
) -> Window {
    let win = Window::builder()
        .title(i18n::tr(TextKey::CmdPreferences))
        .default_width(680)
        .default_height(760)
        .modal(true)
        .transient_for(parent)
        .build();
    win.set_widget_name("muxterm-prefs-window");

    let service = SettingsService::open(&config_path).unwrap_or_else(|error| {
        tracing::warn!(
            target = "muxterm::config",
            "设置窗口使用内存默认值: {error}"
        );
        SettingsService::in_memory_default(config_path.clone())
    });
    let cfg = service.document().config.clone();

    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let search = SearchEntry::new();
    search.set_widget_name("muxterm-prefs-search");
    search.set_placeholder_text(Some("Search settings"));
    root.append(&search);

    let appearance = section("Appearance");
    let theme = ComboBoxText::new();
    theme.set_widget_name("muxterm-prefs-theme");
    theme.append(Some("system"), "System");
    theme.append(Some("black"), "Black");
    theme.append(Some("white"), "White");
    // Keep legacy IDs selectable for an old config until the next Apply.
    theme.append(Some("dark"), "Dark (legacy)");
    theme.append(Some("light"), "Light (legacy)");
    theme.set_active_id(Some(&cfg.theme.name.to_ascii_lowercase()));
    appearance.append(&field_row("Theme", &theme));

    let font_family = Entry::new();
    font_family.set_widget_name("muxterm-prefs-font-family");
    font_family.set_text(&cfg.font.family);
    appearance.append(&field_row("Font family", &font_family));

    let font_fallback = Entry::new();
    font_fallback.set_widget_name("muxterm-prefs-font-fallback");
    font_fallback.set_placeholder_text(Some("Noto Sans Mono, monospace"));
    font_fallback.set_text(&cfg.font.fallback.join(", "));
    appearance.append(&field_row("Fallback families", &font_fallback));

    let font_size = SpinButton::with_range(9.0, 72.0, 0.5);
    font_size.set_widget_name("muxterm-prefs-font-size");
    font_size.set_value(f64::from(cfg.font.size));
    appearance.append(&field_row("Font size", &font_size));
    root.append(&appearance);

    let runtime = section("Runtime");
    let status_mode = ComboBoxText::new();
    status_mode.set_widget_name("muxterm-prefs-status-mode");
    status_mode.append(Some("tmux"), "tmux");
    status_mode.append(Some("theme"), "theme");
    status_mode.set_active_id(Some(&cfg.statusbar.mode));
    runtime.append(&field_row("Status bar mode", &status_mode));

    let scrollback_lines = SpinButton::with_range(100.0, 1_000_000.0, 100.0);
    scrollback_lines.set_widget_name("muxterm-prefs-scrollback");
    scrollback_lines.set_value(f64::from(cfg.scrollback.lines));
    runtime.append(&field_row("Scrollback lines", &scrollback_lines));

    let auto_mouse = gtk4::Switch::new();
    auto_mouse.set_widget_name("muxterm-prefs-auto-mouse");
    auto_mouse.set_active(cfg.tmux.auto_mouse);
    runtime.append(&field_row("tmux mouse mode", &auto_mouse));

    let default_session = Entry::new();
    default_session.set_widget_name("muxterm-prefs-default-session");
    default_session.set_text(&cfg.tmux.default_session);
    runtime.append(&field_row("Default session", &default_session));

    let socket = Entry::new();
    socket.set_widget_name("muxterm-prefs-socket");
    socket.set_text(&cfg.tmux.socket);
    runtime.append(&field_row("tmux socket", &socket));
    root.append(&runtime);

    let attention = section("Attention");
    let debounce_ms = SpinButton::with_range(0.0, 10_000.0, 10.0);
    debounce_ms.set_widget_name("muxterm-prefs-attention-debounce");
    debounce_ms.set_value(cfg.attention.debounce_ms as f64);
    attention.append(&field_row("Debounce (ms)", &debounce_ms));
    let regex_view = TextView::new();
    regex_view.set_widget_name("muxterm-prefs-blocked-regex");
    regex_view.set_wrap_mode(gtk4::WrapMode::Word);
    regex_view.set_size_request(-1, 84);
    regex_view
        .buffer()
        .set_text(&cfg.attention.blocked_regex.join("\n"));
    attention.append(&field_row("Blocked regex (one per line)", &regex_view));
    root.append(&attention);

    let projects = section("Projects");
    projects.append(&Label::new(Some(&format!(
        "{} project(s) — edit with the Project editor or `muxterm config project`",
        service.document().projects.len()
    ))));
    projects
        .last_child()
        .unwrap()
        .set_widget_name("muxterm-prefs-project-summary");
    root.append(&projects);

    let shortcuts = section("Shortcuts");
    let shortcut_search = SearchEntry::new();
    shortcut_search.set_widget_name("muxterm-prefs-shortcut-search");
    shortcut_search.set_placeholder_text(Some("Filter actions"));
    shortcuts.append(&shortcut_search);
    let keys = ListBox::new();
    keys.set_widget_name("muxterm-prefs-shortcuts");
    keys.add_css_class("prefs-keys");
    for item in &service.document().shortcuts.overrides {
        let row = ListBoxRow::new();
        row.set_widget_name("muxterm-prefs-shortcut-row");
        let bindings = item
            .bindings
            .iter()
            .map(|binding| {
                let mut modifiers = binding.modifiers.clone();
                modifiers.push(binding.key.clone());
                modifiers.join("+")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let label = Label::new(Some(&format!("{}  →  {}", item.action, bindings)));
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
    keys_sw.set_size_request(-1, 150);
    shortcuts.append(&keys_sw);
    root.append(&shortcuts);

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let cancel = gtk4::Button::with_label("Cancel");
    cancel.set_widget_name("muxterm-prefs-cancel");
    let save = gtk4::Button::with_label(&i18n::tr(TextKey::Save));
    save.set_widget_name("muxterm-prefs-save");
    actions.append(&cancel);
    actions.append(&save);
    root.append(&actions);
    win.set_child(Some(&root));

    let on_saved = Rc::new(on_saved);
    let dirty = Rc::new(RefCell::new(false));
    for widget in [
        font_family.clone().upcast::<gtk4::Widget>(),
        font_fallback.clone().upcast::<gtk4::Widget>(),
        font_size.clone().upcast::<gtk4::Widget>(),
        theme.clone().upcast::<gtk4::Widget>(),
        status_mode.clone().upcast::<gtk4::Widget>(),
        scrollback_lines.clone().upcast::<gtk4::Widget>(),
        auto_mouse.clone().upcast::<gtk4::Widget>(),
        default_session.clone().upcast::<gtk4::Widget>(),
        socket.clone().upcast::<gtk4::Widget>(),
        debounce_ms.clone().upcast::<gtk4::Widget>(),
        regex_view.clone().upcast::<gtk4::Widget>(),
    ] {
        let dirty = dirty.clone();
        widget.connect_notify_local(Some("sensitive"), move |_, _| {
            *dirty.borrow_mut() = true;
        });
    }

    cancel.connect_clicked({
        let win = win.clone();
        move |_| win.close()
    });

    save.connect_clicked({
        let win = win.clone();
        let on_saved = on_saved.clone();
        let config_path = config_path.clone();
        let theme = theme.clone();
        let font_family = font_family.clone();
        let font_fallback = font_fallback.clone();
        let font_size = font_size.clone();
        let status_mode = status_mode.clone();
        let scrollback_lines = scrollback_lines.clone();
        let auto_mouse = auto_mouse.clone();
        let default_session = default_session.clone();
        let socket = socket.clone();
        let debounce_ms = debounce_ms.clone();
        let regex_view = regex_view.clone();
        move |_| {
            let mut service = match SettingsService::open(&config_path) {
                Ok(service) => service,
                Err(error) => {
                    tracing::error!(target = "muxterm::config", "打开配置事务失败: {error}");
                    return;
                }
            };
            let transaction = service.begin();
            let theme_id = theme
                .active_id()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "system".into());
            let fallback = font_fallback
                .text()
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let regexes = {
                let buffer = regex_view.buffer();
                buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .lines()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            };
            let operations = vec![
                replace("/theme/name", Value::String(theme_id)),
                replace(
                    "/font/family",
                    Value::String(font_family.text().to_string()),
                ),
                replace(
                    "/font/fallback",
                    serde_json::to_value(fallback).unwrap_or(Value::Array(Vec::new())),
                ),
                replace("/font/size", Value::from(font_size.value())),
                replace(
                    "/statusbar/mode",
                    Value::String(
                        status_mode
                            .active_id()
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "tmux".into()),
                    ),
                ),
                replace(
                    "/scrollback/lines",
                    Value::from(scrollback_lines.value() as u64),
                ),
                replace("/tmux/auto_mouse", Value::Bool(auto_mouse.is_active())),
                replace(
                    "/tmux/default_session",
                    Value::String(default_session.text().to_string()),
                ),
                replace("/tmux/socket", Value::String(socket.text().to_string())),
                replace(
                    "/attention/debounce_ms",
                    Value::from(debounce_ms.value() as u64),
                ),
                replace(
                    "/attention/blocked_regex",
                    serde_json::to_value(regexes).unwrap_or(Value::Array(Vec::new())),
                ),
            ];
            if let Err(error) = service
                .patch(&transaction, &operations)
                .and_then(|_| service.commit(&transaction).map(|_| ()))
            {
                tracing::error!(target = "muxterm::config", "保存设置失败: {error}");
                let _ = service.cancel(&transaction);
                return;
            }
            on_saved();
            win.close();
        }
    });

    // 外部编辑只触发平台回调；下一次打开窗口会拿到新的 Schema snapshot。
    if let Ok(monitor) = gtk4::gio::File::for_path(&config_path).monitor_file(
        gtk4::gio::FileMonitorFlags::NONE,
        gtk4::gio::Cancellable::NONE,
    ) {
        let on_saved = on_saved.clone();
        monitor.connect_changed(move |_, _, _, _| on_saved());
    }

    win.present();
    win
}

fn replace(path: &str, value: Value) -> JsonPatchOperation {
    JsonPatchOperation {
        op: "replace".into(),
        path: path.into(),
        value: Some(value),
    }
}

fn section(title: &str) -> GtkBox {
    let box_ = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .build();
    let label = Label::new(Some(title));
    label.set_halign(Align::Start);
    label.add_css_class("heading");
    box_.append(&label);
    box_
}

fn field_row(label: &str, widget: &impl IsA<gtk4::Widget>) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    let label = Label::new(Some(label));
    label.set_halign(Align::Start);
    label.set_hexpand(true);
    row.append(&label);
    row.append(widget);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_uses_rfc6902_operation() {
        let operation = replace("/font/size", Value::from(13.0));
        assert_eq!(operation.op, "replace");
        assert_eq!(operation.path, "/font/size");
    }

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
