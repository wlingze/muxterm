//! Schema-driven Preferences 字段控件与字段文案策略。
//!
//! 这里只负责把 Settings Manifest 字段投影成 GTK 控件，并记录打开设置页
//! 时的 baseline；draft transaction 和页面导航仍由父设置页持有。

use crate::frontend::utils::i18n::{tr_static as text, Key as TextKey};
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
            let widget = Button::with_label(text(TextKey::SettingsManageProjects));
            widget.set_widget_name(&widget_name);
            widget.set_halign(Align::End);
            widget.add_css_class("prefs-control");
            control_slot.append(&widget);
            content.append(&control_slot);
            Some(tracked_field(path, ControlKind::Summary(widget)))
        }
        "shortcut_editor" => {
            let widget = Button::with_label(text(TextKey::SettingsManageShortcuts));
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
        "/ui/tab_bar_style" => {
            crate::frontend::utils::i18n::tr(crate::frontend::utils::i18n::Key::TabBarStyle)
        }
        "/font/family" => text(TextKey::SettingsFontFamily).into(),
        "/font/size" => text(TextKey::SettingsFontSize).into(),
        "/font/fallback" => text(TextKey::SettingsFallbackFonts).into(),
        "/theme/name" => text(TextKey::SettingsTheme).into(),
        "/theme/light" => text(TextKey::SettingsLightTheme).into(),
        "/theme/dark" => text(TextKey::SettingsDarkTheme).into(),
        "/statusbar/mode" => text(TextKey::SettingsStatusBarAppearance).into(),
        "/tmux/auto_mouse" => text(TextKey::SettingsEnableTmuxMouseMode).into(),
        "/tmux/default_session" => text(TextKey::SettingsDefaultWorkspace).into(),
        "/tmux/socket" => text(TextKey::SettingsTmuxSocket).into(),
        "/pool/max_slots" => text(TextKey::SettingsWorkspaceReminderLimit).into(),
        "/scrollback/lines" => text(TextKey::SettingsScrollbackLines).into(),
        "/pane/default_command" => text(TextKey::SettingsDefaultShellCommand).into(),
        "/pane/workdir" => text(TextKey::SettingsInitialWorkingDirectory).into(),
        "/attention/enabled" => text(TextKey::SettingsWorkspaceAttention).into(),
        "/attention/blocked_regex" => text(TextKey::SettingsBlockedOutputPatterns).into(),
        "/attention/debounce_ms" => text(TextKey::SettingsNotificationDelay).into(),
        "/ui/tab_bar_position" => text(TextKey::SettingsTabBarPosition).into(),
        "/ui/tab_bar_height" => text(TextKey::SettingsTabBarHeight).into(),
        "/ui/show_title_bar" => text(TextKey::SettingsShowTitleBar).into(),
        "/ui/borderless" => text(TextKey::SettingsBorderlessWindow).into(),
        "/ssh/host" => text(TextKey::SettingsDefaultSshHost).into(),
        "/ssh/port" => text(TextKey::SettingsSshPort).into(),
        "/ssh/user" => text(TextKey::SettingsSshUser).into(),
        "/ssh/key_path" => text(TextKey::SettingsSshPrivateKey).into(),
        "/behavior/on_last_pane_exit" => text(TextKey::SettingsWhenTheLastPaneExits).into(),
        "/behavior/on_program_exit_abnormal" => text(TextKey::SettingsWhenACommandFails).into(),
        "/platform/linux/client_side_decorations" => {
            text(TextKey::SettingsClientSideDecorations).into()
        }
        "/platform/macos/option_as_alt" => text(TextKey::SettingsTreatOptionAsAlt).into(),
        "/shortcuts/preset" => text(TextKey::SettingsKeyboardLayout).into(),
        "/shortcuts/primary_key" => text(TextKey::SettingsPrimaryModifier).into(),
        "/projects" => text(TextKey::SettingsSavedProjects).into(),
        "/shortcuts/overrides" => text(TextKey::SettingsCustomShortcuts).into(),
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
        "/ui/tab_bar_style" => text(TextKey::SettingsTabLayoutSizing),
        "/font/family" => text(TextKey::SettingsTheTypefaceUsedToDrawTerminalText),
        "/font/size" => {
            text(TextKey::SettingsAdjustTheTerminalScaleWithoutChangingYourDisplaySettings)
        }
        "/font/fallback" => {
            text(TextKey::SettingsCommaSeparatedFontsUsedWhenThePrimaryFamilyIsMissingAGlyph)
        }
        "/theme/name" => text(TextKey::SettingsChooseAFixedThemeOrFollowYourSystemAppearance),
        "/theme/light" => text(TextKey::SettingsThemeUsedWhenTheSystemIsInLightMode),
        "/theme/dark" => text(TextKey::SettingsThemeUsedWhenTheSystemIsInDarkMode),
        "/statusbar/mode" => {
            text(TextKey::SettingsUseTmuxColorsOrKeepTheStatusBarInTheMuxtermTheme)
        }
        "/tmux/auto_mouse" => {
            text(TextKey::SettingsForwardMouseInteractionsToAttachedTmuxWorkspaces)
        }
        "/tmux/default_session" => {
            text(TextKey::SettingsWorkspaceToAttachOnLaunchLeaveEmptyToStartLocally)
        }
        "/tmux/socket" => text(TextKey::SettingsOptionalNamedTmuxSocketEmptyUsesTheDefaultServer),
        "/pool/max_slots" => text(TextKey::SettingsShowAReminderWhenThisManyWarmWorkspacesAreOpen),
        "/scrollback/lines" => text(TextKey::SettingsHistoryKeptForEachNewlyCreatedPane),
        "/pane/default_command" => text(TextKey::SettingsCommandStartedForANewLocalPane),
        "/pane/workdir" => text(TextKey::SettingsDirectoryUsedWhenANewLocalPaneStarts),
        "/attention/enabled" => {
            text(TextKey::SettingsShowAttentionBadgesWhenAWorkspaceIsWaitingForYou)
        }
        "/attention/blocked_regex" => {
            text(TextKey::SettingsOneRegularExpressionPerLineThatMarksOutputAsBlocked)
        }
        "/attention/debounce_ms" => {
            text(TextKey::SettingsWaitThisLongBeforeRaisingANewAttentionSignal)
        }
        "/ui/tab_bar_position" => {
            text(TextKey::SettingsPlaceTheWorkspaceTabBarAboveOrBelowTheTerminal)
        }
        "/ui/tab_bar_height" => text(TextKey::SettingsHeightOfTheCompactTabBarInPixels),
        "/ui/show_title_bar" => text(TextKey::SettingsKeepTheNativeWindowTitleVisible),
        "/ui/borderless" => {
            text(TextKey::SettingsRemoveTheOuterWindowBorderWhenSupportedByTheDesktop)
        }
        "/ssh/host" => text(TextKey::SettingsFallbackSshHostUsedByRemoteConnections),
        "/ssh/port" => text(TextKey::SettingsTcpPortUsedForTheDefaultSshConnection),
        "/ssh/user" => text(TextKey::SettingsRemoteUserNameEmptyUsesTheCurrentLocalUser),
        "/ssh/key_path" => {
            text(TextKey::SettingsPrivateKeyPathEmptyAllowsSshAgentToProvideCredentials)
        }
        "/behavior/on_last_pane_exit" => {
            text(TextKey::SettingsChooseWhatRemainsAfterTheFinalPaneCloses)
        }
        "/behavior/on_program_exit_abnormal" => {
            text(TextKey::SettingsChooseHowMuxtermHandlesANonZeroCommandExit)
        }
        "/platform/linux/client_side_decorations" => {
            text(TextKey::SettingsLetMuxtermDrawItsOwnWindowControls)
        }
        "/platform/macos/option_as_alt" => {
            text(TextKey::SettingsUseTheOptionKeyAsAnAltModifierOnMacos)
        }
        "/shortcuts/preset" => text(TextKey::SettingsStartFromAQwertyOrColemakActionLayout),
        "/shortcuts/primary_key" => text(TextKey::SettingsModifierUsedForThePrimaryShortcutSet),
        "/projects" => text(TextKey::SettingsReusableWorkspaceLaunchProfilesSharedByQuickConnect),
        "/shortcuts/overrides" => text(TextKey::SettingsOverrideOrDisableIndividualActionBindings),
        _ => text(TextKey::SettingsConfigureThisSettingForNewMuxtermSessions),
    }
}

