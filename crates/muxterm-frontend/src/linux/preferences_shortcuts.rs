//! Preferences 中快捷键管理页及按键捕获窗口。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, ListBox, Orientation, ScrolledWindow, Window};
use serde_json::Value;

use crate::linux::settings_model::ConfigApi;

use super::{humanize_words, install_preferences_css, replace, subwindow_header};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ShortcutBindingDto {
    key: String,
    #[serde(default)]
    modifiers: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ShortcutOverrideDto {
    action: String,
    #[serde(default)]
    bindings: Vec<ShortcutBindingDto>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ActionDescriptorDto {
    id: String,
    title_key: String,
}

pub(super) fn show_shortcut_manager(
    app: &impl IsA<Window>,
    config_path: PathBuf,
    config: ConfigApi,
    on_changed: Rc<Box<dyn Fn() + 'static>>,
) {
    install_preferences_css();
    let win = Window::builder()
        .title("Shortcuts")
        .transient_for(app)
        .modal(true)
        .default_width(720)
        .default_height(560)
        .build();
    win.set_widget_name("muxterm-shortcuts-window");
    win.add_css_class("muxterm-preferences-window");
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    root.add_css_class("prefs-subwindow");
    root.append(&subwindow_header(
        "Shortcuts",
        "Customize actions without touching GTK key codes.",
    ));

    let list = ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("prefs-list-card");
    let sw = ScrolledWindow::builder()
        .min_content_height(360)
        .child(&list)
        .build();
    sw.add_css_class("prefs-subwindow-scroll");
    root.append(&sw);

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let close = Button::with_label("Close");
    close.add_css_class("prefs-secondary-action");
    actions.append(&close);
    root.append(&actions);
    win.set_child(Some(&root));

    let refresh = {
        let list = list.clone();
        let config = config.clone();
        let win_for_rows = win.clone();
        let on_changed_for_rows = on_changed.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let snapshot = match config.describe() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(target = "muxterm::config", "读取快捷键配置失败: {error}");
                    return;
                }
            };
            let shortcuts: Vec<ShortcutOverrideDto> =
                serde_json::from_value(snapshot.values["shortcuts"]["overrides"].clone())
                    .unwrap_or_default();
            let catalog: Vec<ActionDescriptorDto> =
                serde_json::from_value(snapshot.action_catalog.clone()).unwrap_or_default();
            for action in &catalog {
                let override_item = shortcuts.iter().find(|item| item.action == action.id);
                let row = GtkBox::builder()
                    .orientation(Orientation::Horizontal)
                    .spacing(8)
                    .margin_top(8)
                    .margin_bottom(8)
                    .margin_start(12)
                    .margin_end(12)
                    .build();
                row.add_css_class("prefs-shortcut-row");
                let label = Label::new(Some(&action_title(&action.title_key)));
                label.set_hexpand(true);
                label.set_halign(Align::Start);
                label.add_css_class("prefs-shortcut-name");
                row.append(&label);
                let summary = Label::new(Some(
                    &override_item
                        .map(|item| {
                            if item.bindings.is_empty() {
                                "disabled".into()
                            } else {
                                item.bindings
                                    .iter()
                                    .map(shortcut_binding_label)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        })
                        .unwrap_or_else(|| "default".into()),
                ));
                summary.set_halign(Align::Start);
                summary.set_hexpand(true);
                summary.add_css_class("prefs-keycap");
                row.append(&summary);

                let bind = Button::with_label("Bind…");
                bind.add_css_class("prefs-inline-action");
                let unbind = Button::with_label("Unbind");
                unbind.add_css_class("prefs-inline-action");
                let action_id = action.id.clone();
                let config_for_bind = config.clone();
                let win_for_capture = win_for_rows.clone();
                let on_changed = on_changed_for_rows.clone();
                bind.connect_clicked(move |_| {
                    let action_id = action_id.clone();
                    capture_shortcut(&win_for_capture, {
                        let config = config_for_bind.clone();
                        let on_changed = on_changed.clone();
                        move |key, modifiers| {
                            let snapshot = match config.describe() {
                                Ok(snapshot) => snapshot,
                                Err(error) => {
                                    tracing::error!(
                                        target = "muxterm::config",
                                        "读取快捷键配置失败: {error}"
                                    );
                                    return;
                                }
                            };
                            let mut overrides: Vec<ShortcutOverrideDto> = serde_json::from_value(
                                snapshot.values["shortcuts"]["overrides"].clone(),
                            )
                            .unwrap_or_default();
                            overrides.retain(|item| item.action != action_id);
                            overrides.push(ShortcutOverrideDto {
                                action: action_id.clone(),
                                bindings: vec![ShortcutBindingDto { key, modifiers }],
                            });
                            let value =
                                serde_json::to_value(overrides).unwrap_or(Value::Array(Vec::new()));
                            if config
                                .apply(&[replace("/shortcuts/overrides", value)])
                                .is_ok()
                            {
                                on_changed();
                            }
                        }
                    });
                });
                let action_id = action.id.clone();
                let config_for_unbind = config.clone();
                let on_changed = on_changed_for_rows.clone();
                unbind.connect_clicked(move |_| {
                    let snapshot = match config_for_unbind.describe() {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            tracing::error!(
                                target = "muxterm::config",
                                "读取快捷键配置失败: {error}"
                            );
                            return;
                        }
                    };
                    let mut overrides: Vec<ShortcutOverrideDto> =
                        serde_json::from_value(snapshot.values["shortcuts"]["overrides"].clone())
                            .unwrap_or_default();
                    overrides.retain(|item| item.action != action_id);
                    let value = serde_json::to_value(overrides).unwrap_or(Value::Array(Vec::new()));
                    if config_for_unbind
                        .apply(&[replace("/shortcuts/overrides", value)])
                        .is_ok()
                    {
                        on_changed();
                    }
                });
                row.append(&bind);
                row.append(&unbind);
                list.append(&row);
            }
        }
    };
    let refresh = Rc::new(RefCell::new(refresh));
    refresh.borrow()();

    // 文件变更（含本窗口的绑定/解绑）后自动重建列表。
    if let Ok(monitor) = gtk4::gio::File::for_path(&config_path).monitor_file(
        gtk4::gio::FileMonitorFlags::NONE,
        gtk4::gio::Cancellable::NONE,
    ) {
        let config = config.clone();
        let refresh = refresh.clone();
        monitor.connect_changed(move |_, _, _, _| {
            if config.reload().is_ok() {
                refresh.borrow()();
            }
        });
    }

    {
        let win = win.clone();
        close.connect_clicked(move |_| win.close());
    }
    win.present();
}

