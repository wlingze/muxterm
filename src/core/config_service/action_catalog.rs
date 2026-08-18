//! Core Action Catalog and shortcut presets.
//!
//! Menus, the command palette and platform key maps all use these stable Action
//! IDs. Presets are expressed with physical key positions so QWERTY and Colemak
//! users keep the same hand position; the platform layer renders the local
//! label.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::core::config::default_keybindings;
use crate::core::config_service::schema::ShortcutBinding;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActionDescriptor {
    pub id: &'static str,
    pub title_key: String,
    pub help_key: String,
    pub scope: &'static str,
    pub platforms: Vec<&'static str>,
    pub repeat_allowed: bool,
    pub default_bindings: Vec<ShortcutBinding>,
}

/// The full, Core-owned action table. Platform code renders and dispatches this
/// list; it never maintains a second action catalog.
pub fn action_catalog() -> Vec<ActionDescriptor> {
    let mut groups: BTreeMap<&'static str, Vec<ShortcutBinding>> = BTreeMap::new();
    for binding in default_keybindings() {
        groups
            .entry(Box::leak(binding.action.clone().into_boxed_str()))
            .or_default()
            .push(ShortcutBinding {
                key: binding.key,
                modifiers: binding.mods,
            });
    }
    groups
        .into_iter()
        .map(|(id, bindings)| ActionDescriptor {
            id,
            title_key: format!("action.{id}.title"),
            help_key: format!("action.{id}.help"),
            scope: "global",
            platforms: vec!["linux", "macos"],
            repeat_allowed: true,
            default_bindings: bindings,
        })
        .collect()
}

/// Physical QWERTY key -> Colemak key mapping. Keys not listed keep the same
/// physical position (digits, brackets, punctuation and function keys).
fn colemak_key_map() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("n", "k"),
        ("t", "g"),
        ("s", "r"),
        ("d", "s"),
        ("r", "p"),
        ("p", "semicolon"),
    ])
}

/// Preset bindings expressed in physical key positions.
pub fn preset_bindings(_preset: &str) -> Vec<ShortcutBinding> {
    let map = colemak_key_map();
    default_keybindings()
        .into_iter()
        .map(|binding| ShortcutBinding {
            key: map
                .get(binding.key.as_str())
                .map(|key| (*key).to_string())
                .unwrap_or(binding.key),
            modifiers: binding.mods,
        })
        .collect()
}

/// Resolve the configured primary modifier. `auto` means Alt on Linux and
/// Command on macOS; an explicit value is used verbatim on every platform.
pub fn primary_modifier(primary_key: &str, is_macos: bool) -> &'static str {
    match primary_key.trim().to_ascii_lowercase().as_str() {
        "command" => "command",
        "control" => "control",
        "super" => "super",
        "alt" => "alt",
        _ if is_macos => "command",
        _ => "alt",
    }
}

/// Preset bindings with the primary Alt modifier replaced by the configured
/// primary key. Non-primary chords (Ctrl+Shift copy/paste, Ctrl+Q quit) are
/// left untouched.
pub fn resolve_preset_bindings(
    preset: &str,
    primary_key: &str,
    is_macos: bool,
) -> Vec<ShortcutBinding> {
    let primary = primary_modifier(primary_key, is_macos).to_string();
    preset_bindings(preset)
        .into_iter()
        .map(|mut binding| {
            if binding.modifiers.iter().any(|modifier| modifier == "alt") {
                binding.modifiers = binding
                    .modifiers
                    .iter()
                    .map(|modifier| {
                        if modifier == "alt" {
                            primary.clone()
                        } else {
                            modifier.clone()
                        }
                    })
                    .collect();
            }
            binding
        })
        .collect()
}

/// Ensure the resolved preset has no duplicate chords. Overrides are applied by
/// the caller before invoking this when needed.
pub fn validate_preset_chords(bindings: &[ShortcutBinding]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for binding in bindings {
        let mut modifiers = binding.modifiers.clone();
        modifiers.sort();
        let chord = format!("{}+{}", modifiers.join("+"), binding.key.to_ascii_lowercase());
        if !seen.insert(chord.clone()) {
            return Err(format!("快捷键冲突: {chord}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Action;

    #[test]
    fn colemak_preset_keeps_physical_positions() {
        let bindings = preset_bindings("colemak");
        let find = |key: &str| bindings.iter().any(|binding| binding.key == key);
        assert!(find("k"), "Alt+N physical position should be k in Colemak");
        assert!(find("g"), "Alt+T physical position should be g in Colemak");
        assert!(find("r"), "Alt+S physical position should be r in Colemak");
        assert!(find("semicolon"), "Alt+P physical position should be semicolon");
        assert!(find("1"), "digit keys stay at the same physical position");
    }

    #[test]
    fn primary_modifier_auto_is_platform_specific() {
        assert_eq!(primary_modifier("auto", false), "alt");
        assert_eq!(primary_modifier("auto", true), "command");
        assert_eq!(primary_modifier("control", true), "control");
    }

    #[test]
    fn resolve_preset_replaces_only_primary_alt() {
        let bindings = resolve_preset_bindings("qwerty", "control", false);
        let quick = bindings
            .iter()
            .find(|binding| binding.key == "p")
            .expect("quick connect preset");
        assert!(quick.modifiers.contains(&"control".to_string()));
        let quit = bindings
            .iter()
            .find(|binding| binding.key == "q" && binding.modifiers.contains(&"control".to_string()))
            .expect("quit binding");
        assert!(quit.modifiers.contains(&"control".to_string()));
        let copy = bindings
            .iter()
            .find(|binding| binding.key == "c")
            .expect("copy binding");
        assert!(copy.modifiers.contains(&"control".to_string()));
        assert!(copy.modifiers.contains(&"shift".to_string()));
    }

    #[test]
    fn action_catalog_covers_all_known_actions() {
        let catalog = action_catalog();
        let known = [
            "new_window",
            "new_tab",
            "new_pane",
            "new_pane_vertical",
            "switch_tab_1",
            "switch_tab_last",
            "switch_pane_prev",
            "switch_pane_next",
            "search",
            "command_palette",
            "quick_connect",
            "quit",
            "increase_font_size",
            "decrease_font_size",
            "reset_font_size",
            "toggle_pane_fullscreen",
            "copy",
            "paste",
        ];
        let ids: std::collections::BTreeSet<_> = catalog.iter().map(|item| item.id).collect();
        for action in known {
            assert!(ids.contains(action), "missing catalog action {action}");
        }
        assert_eq!(Action::from_str("nonsense"), Action::Unknown);
    }
}
