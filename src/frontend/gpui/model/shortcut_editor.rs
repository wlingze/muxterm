//! 设置里的快捷键目录、录制与覆盖。与 Swift `ShortcutEditor.swift` 同一语义。
//! 内部一律保存 Colemak 标签；QWERTY 只是一套同手位显示标签。

use super::{
    physical_key,
    shortcuts::{self, KeyAction, KeyChord},
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutMap {
    pub custom: HashMap<KeyChord, KeyAction>,
    pub suppressed: HashSet<KeyAction>,
}

impl ShortcutMap {
    pub fn from_values(values: &Value) -> Self {
        let qwerty = values.pointer("/shortcuts/preset").and_then(Value::as_str) == Some("qwerty");
        let custom = shortcuts::parse_values(values)
            .into_iter()
            .map(|(mut key, action)| {
                if qwerty {
                    key.key = physical_key::from_qwerty(&key.key);
                }
                (key, action)
            })
            .collect();
        Self {
            custom,
            suppressed: shortcuts::suppressed_actions(values),
        }
    }

    pub fn resolve(&self, canonical: &KeyChord) -> Option<KeyAction> {
        shortcuts::action(canonical, Some(&self.custom), &self.suppressed)
    }

    /// 菜单提示用：每个动作的第一条默认键（若有）。
    /// 必须在快捷键显示里区分「内置默认」与用户自定义。
    pub fn default_chord(&self, action: KeyAction) -> Option<KeyChord> {
        catalog()
            .into_iter()
            .find(|row| row.action == action)
            .and_then(|row| {
                row.defaults
                    .into_iter()
                    .find(|key| self.resolve(key) == Some(action))
            })
            .or_else(|| {
                self.custom
                    .iter()
                    .filter(|(_, bound)| **bound == action)
                    .map(|(key, _)| key.clone())
                    .min_by_key(|key| label(key, false))
            })
    }

    pub fn chords(&self, action: KeyAction) -> Vec<KeyChord> {
        let mut chords: Vec<_> = catalog()
            .into_iter()
            .filter(|row| row.action == action)
            .flat_map(|row| row.defaults)
            .filter(|key| self.resolve(key) == Some(action))
            .collect();
        for (chord, bound) in &self.custom {
            if *bound == action && !chords.contains(chord) {
                chords.push(chord.clone());
            }
        }
        chords.sort_by_key(|key| (key.option, key.control, key.shift, key.key.clone()));
        chords
    }
}

#[derive(Debug, Clone)]
pub struct ShortcutRow {
    pub action: KeyAction,
    pub id: String,
    pub title: String,
    pub defaults: Vec<KeyChord>,
}

/// 目录来自同一份内置解析器，不另外维护一张可能漏动作的表。
pub fn catalog() -> Vec<ShortcutRow> {
    let mut rows: Vec<ShortcutRow> = Vec::new();
    for key in [
        "t", "d", "s", "w", "p", "f", "r", "k", "i", "b", "c", "v", "a", "q", "l", "1", "2", "3",
        "4", "5", "6", "7", "8", "9", "0", "left", "right", "up", "down", "[", "]", "\r", ",", "=",
        "+", "-",
    ] {
        for (command, shift, option, control) in [
            (true, false, false, false),
            (true, true, false, false),
            (true, false, false, true),
            (true, false, true, false),
            (false, false, true, false),
            (false, false, false, true),
        ] {
            let chord = KeyChord::new(command, shift, option, control, key);
            let Some(action) = shortcuts::builtin_action(&chord) else {
                continue;
            };
            let id = shortcuts::action_name(action).unwrap_or_else(|| match action {
                KeyAction::SelectAll => "select_all".into(),
                _ => format!("{action:?}"),
            });
            if let Some(row) = rows.iter_mut().find(|row| row.action == action) {
                if !row.defaults.contains(&chord) {
                    row.defaults.push(chord);
                }
            } else {
                rows.push(ShortcutRow {
                    action,
                    title: super::settings::humanize(&id),
                    id,
                    defaults: vec![chord],
                });
            }
        }
    }
    rows
}

/// 快捷键的显示文本。
///
/// 平台写法统一交给 `platform::chord_label`：macOS 是 `⌘⇧T`，Linux / Windows
/// 是 `Alt+Shift+T`（命令键取 `[shortcuts] primary_key`）。设置页与菜单提示
/// 共用一份，不会一边写 ⌘ 一边写 Alt。
pub fn label(chord: &KeyChord, qwerty: bool) -> String {
    let key = if qwerty {
        physical_key::to_qwerty(&chord.key)
    } else {
        chord.key.clone()
    };
    crate::frontend::gpui::platform::chord_label(
        chord.command,
        chord.shift,
        chord.option,
        chord.control,
        &key,
    )
}

#[derive(Debug, Clone, Default)]
pub struct ShortcutEditor {
    pub query: String,
    pub searching: bool,
    pub recording: Option<KeyAction>,
    pub error: Option<String>,
}

pub fn set_override(values: &Value, action: KeyAction, chord: Option<&KeyChord>) -> Value {
    let id = shortcuts::action_name(action).unwrap_or_default();
    let mut rows = values
        .pointer("/shortcuts/overrides")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rows.retain(|row| row["action"].as_str() != Some(&id));
    let bindings = chord
        .map(|key| {
            let mut modifiers = Vec::new();
            if key.command {
                modifiers.push("command");
            }
            if key.shift {
                modifiers.push("shift");
            }
            if key.option {
                modifiers.push("option");
            }
            if key.control {
                modifiers.push("control");
            }
            let qwerty =
                values.pointer("/shortcuts/preset").and_then(Value::as_str) == Some("qwerty");
            let key = if qwerty {
                physical_key::to_qwerty(&key.key)
            } else {
                key.key.clone()
            };
            vec![json!({"key": key, "modifiers": modifiers})]
        })
        .unwrap_or_default();
    rows.push(json!({"action": id, "bindings": bindings}));
    Value::Array(rows)
}

pub fn restore(values: &Value, action: KeyAction) -> Value {
    let id = shortcuts::action_name(action).unwrap_or_default();
    let mut rows = values
        .pointer("/shortcuts/overrides")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rows.retain(|row| row["action"].as_str() != Some(&id));
    Value::Array(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `testShortcutDialogSearchesDisablesAndSavesWithoutChangingCancel`。
    #[test]
    fn disable_restore_and_record_are_values_not_side_effects() {
        let values = json!({"shortcuts": {"preset":"colemak", "overrides":[]}});
        let mut draft = values.clone();
        draft["shortcuts"]["overrides"] = set_override(&draft, KeyAction::NewTab, None);
        assert_eq!(
            ShortcutMap::from_values(&draft).resolve(&KeyChord::command("t")),
            None
        );
        assert_eq!(values["shortcuts"]["overrides"], json!([]));
        draft["shortcuts"]["overrides"] = restore(&draft, KeyAction::NewTab);
        assert_eq!(
            ShortcutMap::from_values(&draft).resolve(&KeyChord::command("t")),
            Some(KeyAction::NewTab)
        );
        draft["shortcuts"]["overrides"] =
            set_override(&draft, KeyAction::NewTab, Some(&KeyChord::command("h")));
        assert_eq!(
            ShortcutMap::from_values(&draft).resolve(&KeyChord::command("h")),
            Some(KeyAction::NewTab)
        );
    }

    #[test]
    fn configuration_labels_do_not_change_hand_positions() {
        for preset in ["colemak", "qwerty"] {
            let mut values = json!({"shortcuts":{"preset":preset,"overrides":[]}});
            values["shortcuts"]["overrides"] =
                set_override(&values, KeyAction::NewTab, Some(&KeyChord::command("d")));
            assert_eq!(
                ShortcutMap::from_values(&values).resolve(&KeyChord::command("d")),
                Some(KeyAction::NewTab)
            );
        }
        // 显示文本按平台：macOS `⌘D`，Linux / Windows `<primary>+D`。
        assert_eq!(
            label(&KeyChord::command("d"), false),
            crate::frontend::gpui::platform::chord_label(true, false, false, false, "d")
        );
        assert_eq!(
            label(&KeyChord::command("d"), true),
            crate::frontend::gpui::platform::chord_label(true, false, false, false, "g")
        );
    }

    #[test]
    fn catalog_covers_all_reported_shortcuts_and_the_paste_path() {
        let rows = catalog();
        for action in [
            KeyAction::NewTab,
            KeyAction::SplitHorizontal,
            KeyAction::SplitVertical,
            KeyAction::QuickConnect,
            KeyAction::Paste,
            KeyAction::OpenSettings,
            KeyAction::OpenAgents,
        ] {
            assert!(rows.iter().any(|row| row.action == action));
        }
    }
}
