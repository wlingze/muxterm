//! Schema-backed Linux settings window.
//!
//! Generic fields are rendered from the Core Settings Manifest: a new Core
//! field only needs a Manifest entry and this window shows and saves it without
//! platform business logic. Projects and Shortcuts keep summary sections; their
//! full editors are out of scope for this pass.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, ComboBoxText, Entry, Label, ListBox, Orientation, ScrolledWindow,
    SearchEntry, SpinButton, TextView, Window,
};
use serde_json::Value;

use crate::core::config_service::{JsonPatchOperation, SettingsService};
use crate::platform::i18n::{self, Key as TextKey};
use crate::platform::linux::quickconnect::store::QuickConnectStore;

enum ControlKind {
    Switch(gtk4::Switch),
    Number(SpinButton),
    Text(Entry),
    StringList(Entry),
    MultiLine(TextView),
    Select(ComboBoxText),
    FontPicker(gtk4::FontButton),
    Summary(gtk4::Button),
}

struct FieldControl {
    path: String,
    kind: ControlKind,
    baseline: Option<Value>,
    integer: bool,
}

impl FieldControl {
    fn value(&self) -> Option<Value> {
        match &self.kind {
            ControlKind::Switch(widget) => Some(Value::Bool(widget.is_active())),
            ControlKind::Number(widget) => {
                let raw = widget.value();
                if self.integer {
                    Some(Value::from(raw.round() as i64))
                } else {
                    Some(Value::from(raw))
                }
            }
            ControlKind::Text(widget) => Some(Value::String(widget.text().to_string())),
            ControlKind::StringList(widget) => {
                let values = widget
                    .text()
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                Some(serde_json::to_value(values).unwrap_or(Value::Array(Vec::new())))
            }
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
            ControlKind::FontPicker(widget) => widget
                .font_desc()
                .and_then(|desc| desc.family())
                .map(|family| Value::String(family.to_string())),
            ControlKind::Summary(_) => None,
        }
    }

    /// 控件当前值是否偏离打开时的基线（用于脏关闭确认）。
    fn is_changed(&self) -> bool {
        self.value() != self.baseline
    }
}

fn tracked_field(path: String, kind: ControlKind) -> FieldControl {
    let mut control = FieldControl {
        path,
        kind,
        baseline: None,
        integer: false,
    };
    control.baseline = control.value();
    control
}

fn widget_name_for(path: &str) -> String {
    // e2e 契约：muxterm-prefs-<dotted path>（`/font/size` → font-size）。
    let dotted = path.trim_start_matches('/').replace(['/', '~'], "-");
    format!("muxterm-prefs-{dotted}")
}

