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
    Align, Box as GtkBox, Button, ComboBoxText, CssProvider, Entry, Label, ListBox, ListBoxRow,
    Orientation, ScrolledWindow, SearchEntry, Separator, SpinButton, Stack, TextView, Window,
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

struct CategoryPage {
    id: String,
    navigation_index: i32,
    fields: Vec<(glib::WeakRef<GtkBox>, String)>,
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

/// 数字控件必须在建立 baseline 前确定整数/小数语义。
///
/// JSON 中的整数读入 GTK 后会变成 `f64`，如果先用小数 baseline 再切换
/// `integer`，一个完全没改过的整数设置也会被错误地标记为 dirty。
fn tracked_number(path: String, widget: SpinButton, integer: bool) -> FieldControl {
    let mut control = FieldControl {
        path,
        kind: ControlKind::Number(widget),
        baseline: None,
        integer,
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
    let title = field_title(&path, field["title_key"].as_str().unwrap_or(""));
    let description = field_description(&path);
    let apply = apply_label(field["apply"].as_str().unwrap_or("commit"));
    let current = pointer(values, &format!("/{}", path.trim_start_matches('/')));
    let row = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(9)
        .build();
    row.add_css_class("prefs-setting-row");

    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(18)
        .hexpand(true)
        .build();
    let copy = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .build();
    let label = Label::new(Some(&title));
    label.set_halign(Align::Start);
    label.set_xalign(0.0);
    label.add_css_class("prefs-setting-title");
    copy.append(&label);

    let description_label = Label::new(Some(description));
    description_label.set_halign(Align::Start);
    description_label.set_xalign(0.0);
    description_label.set_hexpand(true);
    description_label.set_wrap(true);
    description_label.set_max_width_chars(58);
    description_label.add_css_class("prefs-setting-description");
    copy.append(&description_label);

    let apply_label_widget = Label::new(Some(apply));
    apply_label_widget.set_valign(Align::Center);
    apply_label_widget.set_halign(Align::Center);
    apply_label_widget.set_size_request(124, -1);
    apply_label_widget.add_css_class("prefs-apply-badge");
    content.append(&copy);

    // 所有普通控件共用固定的右侧槽位；开关也靠右，不随左侧文案长度漂移。
    let control_slot = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(0)
        .build();
    control_slot.set_size_request(208, -1);
    let control_spacer = GtkBox::new(Orientation::Horizontal, 0);
    control_spacer.set_hexpand(true);
    control_slot.append(&control_spacer);

    row.append(&content);

    let field_control = match control {
        "switch" => {
            let widget = gtk4::Switch::new();
            widget.set_widget_name(&widget_name);
            widget.set_active(current.and_then(Value::as_bool).unwrap_or(false));
            widget.set_valign(Align::Center);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::Switch(widget)))
        }
        "number" => {
            let (min, max, step, digits) = number_spec(&path, current);
            let widget = SpinButton::with_range(min, max, step);
            widget.set_widget_name(&widget_name);
            widget.set_digits(digits);
            widget.set_value(current.and_then(Value::as_f64).unwrap_or(0.0));
            widget.set_width_chars(9);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_number(path, widget, digits == 0))
        }
        "multiline" => {
            let widget = TextView::new();
            widget.set_widget_name(&widget_name);
            widget.set_wrap_mode(gtk4::WrapMode::Word);
            widget.set_top_margin(10);
            widget.set_bottom_margin(10);
            widget.set_left_margin(10);
            widget.set_right_margin(10);
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
            let editor = ScrolledWindow::builder()
                .child(&widget)
                .min_content_height(104)
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vscrollbar_policy(gtk4::PolicyType::Automatic)
                .build();
            editor.add_css_class("prefs-text-editor");
            row.append(&editor);
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
            widget.set_size_request(208, -1);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::FontPicker(widget)))
        }
        "select" | "theme_picker" => {
            let widget = ComboBoxText::new();
            widget.set_widget_name(&widget_name);
            let active = current.and_then(Value::as_str).unwrap_or_default();
            let mut has_active_option = false;
            if let Some(options) = field["options"].as_array() {
                for option in options {
                    if let Some(value) = option.as_str() {
                        has_active_option |= value == active;
                        widget.append(Some(value), &option_label(&path, value));
                    }
                }
            }
            if !has_active_option {
                widget.append(Some(active), &option_label(&path, active));
            }
            widget.set_active_id(Some(active));
            widget.set_size_request(208, -1);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::Select(widget)))
        }
        "font_fallback" => {
            let widget = Entry::new();
            widget.set_widget_name(&widget_name);
            widget.set_placeholder_text(Some("Noto Sans Mono, monospace"));
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
            widget.set_size_request(208, -1);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::StringList(widget)))
        }
        "project_editor" => {
            let widget = Button::with_label("Manage projects");
            widget.set_widget_name(&widget_name);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::Summary(widget)))
        }
        "shortcut_editor" => {
            let widget = Button::with_label("Manage shortcuts");
            widget.set_widget_name(&widget_name);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::Summary(widget)))
        }
        _ => {
            let widget = Entry::new();
            widget.set_widget_name(&widget_name);
            widget.set_placeholder_text(Some(input_placeholder(&path)));
            widget.set_text(current.and_then(Value::as_str).unwrap_or_default());
            widget.set_size_request(208, -1);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
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
    install_preferences_css();

    let win = Window::builder()
        .title(i18n::tr(TextKey::CmdPreferences))
        .default_width(980)
        .default_height(720)
        .modal(true)
        .transient_for(parent)
        .build();
    win.set_widget_name("muxterm-prefs-window");
    win.add_css_class("muxterm-preferences-window");

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
        .spacing(0)
        .build();
    root.add_css_class("prefs-root");

    let header = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(14)
        .margin_top(24)
        .margin_bottom(20)
        .margin_start(28)
        .margin_end(28)
        .build();
    header.add_css_class("prefs-header");
    let mark = Label::new(Some("⌘"));
    mark.add_css_class("prefs-header-mark");
    mark.set_valign(Align::Center);
    header.append(&mark);
    let heading = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let header_title = Label::new(Some(&i18n::tr(TextKey::CmdPreferences)));
    header_title.set_halign(Align::Start);
    header_title.add_css_class("prefs-header-title");
    heading.append(&header_title);
    let header_subtitle = Label::new(Some("Make Muxterm feel like yours."));
    header_subtitle.set_halign(Align::Start);
    header_subtitle.add_css_class("prefs-header-subtitle");
    heading.append(&header_subtitle);
    let config_label = Label::new(Some(&format!("Config file  ·  {}", config_path.display())));
    config_label.set_halign(Align::Start);
    config_label.add_css_class("prefs-config-path");
    heading.append(&config_label);
    header.append(&heading);

    let search = SearchEntry::new();
    search.set_widget_name("muxterm-prefs-search");
    search.set_placeholder_text(Some("Search preferences"));
    search.set_tooltip_text(Some("Search by setting name or keyword"));
    search.set_size_request(260, -1);
    search.add_css_class("prefs-search");
    header.append(&search);
    root.append(&header);

    let header_separator = Separator::new(Orientation::Horizontal);
    header_separator.add_css_class("prefs-divider");
    root.append(&header_separator);

    let body = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(0)
        .vexpand(true)
        .build();
    body.set_widget_name("muxterm-prefs-body");
    body.add_css_class("prefs-body");

    let sidebar = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(22)
        .margin_bottom(22)
        .margin_start(18)
        .margin_end(14)
        .build();
    sidebar.set_size_request(220, -1);
    sidebar.add_css_class("prefs-sidebar");
    let sidebar_label = Label::new(Some("CONFIGURATION"));
    sidebar_label.set_halign(Align::Start);
    sidebar_label.add_css_class("prefs-sidebar-label");
    sidebar.append(&sidebar_label);

    let categories = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .vexpand(true)
        .build();
    categories.set_widget_name("muxterm-prefs-categories");
    categories.add_css_class("muxterm-prefs-categories");
    let categories_scroll = ScrolledWindow::builder()
        .vexpand(true)
        .child(&categories)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    categories_scroll.set_widget_name("muxterm-prefs-categories-scroll");
    categories_scroll.add_css_class("prefs-category-scroll");
    sidebar.append(&categories_scroll);
    body.append(&sidebar);

    let sidebar_separator = Separator::new(Orientation::Vertical);
    sidebar_separator.add_css_class("prefs-divider");
    body.append(&sidebar_separator);

    let pages = Stack::new();
    pages.set_widget_name("muxterm-prefs-pages");
    pages.add_css_class("muxterm-prefs-pages");
    pages.set_hexpand(true);
    pages.set_vexpand(true);
    body.append(&pages);
    root.append(&body);

    let mut controls: Vec<FieldControl> = Vec::new();
    let mut category_pages: Vec<CategoryPage> = Vec::new();
    if let Some(groups) = manifest["groups"].as_array() {
        for group in groups {
            let group_id = group["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or("settings")
                .to_string();
            let group_title = group["title_key"].as_str().unwrap_or("Settings");

            let navigation_row = ListBoxRow::new();
            navigation_row.set_widget_name(&format!("muxterm-prefs-category-{group_id}"));
            navigation_row.add_css_class("muxterm-prefs-category-row");
            let navigation_content = GtkBox::builder()
                .orientation(Orientation::Horizontal)
                .spacing(12)
                .margin_top(9)
                .margin_bottom(9)
                .margin_start(10)
                .margin_end(10)
                .build();
            let navigation_icon = Label::new(Some(category_icon(&group_id)));
            navigation_icon.set_width_chars(2);
            navigation_icon.set_halign(Align::Center);
            navigation_icon.add_css_class("prefs-nav-icon");
            navigation_content.append(&navigation_icon);
            let navigation_copy = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(1)
                .hexpand(true)
                .build();
            let navigation_label = Label::new(Some(&category_title(&group_id, group_title)));
            navigation_label.set_halign(Align::Start);
            navigation_label.set_xalign(0.0);
            navigation_label.add_css_class("prefs-nav-title");
            navigation_copy.append(&navigation_label);
            let navigation_hint = Label::new(Some(category_hint(&group_id)));
            navigation_hint.set_halign(Align::Start);
            navigation_hint.set_xalign(0.0);
            navigation_hint.add_css_class("prefs-nav-hint");
            navigation_copy.append(&navigation_hint);
            navigation_content.append(&navigation_copy);
            navigation_row.set_child(Some(&navigation_content));
            categories.append(&navigation_row);

            let page_content = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(20)
                .hexpand(true)
                .margin_top(28)
                .margin_bottom(28)
                .margin_start(34)
                .margin_end(38)
                .build();
            page_content.set_widget_name(&format!("muxterm-prefs-page-{group_id}"));
            page_content.add_css_class("prefs-page");

            let page_heading = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(5)
                .build();
            let page_title = Label::new(Some(&category_title(&group_id, group_title)));
            page_title.set_halign(Align::Start);
            page_title.add_css_class("prefs-page-title");
            page_heading.append(&page_title);
            let page_description = Label::new(Some(category_description(&group_id)));
            page_description.set_halign(Align::Start);
            page_description.set_xalign(0.0);
            page_description.set_wrap(true);
            page_description.set_max_width_chars(78);
            page_description.add_css_class("prefs-page-description");
            page_heading.append(&page_description);
            page_content.append(&page_heading);

            let section_box = section(&group_id, group_title);
            let mut page_fields = Vec::new();
            let mut has_field = false;
            if let Some(manifest_fields) = group["fields"].as_array() {
                for field in manifest_fields {
                    let (row, control) = control_row(field, &values);
                    let path = field["path"].as_str().unwrap_or_default();
                    if let Some(control) = control {
                        controls.push(control);
                    }
                    let row_ref = glib::WeakRef::new();
                    row_ref.set(Some(&row));
                    let title = field_title(path, field["title_key"].as_str().unwrap_or(""));
                    page_fields.push((
                        row_ref,
                        format!("{group_id} {path} {title} {}", field_description(path)),
                    ));
                    if has_field {
                        let divider = Separator::new(Orientation::Horizontal);
                        divider.add_css_class("prefs-card-divider");
                        section_box.append(&divider);
                    }
                    section_box.append(&row);
                    has_field = true;
                }
            }
            page_content.append(&section_box);
            if group_id == "appearance" {
                page_content.append(&appearance_preview(&values));
            }
            let page = ScrolledWindow::builder()
                .child(&page_content)
                .hexpand(true)
                .vexpand(true)
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vscrollbar_policy(gtk4::PolicyType::Automatic)
                .build();
            page.set_widget_name(&format!("muxterm-prefs-scroll-{group_id}"));
            pages.add_named(&page, Some(&group_id));
            category_pages.push(CategoryPage {
                id: group_id,
                navigation_index: category_pages.len() as i32,
                fields: page_fields,
            });
        }
    }

    let category_pages = Rc::new(category_pages);
    {
        let pages = pages.clone();
        let category_pages = category_pages.clone();
        categories.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            let Some(category) = category_pages.get(row.index().max(0) as usize) else {
                return;
            };
            pages.set_visible_child_name(&category.id);
        });
    }

    if let Some(first) = category_pages.first() {
        if let Some(row) = categories.row_at_index(first.navigation_index) {
            categories.select_row(Some(&row));
        }
        pages.set_visible_child_name(&first.id);
    }

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(28)
        .margin_end(28)
        .build();
    actions.add_css_class("prefs-footer");
    let save_status = Label::new(Some(
        "Changes are saved to config.toml when you click Save.",
    ));
    save_status.set_halign(Align::Start);
    save_status.set_xalign(0.0);
    save_status.set_hexpand(true);
    save_status.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    save_status.set_widget_name("muxterm-prefs-status");
    save_status.add_css_class("prefs-footer-status");
    actions.append(&save_status);
    let cancel = gtk4::Button::with_label("Cancel");
    cancel.set_widget_name("muxterm-prefs-cancel");
    cancel.add_css_class("prefs-secondary-action");
    let save = gtk4::Button::with_label(&i18n::tr(TextKey::Save));
    save.set_widget_name("muxterm-prefs-save");
    save.add_css_class("suggested-action");
    save.set_tooltip_text(Some("Write changes to config.toml"));
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

    {
        let categories = categories.clone();
        let pages = pages.clone();
        let category_pages = category_pages.clone();
        search.connect_search_changed(move |search| {
            let query = search.text().trim().to_ascii_lowercase();
            let mut first_match = None;
            for category in category_pages.iter() {
                let mut category_matches = false;
                for (row, search_text) in &category.fields {
                    let visible =
                        query.is_empty() || search_text.to_ascii_lowercase().contains(&query);
                    if let Some(row) = row.upgrade() {
                        row.set_visible(visible);
                    }
                    category_matches |= visible;
                }
                if let Some(row) = categories.row_at_index(category.navigation_index) {
                    row.set_visible(query.is_empty() || category_matches);
                }
                if category_matches && first_match.is_none() {
                    first_match = Some(category);
                }
            }
            if let Some(category) = first_match {
                if let Some(row) = categories.row_at_index(category.navigation_index) {
                    categories.select_row(Some(&row));
                }
                pages.set_visible_child_name(&category.id);
            }
        });
    }

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
        let save_status = save_status.clone();
        move |_| {
            let mut service = match SettingsService::open(&config_path) {
                Ok(service) => service,
                Err(error) => {
                    tracing::error!(target = "muxterm::config", "打开配置事务失败: {error}");
                    save_status
                        .set_text("Could not read config.toml. Check the file and try again.");
                    save_status.add_css_class("error");
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
                save_status.set_text(&format!("Could not save settings: {error}"));
                save_status.add_css_class("error");
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
    install_preferences_css();
    let win = Window::builder()
        .title("Projects")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(480)
        .build();
    win.set_widget_name("muxterm-projects-window");
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
        "Projects",
        "Saved workspace profiles for Quick Connect.",
    ));

    let list = ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.add_css_class("prefs-list-card");
    let sw = ScrolledWindow::builder()
        .min_content_height(280)
        .child(&list)
        .build();
    sw.add_css_class("prefs-subwindow-scroll");
    root.append(&sw);

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let add = Button::with_label("New project…");
    add.add_css_class("suggested-action");
    let close = Button::with_label("Close");
    close.add_css_class("prefs-secondary-action");
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
                    .margin_top(10)
                    .margin_bottom(10)
                    .margin_start(12)
                    .margin_end(12)
                    .build();
                row.add_css_class("prefs-project-row");
                let copy = GtkBox::builder()
                    .orientation(Orientation::Vertical)
                    .spacing(2)
                    .hexpand(true)
                    .build();
                let label = Label::new(Some(&project.name));
                label.set_hexpand(true);
                label.set_halign(Align::Start);
                label.add_css_class("prefs-project-name");
                copy.append(&label);
                let detail = Label::new(Some(&format!(
                    "{}  ·  {} / {}",
                    project.path, project.transport.id, project.runtime.id
                )));
                detail.set_halign(Align::Start);
                detail.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                detail.add_css_class("prefs-project-detail");
                copy.append(&detail);
                row.append(&copy);
                let edit = Button::with_label("Edit…");
                edit.add_css_class("prefs-inline-action");
                let remove = Button::with_label("Remove");
                remove.add_css_class("destructive-action");
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

fn section(id: &str, title_key: &str) -> GtkBox {
    let box_ = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .build();
    box_.set_hexpand(true);
    box_.add_css_class("prefs-card");
    let header = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(3)
        .margin_top(18)
        .margin_bottom(10)
        .margin_start(16)
        .margin_end(16)
        .build();
    let label = Label::new(Some(section_title(id, title_key)));
    label.set_halign(Align::Start);
    label.add_css_class("prefs-card-title");
    header.append(&label);
    let hint = Label::new(Some("Changes are staged until you save."));
    hint.set_halign(Align::Start);
    hint.add_css_class("prefs-card-hint");
    header.append(&hint);
    box_.append(&header);
    box_
}

fn subwindow_header(title: &str, description: &str) -> GtkBox {
    let header = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_bottom(4)
        .build();
    let title_label = Label::new(Some(title));
    title_label.set_halign(Align::Start);
    title_label.add_css_class("prefs-page-title");
    header.append(&title_label);
    let description_label = Label::new(Some(description));
    description_label.set_halign(Align::Start);
    description_label.add_css_class("prefs-page-description");
    header.append(&description_label);
    header
}

fn section_title(id: &str, title_key: &str) -> String {
    match id {
        "appearance" => "Terminal".into(),
        "runtime" => "Workspace defaults".into(),
        "attention" => "Attention".into(),
        "ui" => "Interface".into(),
        "ssh" => "SSH defaults".into(),
        "behavior" => "Exit behavior".into(),
        "platform" => "Platform".into(),
        "projects" => "Workspace profiles".into(),
        "shortcuts" => "Keyboard shortcuts".into(),
        _ => category_title(id, title_key),
    }
}

fn action_title(title_key: &str) -> String {
    let raw = title_key.strip_prefix("action.").unwrap_or(title_key);
    let raw = raw.strip_suffix(".title").unwrap_or(raw);
    humanize_words(raw)
}

fn shortcut_binding_label(binding: &crate::core::config_service::ShortcutBinding) -> String {
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

fn number_spec(path: &str, current: Option<&Value>) -> (f64, f64, f64, u32) {
    let (min, max, step, digits) = match path {
        "/font/size" => (9.0, 72.0, 0.5, 1),
        "/ssh/port" => (1.0, 65_535.0, 1.0, 0),
        "/attention/debounce_ms" => (0.0, 10_000.0, 50.0, 0),
        "/ui/tab_bar_height" => (16.0, 96.0, 1.0, 0),
        // Core 的类型是 u32，且文档只规定了下限，不能在设置页伪造硬上限。
        "/pool/max_slots" => (1.0, f64::from(u32::MAX), 1.0, 0),
        "/scrollback/lines" => (100.0, 1_000_000.0, 100.0, 0),
        _ => (0.0, 1_000_000.0, 1.0, 0),
    };
    // 即使历史配置超出当前建议范围，也不能仅仅因为打开设置页就丢值。
    let max = current
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .map_or(max, |value| max.max(value));
    (min, max, step, digits)
}

fn field_title(path: &str, title_key: &str) -> String {
    match path {
        "/font/family" => "Font family".into(),
        "/font/size" => "Font size".into(),
        "/font/fallback" => "Fallback fonts".into(),
        "/theme/name" => "Theme".into(),
        "/theme/light" => "Light theme".into(),
        "/theme/dark" => "Dark theme".into(),
        "/statusbar/mode" => "Status bar appearance".into(),
        "/tmux/auto_mouse" => "Enable tmux mouse mode".into(),
        "/tmux/default_session" => "Default workspace".into(),
        "/tmux/socket" => "tmux socket".into(),
        "/pool/max_slots" => "Workspace reminder limit".into(),
        "/scrollback/lines" => "Scrollback lines".into(),
        "/pane/default_command" => "Default shell command".into(),
        "/pane/workdir" => "Initial working directory".into(),
        "/attention/enabled" => "Workspace attention".into(),
        "/attention/blocked_regex" => "Blocked output patterns".into(),
        "/attention/debounce_ms" => "Notification delay".into(),
        "/ui/tab_bar_position" => "Tab bar position".into(),
        "/ui/tab_bar_height" => "Tab bar height".into(),
        "/ui/show_title_bar" => "Show title bar".into(),
        "/ui/borderless" => "Borderless window".into(),
        "/ssh/host" => "Default SSH host".into(),
        "/ssh/port" => "SSH port".into(),
        "/ssh/user" => "SSH user".into(),
        "/ssh/key_path" => "SSH private key".into(),
        "/behavior/on_last_pane_exit" => "When the last pane exits".into(),
        "/behavior/on_program_exit_abnormal" => "When a command fails".into(),
        "/platform/linux/client_side_decorations" => "Client-side decorations".into(),
        "/platform/macos/option_as_alt" => "Treat Option as Alt".into(),
        "/shortcuts/preset" => "Keyboard layout".into(),
        "/shortcuts/primary_key" => "Primary modifier".into(),
        "/projects" => "Saved projects".into(),
        "/shortcuts/overrides" => "Custom shortcuts".into(),
        _ => {
            let raw = title_key
                .strip_prefix("settings.")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path));
            humanize_words(raw)
        }
    }
}

