//! Schema-backed Linux settings window.
//!
//! Generic fields are rendered from the Core Settings Manifest: a new Core
//! field only needs a Manifest entry and this window shows and saves it without
//! platform business logic. Projects and Shortcuts keep summary sections; their
//! full editors are out of scope for this pass.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, CssProvider, Label, Orientation, Window};
use serde_json::Value;

#[cfg(test)]
use crate::frontend::ffi_client::FfiClient;
use crate::frontend::ffi_client::{
    ClientConfigSnapshot, ClientJsonPatchOperation, ClientRuntimeInfo,
};
pub use crate::frontend::linux::settings_model::ConfigApi;
#[cfg(test)]
use gtk4::SpinButton;

#[path = "preferences_projects.rs"]
mod preferences_projects;

#[path = "preferences_shortcuts.rs"]
mod preferences_shortcuts;

#[path = "preferences_fields.rs"]
mod preferences_fields;

#[path = "preferences_window_ui.rs"]
mod preferences_window_ui;

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
    preferences_window_ui::show(
        parent,
        config_path,
        config,
        snapshot,
        on_saved,
        project_editor,
    )
}

fn replace(path: &str, value: Value) -> ClientJsonPatchOperation {
    ClientJsonPatchOperation {
        op: "replace".into(),
        path: path.into(),
        value: Some(value),
    }
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
