//! Schema-backed Linux settings window.
//!
//! Generic fields are rendered from the Core Settings Manifest: a new Core
//! field only needs a Manifest entry and this window shows and saves it without
//! platform business logic. Projects and Shortcuts keep summary sections; their
//! full editors are out of scope for this pass.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, ComboBoxText, Entry, Label, Orientation, SearchEntry, SpinButton,
    TextView, Window,
};
use serde_json::Value;

use crate::core::config_service::{JsonPatchOperation, SettingsService};
use crate::platform::i18n::{self, Key as TextKey};

enum ControlKind {
    Switch(gtk4::Switch),
    Number(SpinButton),
    Text(Entry),
    MultiLine(TextView),
    Select(ComboBoxText),
    Summary(Label),
}

struct FieldControl {
    path: String,
    kind: ControlKind,
}

impl FieldControl {
    fn value(&self) -> Option<Value> {
        match &self.kind {
            ControlKind::Switch(widget) => Some(Value::Bool(widget.is_active())),
            ControlKind::Number(widget) => Some(Value::from(widget.value())),
            ControlKind::Text(widget) => Some(Value::String(widget.text().to_string())),
            ControlKind::MultiLine(widget) => {
                let buffer = widget.buffer();
                let text = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                Some(serde_json::to_value(text).unwrap_or(Value::Array(Vec::new())))
            }
            ControlKind::Select(widget) => {
                widget.active_id().map(|id| Value::String(id.to_string()))
            }
            ControlKind::Summary(_) => None,
        }
    }
}

fn control_row(field: &Value, values: &Value) -> (GtkBox, Option<FieldControl>) {
    let path = field["path"].as_str().unwrap_or_default().to_string();
    let control = field["control"].as_str().unwrap_or("text");
    let title = field["title_key"].as_str().unwrap_or(&path);
    let current = pointer(values, &format!("/{}", path.trim_start_matches('/')));
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    let label = Label::new(Some(title));
    label.set_halign(Align::Start);
    label.set_hexpand(true);
    row.append(&label);

    let field_control = match control {
        "switch" => {
            let widget = gtk4::Switch::new();
            widget.set_active(current.and_then(Value::as_bool).unwrap_or(false));
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::Switch(widget),
            })
        }
        "number" => {
            let widget = SpinButton::with_range(0.0, 1_000_000.0, 1.0);
            widget.set_value(current.and_then(Value::as_f64).unwrap_or(0.0));
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::Number(widget),
            })
        }
        "multiline" => {
            let widget = TextView::new();
            widget.set_wrap_mode(gtk4::WrapMode::Word);
            widget.set_size_request(-1, 84);
            let current = current
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            widget.buffer().set_text(&current);
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::MultiLine(widget),
            })
        }
        "select" | "theme_picker" | "font_picker" => {
            let widget = ComboBoxText::new();
            if let Some(options) = field["options"].as_array() {
                for option in options {
                    if let Some(value) = option.as_str() {
                        widget.append(Some(value), value);
                    }
                }
            }
            let active = current.and_then(Value::as_str).unwrap_or_default();
            if widget.active_id().is_none() {
                widget.append(Some(active), active);
            }
            widget.set_active_id(Some(active));
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::Select(widget),
            })
        }
        "font_fallback" => {
            let widget = Entry::new();
            let current = current
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            widget.set_text(&current);
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::Text(widget),
            })
        }
        "project_editor" | "shortcut_editor" => {
            let widget = Label::new(Some("Managed by the dedicated editor"));
            widget.set_halign(Align::Start);
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::Summary(widget),
            })
        }
        _ => {
            let widget = Entry::new();
            widget.set_text(current.and_then(Value::as_str).unwrap_or_default());
            row.append(&widget);
            Some(FieldControl {
                path,
                kind: ControlKind::Text(widget),
            })
        }
    };
    (row, field_control)
}

/// Open the settings window; `on_saved` fires after a committed Apply or an
/// external file change.
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
    let snapshot = service.snapshot();
    let values = snapshot.values.clone();
    let manifest = snapshot.manifest.clone();

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

    let mut rows: Vec<(GtkBox, String)> = Vec::new();
    let mut controls: Vec<FieldControl> = Vec::new();
    if let Some(groups) = manifest["groups"].as_array() {
        for group in groups {
            let group_title = group["title_key"].as_str().unwrap_or("Settings");
            let section_box = section(group_title);
            if let Some(fields) = group["fields"].as_array() {
                for field in fields {
                    let (row, control) = control_row(field, &values);
                    let path = field["path"].as_str().unwrap_or_default();
                    if let Some(control) = control {
                        if !matches!(control.kind, ControlKind::Summary(_)) {
                            controls.push(control);
                        }
                    }
                    rows.push((row.clone(), path.to_string()));
                    section_box.append(&row);
                }
            }
            root.append(&section_box);
        }
    }

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

    search.connect_search_changed(move |search| {
        let query = search.text().to_ascii_lowercase();
        for (row, path) in &rows {
            row.set_visible(query.is_empty() || path.to_ascii_lowercase().contains(&query));
        }
    });

    cancel.connect_clicked({
        let win = win.clone();
        move |_| win.close()
    });

    save.connect_clicked({
        let win = win.clone();
        let on_saved = on_saved.clone();
        let config_path = config_path.clone();
        let controls = Rc::new(RefCell::new(controls));
        move |_| {
            let mut service = match SettingsService::open(&config_path) {
                Ok(service) => service,
                Err(error) => {
                    tracing::error!(target = "muxterm::config", "打开配置事务失败: {error}");
                    return;
                }
            };
            let transaction = service.begin();
            let operations: Vec<JsonPatchOperation> = controls
                .borrow()
                .iter()
                .filter_map(|control| control.value().map(|value| replace(&control.path, value)))
                .collect();
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

fn pointer<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for token in path.trim_start_matches('/').split('/') {
        current = current.get(token)?;
    }
    Some(current)
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
    fn pointer_walks_manifest_paths() {
        let values = serde_json::json!({"font": {"size": 15.0}});
        assert_eq!(pointer(&values, "/font/size"), Some(&Value::from(15.0)));
        assert!(pointer(&values, "/font/missing").is_none());
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