fn field_description(path: &str) -> &'static str {
    match path {
        "/font/family" => "The typeface used to draw terminal text.",
        "/font/size" => "Adjust the terminal scale without changing your display settings.",
        "/font/fallback" => {
            "Comma-separated fonts used when the primary family is missing a glyph."
        }
        "/theme/name" => "Choose a fixed theme or follow your system appearance.",
        "/theme/light" => "Theme used when the system is in light mode.",
        "/theme/dark" => "Theme used when the system is in dark mode.",
        "/statusbar/mode" => "Use tmux colors or keep the status bar in the Muxterm theme.",
        "/tmux/auto_mouse" => "Forward mouse interactions to attached tmux workspaces.",
        "/tmux/default_session" => "Workspace to attach on launch; leave empty to start locally.",
        "/tmux/socket" => "Optional named tmux socket. Empty uses the default server.",
        "/pool/max_slots" => "Show a reminder when this many warm workspaces are open.",
        "/scrollback/lines" => "History kept for each newly created pane.",
        "/pane/default_command" => "Command started for a new local pane.",
        "/pane/workdir" => "Directory used when a new local pane starts.",
        "/attention/enabled" => "Show attention badges when a workspace is waiting for you.",
        "/attention/blocked_regex" => {
            "One regular expression per line that marks output as blocked."
        }
        "/attention/debounce_ms" => "Wait this long before raising a new attention signal.",
        "/ui/tab_bar_position" => "Place the workspace tab bar above or below the terminal.",
        "/ui/tab_bar_height" => "Height of the compact tab bar in pixels.",
        "/ui/show_title_bar" => "Keep the native window title visible.",
        "/ui/borderless" => "Remove the outer window border when supported by the desktop.",
        "/ssh/host" => "Fallback SSH host used by remote connections.",
        "/ssh/port" => "TCP port used for the default SSH connection.",
        "/ssh/user" => "Remote user name; empty uses the current local user.",
        "/ssh/key_path" => "Private key path; empty allows ssh-agent to provide credentials.",
        "/behavior/on_last_pane_exit" => "Choose what remains after the final pane closes.",
        "/behavior/on_program_exit_abnormal" => {
            "Choose how Muxterm handles a non-zero command exit."
        }
        "/platform/linux/client_side_decorations" => "Let Muxterm draw its own window controls.",
        "/platform/macos/option_as_alt" => "Use the Option key as an Alt modifier on macOS.",
        "/shortcuts/preset" => "Start from a QWERTY or Colemak action layout.",
        "/shortcuts/primary_key" => "Modifier used for the primary shortcut set.",
        "/projects" => "Reusable workspace launch profiles shared by Quick Connect.",
        "/shortcuts/overrides" => "Override or disable individual action bindings.",
        _ => "Configure this setting for new Muxterm sessions.",
    }
}

