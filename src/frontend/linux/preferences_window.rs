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
    Align, Box as GtkBox, Button, CssProvider, Label, ListBox, ListBoxRow, Orientation,
    ScrolledWindow, SearchEntry, Separator, Stack, Window,
};
use serde_json::Value;

#[cfg(test)]
use crate::frontend::ffi_client::FfiClient;
use crate::frontend::ffi_client::{
    ClientConfigSnapshot, ClientJsonPatchOperation, ClientRuntimeInfo,
};
use crate::frontend::i18n::{self, Key as TextKey};
pub use crate::frontend::linux::settings_model::ConfigApi;
#[cfg(test)]
use gtk4::SpinButton;

#[path = "preferences_projects.rs"]
mod preferences_projects;

#[path = "preferences_shortcuts.rs"]
mod preferences_shortcuts;

#[path = "preferences_fields.rs"]
mod preferences_fields;

#[cfg(test)]
use preferences_fields::{apply_label, number_spec, tracked_field, tracked_number};
use preferences_fields::{
    control_row, field_description, field_title, option_label, pointer, CategoryPage, ControlKind,
    FieldControl,
};

/// Open the settings window; `on_saved` fires after a committed Apply or an
/// external file change.
pub fn show(
    parent: &impl IsA<Window>,
    config_path: PathBuf,
    config: ConfigApi,
    snapshot: ClientConfigSnapshot,
    on_saved: Box<dyn Fn() + 'static>,
    project_editor: Option<(
        Vec<ClientRuntimeInfo>,
        Vec<crate::frontend::ffi_client::SshHostEntry>,
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
                let config = config.clone();
                let on_saved = on_saved.clone();
                let project_editor = project_editor.clone();
                let editor_window = editor_window.clone();
                if control.path == "/projects" {
                    button.connect_clicked(move |_| {
                        if let Some((runtimes, hosts)) = &project_editor {
                            show_project_manager(
                                &editor_window,
                                config_path.clone(),
                                config.clone(),
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
                            config.clone(),
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
        let config = config.clone();
        let controls = controls.clone();
        let allow_close = allow_close.clone();
        let save_status = save_status.clone();
        move |_| {
            let operations: Vec<ClientJsonPatchOperation> = controls
                .borrow()
                .iter()
                .filter_map(|control| control.value().map(|value| replace(&control.path, value)))
                .collect();
            if let Err(error) = config.apply(&operations) {
                tracing::error!(target = "muxterm::config", "保存设置失败: {error}");
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
        let config = config.clone();
        let on_saved = on_saved.clone();
        monitor.connect_changed(move |_, _, _, _| match config.reload() {
            Ok(_) => on_saved(),
            Err(error) => tracing::warn!(
                target = "muxterm::config",
                "外部配置变更重新加载失败: {error}"
            ),
        });
    }

    win.present();
    win
}

fn replace(path: &str, value: Value) -> ClientJsonPatchOperation {
    ClientJsonPatchOperation {
        op: "replace".into(),
        path: path.into(),
        value: Some(value),
    }
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
    config: ConfigApi,
    runtimes: Vec<ClientRuntimeInfo>,
    hosts: Vec<crate::frontend::ffi_client::SshHostEntry>,
    on_changed: Rc<Box<dyn Fn() + 'static>>,
) {
    preferences_projects::show_project_manager(
        parent,
        config_path,
        config,
        runtimes,
        hosts,
        on_changed,
    );
}

fn show_shortcut_manager(
    app: &impl IsA<Window>,
    config_path: PathBuf,
    config: ConfigApi,
    on_changed: Rc<Box<dyn Fn() + 'static>>,
) {
    preferences_shortcuts::show_shortcut_manager(app, config_path, config, on_changed);
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
    let title = section_title(id, title_key);
    let label = Label::new(Some(&title));
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
        assert!(number_spec("/pool/max_slots", Some(&configured)).1 >= 5_000.0);
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
            let client = Rc::new(RefCell::new(
                FfiClient::new_catalog().expect("catalog FFI handle"),
            ));
            let config = ConfigApi::from_client(client);
            let snapshot = config.describe().expect("config snapshot");
            let win = show(
                &parent,
                PathBuf::from("/tmp/nonexistent-config.toml"),
                config,
                snapshot,
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
