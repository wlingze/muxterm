//! Schema-driven Preferences 字段控件与字段文案策略。
//!
//! 这里只负责把 Settings Manifest 字段投影成 GTK 控件，并记录打开设置页
//! 时的 baseline；draft transaction 和页面导航仍由父设置页持有。

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, ComboBoxText, Entry, Label, ScrolledWindow, SpinButton, TextView,
};
use serde_json::Value;

pub(super) enum ControlKind {
    Switch(gtk4::Switch),
    Number(SpinButton),
    Text(Entry),
    StringList(Entry),
    MultiLine(TextView),
    Select(ComboBoxText),
    FontPicker(gtk4::FontButton),
    Summary(Button),
}

pub(super) struct FieldControl {
    pub(super) path: String,
    pub(super) kind: ControlKind,
    baseline: Option<Value>,
    integer: bool,
}

pub(super) struct CategoryPage {
    pub(super) id: String,
    pub(super) navigation_index: i32,
    pub(super) fields: Vec<(glib::WeakRef<GtkBox>, String)>,
}

impl FieldControl {
    pub(super) fn value(&self) -> Option<Value> {
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
    pub(super) fn is_changed(&self) -> bool {
        self.value() != self.baseline
    }
}

pub(super) fn tracked_field(path: String, kind: ControlKind) -> FieldControl {
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
pub(super) fn tracked_number(path: String, widget: SpinButton, integer: bool) -> FieldControl {
    let mut control = FieldControl {
        path,
        kind: ControlKind::Number(widget),
        baseline: None,
        integer,
    };
    control.baseline = control.value();
    control
}

pub(super) fn widget_name_for(path: &str) -> String {
    // e2e 契约：muxterm-prefs-<dotted path>（`/font/size` → font-size）。
    let dotted = path.trim_start_matches('/').replace(['/', '~'], "-");
    format!("muxterm-prefs-{dotted}")
}

pub(super) fn control_row(field: &Value, values: &Value) -> (GtkBox, Option<FieldControl>) {
    let path = field["path"].as_str().unwrap_or_default().to_string();
    let widget_name = widget_name_for(&path);
    let control = field["control"].as_str().unwrap_or("text");
    let title = field_title(&path, field["title_key"].as_str().unwrap_or(""));
    let description = field_description(&path);
    let apply = apply_label(field["apply"].as_str().unwrap_or("commit"));
    let current = pointer(values, &format!("/{}", path.trim_start_matches('/')));
    let row = GtkBox::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(9)
        .build();
    row.add_css_class("prefs-setting-row");

    let content = GtkBox::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(18)
        .hexpand(true)
        .build();
    let copy = GtkBox::builder()
        .orientation(gtk4::Orientation::Vertical)
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
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(0)
        .build();
    control_slot.set_size_request(208, -1);
    let control_spacer = GtkBox::new(gtk4::Orientation::Horizontal, 0);
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

pub(super) fn pointer<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for token in path.trim_start_matches('/').split('/') {
        current = current.get(token)?;
    }
    Some(current)
}

pub(super) fn number_spec(path: &str, current: Option<&Value>) -> (f64, f64, f64, u32) {
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

pub(super) fn field_title(path: &str, title_key: &str) -> String {
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
            super::humanize_words(raw)
        }
    }
}

pub(super) fn field_description(path: &str) -> &'static str {
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

pub(super) fn apply_label(mode: &str) -> &'static str {
    match mode {
        "immediate" => "LIVE",
        "next_workspace" => "NEXT WORKSPACE",
        _ => "ON SAVE",
    }
}

pub(super) fn option_label(path: &str, value: &str) -> String {
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
        _ => super::humanize_words(value),
    }
}

pub(super) fn input_placeholder(path: &str) -> &'static str {
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