fn apply_label(mode: &str) -> &'static str {
    match mode {
        "immediate" => "LIVE",
        "next_workspace" => "NEXT WORKSPACE",
        _ => "ON SAVE",
    }
}

fn option_label(path: &str, value: &str) -> String {
    match (path, value) {
        ("/theme/name", "system") => "Follow system".into(),
        ("/theme/name", "black") => "Black".into(),
        ("/theme/name", "white") => "White".into(),
        ("/theme/light", "white") => "White".into(),
        ("/theme/light", "black") => "Black".into(),
        ("/theme/dark", "white") => "White".into(),
        ("/theme/dark", "black") => "Black".into(),
        ("/statusbar/mode", "tmux") => "Match tmux".into(),
        ("/statusbar/mode", "theme") => "Use Muxterm theme".into(),
        ("/ui/tab_bar_position", "top") => "Top".into(),
        ("/ui/tab_bar_position", "bottom") => "Bottom".into(),
        ("/behavior/on_last_pane_exit", "close_window") => "Close the window".into(),
        ("/behavior/on_last_pane_exit", "keep_empty") => "Keep an empty window".into(),
        ("/behavior/on_last_pane_exit", "new_shell") => "Open a new shell".into(),
        ("/behavior/on_program_exit_abnormal", "notify") => "Keep and notify".into(),
        ("/behavior/on_program_exit_abnormal", "close") => "Close the pane".into(),
        ("/behavior/on_program_exit_abnormal", "keep") => "Keep the pane".into(),
        ("/shortcuts/primary_key", "auto") => "Automatic".into(),
        ("/shortcuts/primary_key", "alt") => "Alt".into(),
        ("/shortcuts/primary_key", "command") => "Command".into(),
        ("/shortcuts/primary_key", "control") => "Control".into(),
        ("/shortcuts/primary_key", "super") => "Super".into(),
        _ => humanize_words(value),
    }
}

