//! Linux command palette model and action catalog.
//!
//! The model is GTK-free. The window module only renders these commands and
//! forwards the selected stable action id to the caller.

use crate::i18n::{self, Key as TextKey};
use crate::linux::quick_pick::fuzzy_match;

pub const TMUX_DETACH_COMMAND: &str = "tmux_detach";

/// Command palette id mapped to a product action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    TmuxAttach,
    TmuxNew,
    TmuxDetach,
    SshConnect,
    SshDisconnect,
    NewTab,
    NewPane,
    NewPaneVertical,
    ClosePane,
    CloseTab,
    CloseWindow,
    SwitchTab(usize),
    SwitchPanePrev,
    SwitchPaneNext,
    SearchPanes,
    RenamePane,
    ReloadConfig,
    OpenConfig,
    Preferences,
    Language,
    QuickConnect,
    TogglePaneFullscreen,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ToggleTheme,
    ToggleStatusBarMode,
    Quit,
}

/// Parse a stable command palette id into a product action.
pub fn parse_palette_action(id: &str) -> Option<PaletteAction> {
    Some(match id {
        "tmux_attach" => PaletteAction::TmuxAttach,
        "tmux_new" => PaletteAction::TmuxNew,
        TMUX_DETACH_COMMAND => PaletteAction::TmuxDetach,
        "ssh_connect" => PaletteAction::SshConnect,
        "ssh_disconnect" => PaletteAction::SshDisconnect,
        "new_tab" | "new_window" => PaletteAction::NewTab,
        "new_pane" => PaletteAction::NewPane,
        "new_pane_vertical" => PaletteAction::NewPaneVertical,
        "close_pane" => PaletteAction::ClosePane,
        "close_tab" => PaletteAction::CloseTab,
        "close_window" => PaletteAction::CloseWindow,
        "switch_pane_prev" => PaletteAction::SwitchPanePrev,
        "switch_pane_next" => PaletteAction::SwitchPaneNext,
        "search_panes" => PaletteAction::SearchPanes,
        "rename_pane" => PaletteAction::RenamePane,
        "reload_config" => PaletteAction::ReloadConfig,
        "open_config" => PaletteAction::OpenConfig,
        "preferences" => PaletteAction::Preferences,
        "language" => PaletteAction::Language,
        "quick_connect" => PaletteAction::QuickConnect,
        "toggle_pane_fullscreen" => PaletteAction::TogglePaneFullscreen,
        "increase_font_size" => PaletteAction::IncreaseFontSize,
        "decrease_font_size" => PaletteAction::DecreaseFontSize,
        "reset_font_size" => PaletteAction::ResetFontSize,
        "theme" => PaletteAction::ToggleTheme,
        "statusbar_mode" => PaletteAction::ToggleStatusBarMode,
        "quit" => PaletteAction::Quit,
        id if id.starts_with("switch_tab_") => {
            let n = id.trim_start_matches("switch_tab_").parse().ok()?;
            PaletteAction::SwitchTab(n)
        }
        _ => return None,
    })
}

/// A command shown in the palette.
#[derive(Debug, Clone)]
pub struct PaletteCommand {
    pub id: &'static str,
    pub label: String,
}

/// Return the complete command catalog.
pub fn core_commands() -> Vec<PaletteCommand> {
    core_commands_with("Light", "theme")
}

/// Return the command catalog with the next theme/status labels interpolated.
pub fn core_commands_with(next_theme: &str, next_status_mode: &str) -> Vec<PaletteCommand> {
    let mut commands = core_command_list();
    for command in &mut commands {
        match command.id {
            "theme" => {
                command.label = i18n::tr_args(TextKey::ThemeSwitchTo, &[("theme", next_theme)]);
            }
            "statusbar_mode" => {
                command.label = i18n::tr_args(
                    TextKey::StatusBarModeSwitchTo,
                    &[("mode", next_status_mode)],
                );
            }
            _ => {}
        }
    }
    commands
}