pub(super) fn apply_label(mode: &str) -> &'static str {
    match mode {
        "immediate" => text(TextKey::SettingsLive),
        "next_workspace" => text(TextKey::SettingsNextWorkspace),
        _ => text(TextKey::SettingsOnSave),
    }
}

pub(super) fn option_label(path: &str, value: &str) -> String {
    use crate::frontend::utils::i18n::{self, Key};
    match (path, value) {
        ("/ui/tab_bar_style", "equal_width") => i18n::tr(Key::TabEqualWidth),
        ("/ui/tab_bar_style", "compact") => i18n::tr(Key::TabCompact),
        ("/theme/name", "system") => text(TextKey::SettingsFollowSystem).into(),
        ("/theme/name", "black") => text(TextKey::SettingsBlack).into(),
        ("/theme/name", "white") => text(TextKey::SettingsWhite).into(),
        ("/theme/light", "white") => text(TextKey::SettingsWhite).into(),
        ("/theme/light", "black") => text(TextKey::SettingsBlack).into(),
        ("/theme/dark", "white") => text(TextKey::SettingsWhite).into(),
        ("/theme/dark", "black") => text(TextKey::SettingsBlack).into(),
        ("/statusbar/mode", "tmux") => text(TextKey::SettingsMatchTmux).into(),
        ("/statusbar/mode", "theme") => text(TextKey::SettingsUseMuxtermTheme).into(),
        ("/ui/tab_bar_position", "top") => text(TextKey::SettingsTop).into(),
        ("/ui/tab_bar_position", "bottom") => text(TextKey::SettingsBottom).into(),
        ("/behavior/on_last_pane_exit", "close_window") => {
            text(TextKey::SettingsCloseTheWindow).into()
        }
        ("/behavior/on_last_pane_exit", "keep_empty") => {
            text(TextKey::SettingsKeepAnEmptyWindow).into()
        }
        ("/behavior/on_last_pane_exit", "new_shell") => text(TextKey::SettingsOpenANewShell).into(),
        ("/behavior/on_program_exit_abnormal", "notify") => {
            text(TextKey::SettingsKeepAndNotify).into()
        }
        ("/behavior/on_program_exit_abnormal", "close") => {
            text(TextKey::SettingsCloseThePane).into()
        }
        ("/behavior/on_program_exit_abnormal", "keep") => text(TextKey::SettingsKeepThePane).into(),
        ("/shortcuts/primary_key", "auto") => text(TextKey::SettingsAutomatic).into(),
        ("/shortcuts/primary_key", "alt") => "Alt".into(),
        ("/shortcuts/primary_key", "command") => "Command".into(),
        ("/shortcuts/primary_key", "control") => "Control".into(),
        ("/shortcuts/primary_key", "super") => "Super".into(),
        _ => super::humanize_words(value),
    }
}

pub(super) fn input_placeholder(path: &str) -> &'static str {
    match path {
        "/tmux/default_session" => text(TextKey::SettingsWorkspaceName),
        "/tmux/socket" => text(TextKey::SettingsDefaultSocket),
        "/pane/default_command" => "$SHELL",
        "/pane/workdir" => "$HOME",
        "/ssh/host" => "example.com",
        "/ssh/user" => text(TextKey::SettingsOptional),
        "/ssh/key_path" => "~/.ssh/id_ed25519",
        _ => "",
    }
}