/// 打开按键捕获窗口：下一次非 Escape 按键返回 (key, modifiers)。
fn capture_shortcut(parent: &impl IsA<Window>, on_capture: impl Fn(String, Vec<String>) + 'static) {
    let win = Window::builder()
        .title("Bind shortcut")
        .transient_for(parent)
        .modal(true)
        .default_width(360)
        .default_height(140)
        .build();
    win.set_widget_name("muxterm-shortcut-capture");
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let label = Label::new(Some("Press the key combination…"));
    label.set_halign(Align::Start);
    root.append(&label);
    let cancel = Button::with_label("Cancel");
    cancel.set_halign(Align::End);
    root.append(&cancel);
    win.set_child(Some(&root));

    let finished = Rc::new(Cell::new(false));
    {
        let win = win.clone();
        let finished = finished.clone();
        cancel.connect_clicked(move |_| {
            if !finished.replace(true) {
                win.close();
            }
        });
    }
    {
        let win = win.clone();
        let finished = finished.clone();
        win.connect_close_request(move |_| {
            finished.set(true);
            glib::Propagation::Proceed
        });
    }
    {
        let controller = gtk4::EventControllerKey::new();
        let win_for_keys = win.clone();
        let finished = finished.clone();
        let on_capture = Rc::new(RefCell::new(Some(on_capture)));
        controller.connect_key_pressed(move |_, keyval, _keycode, mods| {
            if !finished.replace(true) {
                if keyval != gdk::Key::Escape {
                    if let Some((key, modifiers)) = gdk_key_to_binding(keyval, mods) {
                        if let Some(callback) = on_capture.borrow_mut().take() {
                            callback(key, modifiers);
                        }
                    }
                }
                win_for_keys.close();
            }
            glib::Propagation::Stop
        });
        win.add_controller(controller);
    }
    win.present();
}

/// GDK 按键 + 修饰键 → (key, modifiers)。大小写/特殊键归一化与 keymap 一致。
fn gdk_key_to_binding(keyval: gdk::Key, mods: gdk::ModifierType) -> Option<(String, Vec<String>)> {
    let mut modifiers = Vec::new();
    if mods.contains(gdk::ModifierType::CONTROL_MASK) {
        modifiers.push("control".to_string());
    }
    if mods.contains(gdk::ModifierType::SHIFT_MASK)
        || keyval.to_unicode().is_some_and(|c| c.is_ascii_uppercase())
    {
        modifiers.push("shift".to_string());
    }
    if mods.contains(gdk::ModifierType::ALT_MASK) {
        modifiers.push("alt".to_string());
    }
    if mods.contains(gdk::ModifierType::SUPER_MASK) {
        modifiers.push("super".to_string());
    }
    let key = match keyval.name() {
        Some(name) => {
            let lower = name.to_ascii_lowercase();
            match lower.as_str() {
                "bracketleft" => "[".to_string(),
                "bracketright" => "]".to_string(),
                _ => match keyval.to_unicode() {
                    Some(c) => c.to_ascii_lowercase().to_string(),
                    None => lower,
                },
            }
        }
        None => keyval.to_unicode()?.to_ascii_lowercase().to_string(),
    };
    if key.is_empty() {
        return None;
    }
    Some((key, modifiers))
}

fn action_title(title_key: &str) -> String {
    let raw = title_key.strip_prefix("action.").unwrap_or(title_key);
    let raw = raw.strip_suffix(".title").unwrap_or(raw);
    humanize_words(raw)
}

fn shortcut_binding_label(binding: &ShortcutBindingDto) -> String {
    let mut parts = binding
        .modifiers
        .iter()
        .map(|modifier| humanize_words(modifier))
        .collect::<Vec<_>>();
    let key = match binding.key.as_str() {
        "bracketleft" => "[".into(),
        "bracketright" => "]".into(),
        "semicolon" => ";".into(),
        key => key.to_ascii_uppercase(),
    };
    parts.push(key);
    parts.join("+")
}