fn control_row(field: &Value, values: &Value) -> (GtkBox, Option<FieldControl>) {
    let path = field["path"].as_str().unwrap_or_default().to_string();
    let widget_name = widget_name_for(&path);
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
            widget.set_widget_name(&widget_name);
            widget.set_active(current.and_then(Value::as_bool).unwrap_or(false));
            row.append(&widget);
            Some(tracked_field(path, ControlKind::Switch(widget)))
        }
        "number" => {
            let widget = SpinButton::with_range(0.0, 1_000_000.0, 1.0);
            widget.set_widget_name(&widget_name);
            widget.set_value(current.and_then(Value::as_f64).unwrap_or(0.0));
            let number_is_integer = current.and_then(Value::as_i64).is_some()
                || current.and_then(Value::as_u64).is_some();
            let mut control = tracked_field(path, ControlKind::Number(widget.clone()));
            control.integer = number_is_integer;
            row.append(&widget);
            Some(control)
        }
        "multiline" => {
            let widget = TextView::new();
            widget.set_widget_name(&widget_name);
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
            Some(tracked_field(path, ControlKind::MultiLine(widget)))
        }
        "font_picker" => {
            let widget = gtk4::FontButton::new();
            widget.set_widget_name(&widget_name);
            widget.set_use_size(false);
            let family = current.and_then(Value::as_str).unwrap_or_default();
            if !family.is_empty() {
                widget.set_font(&format!("{family} 12"));
            }
            row.append(&widget);
            Some(tracked_field(path, ControlKind::FontPicker(widget)))
        }
        "select" | "theme_picker" => {
            let widget = ComboBoxText::new();
            widget.set_widget_name(&widget_name);
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
            Some(tracked_field(path, ControlKind::Select(widget)))
        }
        "font_fallback" => {
            let widget = Entry::new();
            widget.set_widget_name(&widget_name);
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
            Some(tracked_field(path, ControlKind::StringList(widget)))
        }
        "project_editor" => {
            let widget = Button::with_label("Manage projects…");
            widget.set_widget_name(&widget_name);
            widget.set_halign(Align::Start);
            row.append(&widget);
            Some(tracked_field(path, ControlKind::Summary(widget)))
        }
        "shortcut_editor" => {
            let widget = Button::with_label("Manage shortcuts…");
            widget.set_widget_name(&widget_name);
            widget.set_halign(Align::Start);
            row.append(&widget);
            Some(tracked_field(path, ControlKind::Summary(widget)))
        }
        _ => {
            let widget = Entry::new();
            widget.set_widget_name(&widget_name);
            widget.set_text(current.and_then(Value::as_str).unwrap_or_default());
            row.append(&widget);
            Some(tracked_field(path, ControlKind::Text(widget)))
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
    project_editor: Option<(
        Vec<crate::core::catalog::driver::RuntimeInfo>,
        Vec<crate::platform::linux::ffi_bridge::SshHostEntry>,
    )>,
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
    let controls = Rc::new(RefCell::new(controls));
    let allow_close = Rc::new(Cell::new(false));

    // 专用编辑器：项目 / 快捷键按钮在独立窗口中编辑，保存后刷新主窗口。
    {
        let editor_window = win.clone();
        for control in controls.borrow().iter() {
            if let ControlKind::Summary(button) = &control.kind {
                let config_path = config_path.clone();
                let on_saved = on_saved.clone();
                let project_editor = project_editor.clone();
                let editor_window = editor_window.clone();
                if control.path == "/projects" {
                    button.connect_clicked(move |_| {
                        if let Some((runtimes, hosts)) = &project_editor {
                            show_project_manager(
                                &editor_window,
                                config_path.clone(),
                                runtimes.clone(),
                                hosts.clone(),
                                on_saved.clone(),
                            );
                        }
                    });
                } else if control.path == "/shortcuts/overrides" {
                    button.connect_clicked(move |_| {
                        show_shortcut_manager(
                            &editor_window,
                            config_path.clone(),
                            on_saved.clone(),
                        );
                    });
                }
            }
        }
    }

    search.connect_search_changed(move |search| {
        let query = search.text().to_ascii_lowercase();
        for (row, path) in &rows {
            row.set_visible(query.is_empty() || path.to_ascii_lowercase().contains(&query));
        }
    });

    cancel.connect_clicked({
        let win = win.clone();
        let controls = controls.clone();
        let allow_close = allow_close.clone();
        move |_| {
            if controls.borrow().iter().any(FieldControl::is_changed) {
                confirm_discard(&win, {
                    let win = win.clone();
                    let allow_close = allow_close.clone();
                    move || {
                        allow_close.set(true);
                        win.close();
                    }
                });
            } else {
                allow_close.set(true);
                win.close();
            }
        }
    });

    save.connect_clicked({
        let win = win.clone();
        let on_saved = on_saved.clone();
        let config_path = config_path.clone();
        let controls = controls.clone();
        let allow_close = allow_close.clone();
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
            allow_close.set(true);
            win.close();
        }
    });

    win.connect_close_request({
        let win = win.clone();
        let controls = controls.clone();
        let allow_close = allow_close.clone();
        move |_| {
            if allow_close.get() {
                return glib::Propagation::Proceed;
            }
            if controls.borrow().iter().any(FieldControl::is_changed) {
                confirm_discard(&win, {
                    let win = win.clone();
                    let allow_close = allow_close.clone();
                    move || {
                        allow_close.set(true);
                        win.close();
                    }
                });
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
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

fn confirm_discard(parent: &impl IsA<Window>, on_discard: impl Fn() + 'static) {
    let dialog = Window::builder()
        .modal(true)
        .title("Discard unsaved changes?")
        .default_width(380)
        .build();
    dialog.set_transient_for(Some(parent));
    dialog.set_widget_name("muxterm-prefs-discard");
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(14)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let label = Label::new(Some("Your edits have not been applied to config.toml."));
    label.set_wrap(true);
    label.set_halign(Align::Start);
    root.append(&label);
    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let cancel = Button::with_label("Cancel");
    let discard = Button::with_label("Discard");
    actions.append(&cancel);
    actions.append(&discard);
    root.append(&actions);
    dialog.set_child(Some(&root));

    let finished = Rc::new(Cell::new(false));
    let on_discard = Rc::new(on_discard);
    {
        let dialog = dialog.clone();
        let finished = finished.clone();
        cancel.connect_clicked(move |_| {
            if !finished.replace(true) {
                dialog.close();
            }
        });
    }
    {
        let dialog = dialog.clone();
        let finished = finished.clone();
        let on_discard = on_discard.clone();
        discard.connect_clicked(move |_| {
            if !finished.replace(true) {
                on_discard();
                dialog.close();
            }
        });
    }
    dialog.present();
}

fn show_project_manager(
    parent: &impl IsA<Window>,
    config_path: PathBuf,
    runtimes: Vec<crate::core::catalog::driver::RuntimeInfo>,
    hosts: Vec<crate::platform::linux::ffi_bridge::SshHostEntry>,
    on_changed: Rc<Box<dyn Fn() + 'static>>,
) {
    let win = Window::builder()
        .title("Projects")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(480)
        .build();
    win.set_widget_name("muxterm-projects-window");
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let list = ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    let sw = ScrolledWindow::builder()
        .min_content_height(280)
        .child(&list)
        .build();
    root.append(&sw);

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let add = Button::with_label("New project…");
    let close = Button::with_label("Close");
    actions.append(&add);
    actions.append(&close);
    root.append(&actions);
    win.set_child(Some(&root));

    let refresh = {
        let list = list.clone();
        let config_path = config_path.clone();
        let win_for_rows = win.clone();
        let hosts_for_rows = hosts.clone();
        let runtimes_for_rows = runtimes.clone();
        let on_changed_for_rows = on_changed.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let mut service = match SettingsService::open(&config_path) {
                Ok(service) => service,
                Err(_) => return,
            };
            if let Err(error) = service.migrate_legacy_quickconnect() {
                tracing::warn!(
                    target = "muxterm::config",
                    "QuickConnect 迁移未完成: {error}"
                );
            }
            let projects = service.document().projects.clone();
            for project in &projects {
                let row = GtkBox::builder()
                    .orientation(Orientation::Horizontal)
                    .spacing(8)
                    .build();
                let label = Label::new(Some(&project.name));
                label.set_hexpand(true);
                label.set_halign(Align::Start);
                row.append(&label);
                let edit = Button::with_label("Edit…");
                let remove = Button::with_label("Remove");
                let project_for_edit = project.clone();
                let config_for_edit = config_path.clone();
                let on_changed = on_changed_for_rows.clone();
                let win = win_for_rows.clone();
                let hosts = hosts_for_rows.clone();
                let runtimes = runtimes_for_rows.clone();
                edit.connect_clicked(move |_| {
                    let store = Rc::new(RefCell::new(QuickConnectStore::new_unified(Some(
                        config_for_edit.clone(),
                    ))));
                    let target = match project_for_edit.to_target() {
                        Ok(target) => target,
                        Err(error) => {
                            tracing::error!(
                                target = "muxterm::config",
                                "Project 解析失败: {error}"
                            );
                            return;
                        }
                    };
                    let store_inner = store.clone();
                    let on_changed = on_changed.clone();
                    crate::platform::linux::target_config_window::show(
                        &win,
                        Some(target),
                        store.borrow().clone(),
                        hosts.clone(),
                        runtimes.clone(),
                        move |saved| {
                            store_inner.borrow_mut().upsert_project(&saved);
                            on_changed();
                        },
                        || {},
                    );
                });
                let project_for_remove = project.clone();
                let config_for_remove = config_path.clone();
                let on_changed = on_changed_for_rows.clone();
                remove.connect_clicked(move |_| {
                    let mut service = match SettingsService::open(&config_for_remove) {
                        Ok(service) => service,
                        Err(error) => {
                            tracing::error!(target = "muxterm::config", "打开配置失败: {error}");
                            return;
                        }
                    };
                    let transaction = service.begin();
                    let index = service
                        .document()
                        .projects
                        .iter()
                        .position(|item| item.id == project_for_remove.id);
                    if let Some(index) = index {
                        let operation = JsonPatchOperation {
                            op: "remove".into(),
                            path: format!("/projects/{index}"),
                            value: None,
                        };
                        if service
                            .patch(&transaction, &[operation])
                            .and_then(|_| service.commit(&transaction).map(|_| ()))
                            .is_ok()
                        {
                            on_changed();
                        }
                    }
                });
                row.append(&edit);
                row.append(&remove);
                list.append(&row);
            }
        }
    };
    let refresh = Rc::new(RefCell::new(refresh));
    refresh.borrow()();

    // 文件变更（含本窗口的编辑/删除）后自动重建列表，避免自引用闭包。
    if let Ok(monitor) = gtk4::gio::File::for_path(&config_path).monitor_file(
        gtk4::gio::FileMonitorFlags::NONE,
        gtk4::gio::Cancellable::NONE,
    ) {
        let refresh = refresh.clone();
        monitor.connect_changed(move |_, _, _, _| refresh.borrow()());
    }

    {
        let win = win.clone();
        close.connect_clicked(move |_| win.close());
    }
    {
        let win = win.clone();
        let hosts = hosts.clone();
        let runtimes = runtimes.clone();
        let on_changed = on_changed.clone();
        let refresh = refresh.clone();
        add.connect_clicked(move |_| {
            let config_path = config_path.clone();
            let store = Rc::new(RefCell::new(QuickConnectStore::new_unified(Some(
                config_path.clone(),
            ))));
            let store_inner = store.clone();
            let refresh = refresh.clone();
            let on_changed = on_changed.clone();
            crate::platform::linux::target_config_window::show(
                &win,
                None,
                store.borrow().clone(),
                hosts.clone(),
                runtimes.clone(),
                move |saved| {
                    store_inner.borrow_mut().upsert_project(&saved);
                    on_changed();
                    refresh.borrow()();
                },
                || {},
            );
        });
    }
    win.present();
}

fn show_shortcut_manager(
    app: &impl IsA<Window>,
    config_path: PathBuf,
    on_changed: Rc<Box<dyn Fn() + 'static>>,
) {
    let win = Window::builder()
        .title("Shortcuts")
        .transient_for(app)
        .modal(true)
        .default_width(720)
        .default_height(560)
        .build();
    win.set_widget_name("muxterm-shortcuts-window");
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let list = ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    let sw = ScrolledWindow::builder()
        .min_content_height(360)
        .child(&list)
        .build();
    root.append(&sw);

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let close = Button::with_label("Close");
    actions.append(&close);
    root.append(&actions);
    win.set_child(Some(&root));

    let refresh = {
        let list = list.clone();
        let config_path = config_path.clone();
        let win_for_rows = win.clone();
        let on_changed_for_rows = on_changed.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let mut service = match SettingsService::open(&config_path) {
                Ok(service) => service,
                Err(_) => return,
            };
            if let Err(error) = service.migrate_legacy_quickconnect() {
                tracing::warn!(
                    target = "muxterm::config",
                    "QuickConnect 迁移未完成: {error}"
                );
            }
            let shortcuts = service.document().shortcuts.clone();
            let catalog = crate::core::config_service::action_catalog();
            for action in &catalog {
                let override_item = shortcuts
                    .overrides
                    .iter()
                    .find(|item| item.action == action.id);
                let row = GtkBox::builder()
                    .orientation(Orientation::Horizontal)
                    .spacing(8)
                    .build();
                let label = Label::new(Some(&action.title_key));
                label.set_hexpand(true);
                label.set_halign(Align::Start);
                row.append(&label);
                let summary = Label::new(Some(
                    &override_item
                        .map(|item| {
                            if item.bindings.is_empty() {
                                "disabled".into()
                            } else {
                                item.bindings
                                    .iter()
                                    .map(|b| format!("{} {}", b.modifiers.join("+"), b.key))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        })
                        .unwrap_or_else(|| "default".into()),
                ));
                summary.set_halign(Align::Start);
                summary.set_hexpand(true);
                row.append(&summary);

                let bind = Button::with_label("Bind…");
                let unbind = Button::with_label("Unbind");
                let action_id = action.id.to_string();
                let config_for_bind = config_path.clone();
                let win_for_capture = win_for_rows.clone();
                let on_changed = on_changed_for_rows.clone();
                bind.connect_clicked(move |_| {
                    let action_id = action_id.clone();
                    capture_shortcut(&win_for_capture, {
                        let config_path = config_for_bind.clone();
                        let on_changed = on_changed.clone();
                        move |key, modifiers| {
                            let mut service = match SettingsService::open(&config_path) {
                                Ok(service) => service,
                                Err(error) => {
                                    tracing::error!(
                                        target = "muxterm::config",
                                        "打开配置事务失败: {error}"
                                    );
                                    return;
                                }
                            };
                            let transaction = service.begin();
                            let mut overrides = service.document().shortcuts.overrides.clone();
                            overrides.retain(|item| item.action != action_id);
                            overrides.push(crate::core::config_service::ShortcutOverride {
                                action: action_id.clone(),
                                bindings: vec![crate::core::config_service::ShortcutBinding {
                                    key,
                                    modifiers,
                                }],
                            });
                            let value =
                                serde_json::to_value(overrides).unwrap_or(Value::Array(Vec::new()));
                            let operation = JsonPatchOperation {
                                op: "replace".into(),
                                path: "/shortcuts/overrides".into(),
                                value: Some(value),
                            };
                            if service
                                .patch(&transaction, &[operation])
                                .and_then(|_| service.commit(&transaction).map(|_| ()))
                                .is_ok()
                            {
                                on_changed();
                            }
                        }
                    });
                });
                let action_id = action.id.to_string();
                let config_for_unbind = config_path.clone();
                let on_changed = on_changed_for_rows.clone();
                unbind.connect_clicked(move |_| {
                    let mut service = match SettingsService::open(&config_for_unbind) {
                        Ok(service) => service,
                        Err(error) => {
                            tracing::error!(
                                target = "muxterm::config",
                                "打开配置事务失败: {error}"
                            );
                            return;
                        }
                    };
                    let transaction = service.begin();
                    let mut overrides = service.document().shortcuts.overrides.clone();
                    overrides.retain(|item| item.action != action_id);
                    let value = serde_json::to_value(overrides).unwrap_or(Value::Array(Vec::new()));
                    let operation = JsonPatchOperation {
                        op: "replace".into(),
                        path: "/shortcuts/overrides".into(),
                        value: Some(value),
                    };
                    if service
                        .patch(&transaction, &[operation])
                        .and_then(|_| service.commit(&transaction).map(|_| ()))
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
        let refresh = refresh.clone();
        monitor.connect_changed(move |_, _, _, _| refresh.borrow()());
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
    fn text_control_tracks_dirty_state() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("skip: 无 DISPLAY");
            return;
        }
        gtk4::test_synced(|| {
            let entry = gtk4::Entry::new();
            entry.set_text("a");
            let control = tracked_field("/font/family".into(), ControlKind::Text(entry));
            assert!(!control.is_changed());
            if let ControlKind::Text(entry) = &control.kind {
                entry.set_text("b");
            }
            assert!(control.is_changed());
        });
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
                None,
            );
            assert_eq!(win.widget_name(), "muxterm-prefs-window");
            win.close();
            win.destroy();
            parent.destroy();
        });
    }
}