fn core_command_list() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            id: "tmux_attach",
            label: i18n::tr(TextKey::CmdTmuxAttach),
        },
        PaletteCommand {
            id: "tmux_new",
            label: i18n::tr(TextKey::CmdTmuxNew),
        },
        PaletteCommand {
            id: TMUX_DETACH_COMMAND,
            label: i18n::tr(TextKey::CmdTmuxDetach),
        },
        PaletteCommand {
            id: "ssh_connect",
            label: i18n::tr(TextKey::CmdSshConnect),
        },
        PaletteCommand {
            id: "ssh_disconnect",
            label: i18n::tr(TextKey::CmdSshDisconnect),
        },
        PaletteCommand {
            id: "new_tab",
            label: i18n::tr(TextKey::NewTab),
        },
        PaletteCommand {
            id: "new_pane",
            label: i18n::tr(TextKey::CmdNewPane),
        },
        PaletteCommand {
            id: "new_pane_vertical",
            label: i18n::tr(TextKey::CmdNewPaneVertical),
        },
        PaletteCommand {
            id: "close_pane",
            label: i18n::tr(TextKey::ClosePane),
        },
        PaletteCommand {
            id: "close_tab",
            label: i18n::tr(TextKey::CloseTab),
        },
        PaletteCommand {
            id: "close_window",
            label: i18n::tr(TextKey::CloseWindow),
        },
        PaletteCommand {
            id: "switch_tab_1",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "1")]),
        },
        PaletteCommand {
            id: "switch_tab_2",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "2")]),
        },
        PaletteCommand {
            id: "switch_tab_3",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "3")]),
        },
        PaletteCommand {
            id: "switch_tab_4",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "4")]),
        },
        PaletteCommand {
            id: "switch_tab_5",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "5")]),
        },
        PaletteCommand {
            id: "switch_tab_6",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "6")]),
        },
        PaletteCommand {
            id: "switch_tab_7",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "7")]),
        },
        PaletteCommand {
            id: "switch_tab_8",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "8")]),
        },
        PaletteCommand {
            id: "switch_tab_9",
            label: i18n::tr_args(TextKey::CmdSwitchTab, &[("number", "9")]),
        },
        PaletteCommand {
            id: "switch_pane_prev",
            label: i18n::tr(TextKey::CmdSwitchPanePrevious),
        },
        PaletteCommand {
            id: "switch_pane_next",
            label: i18n::tr(TextKey::CmdSwitchPaneNext),
        },
        PaletteCommand {
            id: "search_panes",
            label: i18n::tr(TextKey::CmdSearchPanes),
        },
        PaletteCommand {
            id: "rename_pane",
            label: i18n::tr(TextKey::CmdRenamePane),
        },
        PaletteCommand {
            id: "reload_config",
            label: i18n::tr(TextKey::CmdReloadConfig),
        },
        PaletteCommand {
            id: "open_config",
            label: i18n::tr(TextKey::CmdOpenConfig),
        },
        PaletteCommand {
            id: "preferences",
            label: i18n::tr(TextKey::CmdPreferences),
        },
        PaletteCommand {
            id: "language",
            label: i18n::tr(TextKey::Language),
        },
        PaletteCommand {
            id: "quick_connect",
            label: i18n::tr(TextKey::CmdQuickConnect),
        },
        PaletteCommand {
            id: "toggle_pane_fullscreen",
            label: i18n::tr(TextKey::TogglePaneFullscreen),
        },
        PaletteCommand {
            id: "increase_font_size",
            label: i18n::tr(TextKey::MenuIncreaseFontSize),
        },
        PaletteCommand {
            id: "decrease_font_size",
            label: i18n::tr(TextKey::MenuDecreaseFontSize),
        },
        PaletteCommand {
            id: "reset_font_size",
            label: i18n::tr(TextKey::MenuResetFontSize),
        },
        PaletteCommand {
            id: "theme",
            label: i18n::tr(TextKey::ThemeSwitchTo),
        },
        PaletteCommand {
            id: "statusbar_mode",
            label: i18n::tr(TextKey::StatusBarModeSwitchTo),
        },
        PaletteCommand {
            id: "quit",
            label: i18n::tr(TextKey::QuitMuxterm),
        },
    ]
}

/// Filter commands according to the backend capabilities.
pub fn commands_for_runtime(uses_tmux: bool) -> Vec<PaletteCommand> {
    commands_for_runtime_with(uses_tmux, "Light", "theme")
}

pub fn commands_for_runtime_with(
    uses_tmux: bool,
    next_theme: &str,
    next_status_mode: &str,
) -> Vec<PaletteCommand> {
    let mut commands = core_commands_with(next_theme, next_status_mode);
    if !uses_tmux {
        commands.retain(|command| command.id != TMUX_DETACH_COMMAND);
    }
    commands
}

/// Filter the command catalog by a localized label or stable English id.
pub fn filter_commands(query: &str) -> Vec<PaletteCommand> {
    core_commands()
        .into_iter()
        .filter(|command| fuzzy_match(query, &command.label) || fuzzy_match(query, command.id))
        .collect()
}