fn input_placeholder(path: &str) -> &'static str {
    match path {
        "/tmux/default_session" => "workspace name",
        "/tmux/socket" => "default socket",
        "/pane/default_command" => "$SHELL",
        "/pane/workdir" => "$HOME",
        "/ssh/host" => "example.com",
        "/ssh/user" => "optional",
        "/ssh/key_path" => "~/.ssh/id_ed25519",
        _ => "",
    }
}

fn humanize_words(raw: &str) -> String {
    raw.split(['.', '/', '_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn category_icon(id: &str) -> &'static str {
    match id {
        "appearance" => "Aa",
        "runtime" => "▣",
        "attention" => "◉",
        "ui" => "▤",
        "ssh" => "↗",
        "behavior" => "↯",
        "platform" => "⌘",
        "projects" => "▦",
        "shortcuts" => "⌨",
        _ => "•",
    }
}

fn category_hint(id: &str) -> &'static str {
    match id {
        "appearance" => "Fonts & colors",
        "runtime" => "Workspaces",
        "attention" => "Agent signals",
        "ui" => "Window chrome",
        "ssh" => "Remote access",
        "behavior" => "Exit rules",
        "platform" => "Desktop specific",
        "projects" => "Launch profiles",
        "shortcuts" => "Keyboard",
        _ => "General",
    }
}

fn category_description(id: &str) -> &'static str {
    match id {
        "appearance" => "Tune the terminal you look at all day: type, scale, and color.",
        "runtime" => "Set defaults for new workspaces, panes, and terminal history.",
        "attention" => "Decide when Muxterm should surface work that needs your attention.",
        "ui" => "Shape the surrounding window chrome and tab bar.",
        "ssh" => "Defaults used when opening remote workspaces over SSH.",
        "behavior" => "Choose what Muxterm does when panes or commands exit.",
        "platform" => "Options specific to the desktop platform you are running on.",
        "projects" => "Save the workspaces you return to most often.",
        "shortcuts" => "Choose a keyboard preset and customize individual actions.",
        _ => "Configure this part of Muxterm.",
    }
}

fn appearance_preview(values: &Value) -> GtkBox {
    let preview = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();
    preview.add_css_class("prefs-preview-card");

    let header = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let title = Label::new(Some("Terminal preview"));
    title.set_halign(Align::Start);
    title.add_css_class("prefs-preview-title");
    header.append(&title);
    let live = Label::new(Some("PREVIEW"));
    live.set_halign(Align::End);
    live.set_hexpand(true);
    live.add_css_class("prefs-apply-badge");
    header.append(&live);
    preview.append(&header);

    let terminal = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(16)
        .margin_end(16)
        .margin_bottom(16)
        .build();
    terminal.add_css_class("prefs-terminal-preview");
    let dots = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(5)
        .build();
    for (dot, color) in [("●", "red"), ("●", "yellow"), ("●", "green")] {
        let label = Label::new(Some(dot));
        label.add_css_class("prefs-preview-dot");
        label.add_css_class(&format!("prefs-preview-dot-{color}"));
        dots.append(&label);
    }
    terminal.append(&dots);
    let prompt = Label::new(Some("$ muxterm  --workspace ready"));
    prompt.set_halign(Align::Start);
    prompt.add_css_class("prefs-preview-prompt");
    terminal.append(&prompt);
    let output = Label::new(Some("Connected  ·  2 panes  ·  waiting for input"));
    output.set_halign(Align::Start);
    output.add_css_class("prefs-preview-output");
    terminal.append(&output);
    let family = pointer(values, "/font/family")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("JetBrains Mono");
    let size = pointer(values, "/font/size")
        .and_then(Value::as_f64)
        .unwrap_or(13.0);
    let theme = pointer(values, "/theme/name")
        .and_then(Value::as_str)
        .map(|value| option_label("/theme/name", value))
        .unwrap_or_else(|| "Follow system".into());
    let summary = Label::new(Some(&format!("{family}  ·  {size:.1} pt  ·  {theme}")));
    summary.set_halign(Align::Start);
    summary.add_css_class("prefs-preview-summary");
    preview.append(&terminal);
    preview.append(&summary);
    preview
}

const PREFERENCES_CSS: &str = r#"
.muxterm-preferences-window .prefs-root {
    background-color: @theme_bg_color;
    color: @theme_fg_color;
}
.muxterm-preferences-window .prefs-header-mark {
    min-width: 42px;
    min-height: 42px;
    padding: 0;
    border-radius: 12px;
    background-color: alpha(@theme_selected_bg_color, 0.22);
    color: @theme_selected_bg_color;
    font-size: 22px;
    font-weight: 700;
}
.muxterm-preferences-window .prefs-header-title {
    font-size: 22px;
    font-weight: 700;
}
.muxterm-preferences-window .prefs-header-subtitle {
    color: alpha(@theme_fg_color, 0.72);
    font-size: 12px;
}
.muxterm-preferences-window .prefs-config-path {
    color: alpha(@theme_fg_color, 0.46);
    font-size: 10px;
}
.muxterm-preferences-window .prefs-search {
    min-height: 34px;
    padding-left: 10px;
    padding-right: 10px;
    border-radius: 9px;
}
.muxterm-preferences-window .prefs-divider {
    background-color: alpha(@theme_fg_color, 0.12);
    min-width: 1px;
    min-height: 1px;
}
.muxterm-preferences-window .prefs-sidebar {
    background-color: alpha(@theme_fg_color, 0.025);
}
.muxterm-preferences-window .prefs-sidebar-label {
    margin-left: 10px;
    color: alpha(@theme_fg_color, 0.48);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
}
.muxterm-preferences-window .prefs-category-scroll,
.muxterm-preferences-window .muxterm-prefs-categories {
    background-color: transparent;
    border: none;
}
.muxterm-preferences-window .muxterm-prefs-category-row {
    min-height: 54px;
    margin: 2px 0;
    padding: 0;
    border-radius: 9px;
    background-color: transparent;
}
.muxterm-preferences-window .muxterm-prefs-category-row:hover {
    background-color: alpha(@theme_fg_color, 0.07);
}
.muxterm-preferences-window .muxterm-prefs-category-row:selected {
    background-color: alpha(@theme_selected_bg_color, 0.16);
    box-shadow: inset 3px 0 0 @theme_selected_bg_color;
}
.muxterm-preferences-window .prefs-nav-icon {
    color: alpha(@theme_fg_color, 0.58);
    font-size: 15px;
    font-weight: 700;
}
.muxterm-preferences-window row:selected .prefs-nav-icon {
    color: @theme_selected_bg_color;
}
.muxterm-preferences-window .prefs-nav-title {
    color: @theme_fg_color;
    font-size: 12px;
    font-weight: 600;
}
.muxterm-preferences-window .prefs-nav-hint {
    color: alpha(@theme_fg_color, 0.48);
    font-size: 10px;
}
.muxterm-preferences-window .prefs-page-title {
    font-size: 25px;
    font-weight: 700;
}
.muxterm-preferences-window .prefs-page-description {
    color: alpha(@theme_fg_color, 0.64);
    font-size: 12px;
}
.muxterm-preferences-window .prefs-card,
.muxterm-preferences-window .prefs-preview-card {
    border: 1px solid alpha(@theme_fg_color, 0.13);
    border-radius: 12px;
    background-color: alpha(@theme_fg_color, 0.035);
}
.muxterm-preferences-window .prefs-card-title,
.muxterm-preferences-window .prefs-preview-title {
    font-size: 13px;
    font-weight: 700;
}
.muxterm-preferences-window .prefs-card-hint,
.muxterm-preferences-window .prefs-preview-summary {
    color: alpha(@theme_fg_color, 0.48);
    font-size: 10px;
}
.muxterm-preferences-window .prefs-setting-row {
    padding: 14px 16px;
}
.muxterm-preferences-window .prefs-setting-title {
    font-size: 12px;
    font-weight: 600;
}
.muxterm-preferences-window .prefs-setting-description {
    color: alpha(@theme_fg_color, 0.58);
    font-size: 11px;
}
.muxterm-preferences-window .prefs-apply-badge {
    min-height: 18px;
    padding: 2px 6px;
    border-radius: 5px;
    background-color: alpha(@theme_selected_bg_color, 0.13);
    color: alpha(@theme_selected_bg_color, 0.88);
    font-size: 9px;
    font-weight: 700;
}
.muxterm-preferences-window .prefs-card-divider {
    background-color: alpha(@theme_fg_color, 0.10);
    min-height: 1px;
}
.muxterm-preferences-window entry.prefs-control,
.muxterm-preferences-window spinbutton.prefs-control,
.muxterm-preferences-window combobox.prefs-control,
.muxterm-preferences-window fontbutton.prefs-control {
    min-height: 32px;
}
.muxterm-preferences-window .prefs-text-editor {
    min-height: 104px;
    border: 1px solid alpha(@theme_fg_color, 0.14);
    border-radius: 8px;
    background-color: alpha(@theme_fg_color, 0.025);
}
.muxterm-preferences-window .prefs-text-editor:focus-within {
    border-color: alpha(@theme_selected_bg_color, 0.72);
}
.muxterm-preferences-window .prefs-footer {
    min-height: 58px;
    padding-top: 13px;
    padding-bottom: 13px;
    border-top: 1px solid alpha(@theme_fg_color, 0.12);
    background-color: alpha(@theme_fg_color, 0.025);
}
.muxterm-preferences-window .prefs-footer-status {
    color: alpha(@theme_fg_color, 0.55);
    font-size: 11px;
}
.muxterm-preferences-window .prefs-footer-status.error {
    color: #d94841;
}
.muxterm-preferences-window .prefs-secondary-action {
    min-width: 86px;
}
.muxterm-preferences-window .prefs-subwindow {
    background-color: @theme_bg_color;
}
.muxterm-preferences-window .prefs-subwindow-scroll {
    border: 1px solid alpha(@theme_fg_color, 0.13);
    border-radius: 12px;
    background-color: alpha(@theme_fg_color, 0.035);
}
.muxterm-preferences-window .prefs-list-card,
.muxterm-preferences-window .prefs-shortcut-row,
.muxterm-preferences-window .prefs-project-row {
    background-color: transparent;
}
.muxterm-preferences-window .prefs-project-name,
.muxterm-preferences-window .prefs-shortcut-name {
    font-size: 12px;
    font-weight: 600;
}
.muxterm-preferences-window .prefs-project-detail {
    color: alpha(@theme_fg_color, 0.48);
    font-size: 10px;
}
.muxterm-preferences-window .prefs-keycap {
    min-width: 92px;
    padding: 5px 8px;
    border-radius: 6px;
    background-color: alpha(@theme_fg_color, 0.08);
    color: alpha(@theme_fg_color, 0.72);
    font-family: monospace;
    font-size: 10px;
}
.muxterm-preferences-window .prefs-inline-action {
    min-width: 70px;
}
.muxterm-preferences-window .prefs-preview-card {
    padding-bottom: 1px;
}
.muxterm-preferences-window .prefs-terminal-preview {
    padding: 12px;
    border-radius: 8px;
    background-color: #10141c;
    color: #d8dee9;
    font-family: monospace;
}
.muxterm-preferences-window .prefs-preview-dot {
    font-size: 10px;
}
.muxterm-preferences-window .prefs-preview-dot-red { color: #ff6b6b; }
.muxterm-preferences-window .prefs-preview-dot-yellow { color: #ffd166; }
.muxterm-preferences-window .prefs-preview-dot-green { color: #67e8a5; }
.muxterm-preferences-window .prefs-preview-prompt {
    color: #8be9fd;
    font-size: 11px;
}
.muxterm-preferences-window .prefs-preview-output {
    color: #a9b4c6;
    font-size: 11px;
}
"#;

fn install_preferences_css() {
    thread_local! {
        static PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    }
    PROVIDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let css = slot.get_or_insert_with(|| {
            let provider = CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk4::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            provider
        });
        css.load_from_data(PREFERENCES_CSS);
    });
}

fn category_title(id: &str, title_key: &str) -> String {
    let raw = title_key
        .strip_prefix("settings.")
        .filter(|title| !title.is_empty())
        .unwrap_or(id);
    raw.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    fn category_title_humanizes_manifest_key() {
        assert_eq!(
            category_title("appearance", "settings.appearance"),
            "Appearance"
        );
        assert_eq!(category_title("tab_bar", ""), "Tab Bar");
    }

    #[test]
    fn setting_labels_are_human_readable() {
        assert_eq!(field_title("/font/size", "settings.font.size"), "Font size");
        assert_eq!(
            field_title("/tmux/default_session", ""),
            "Default workspace"
        );
        assert_eq!(option_label("/theme/name", "system"), "Follow system");
        assert_eq!(apply_label("next_workspace"), "NEXT WORKSPACE");
    }

    #[test]
    fn number_ranges_do_not_truncate_valid_pool_values() {
        let default_spec = number_spec("/pool/max_slots", None);
        assert_eq!(default_spec.1, f64::from(u32::MAX));

        let configured = Value::from(5_000_u64);
        assert_eq!(number_spec("/pool/max_slots", Some(&configured)).1, 5_000.0);
    }

    #[test]
    fn integer_number_controls_start_clean() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("skip: 无 DISPLAY");
            return;
        }
        gtk4::test_synced(|| {
            let spin = SpinButton::with_range(1.0, 100.0, 1.0);
            spin.set_value(20.0);
            let control = tracked_number("/pool/max_slots".into(), spin, true);
            assert!(!control.is_changed());
        });
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
