//! 快捷键匹配：把配置里的 `[[keybindings]]` 解析成可匹配结构，供
//! `EventControllerKey` 回调用。

use std::collections::HashMap;

use gtk4::gdk;

use crate::config::{Action, KeyBinding, ModSet, Modifiers};

/// 把 GDK `ModifierType` 转成平台无关 [`Modifiers`]。
///
/// GTK 依赖留在 platform 层，core 不引用 gtk4。
pub fn modifiers_from_gdk(mods: gdk::ModifierType) -> Modifiers {
    use gdk::ModifierType as M;
    let mut m = Modifiers::NONE;
    if mods.contains(M::CONTROL_MASK) {
        m.insert(Modifiers::CONTROL);
    }
    if mods.contains(M::SHIFT_MASK) {
        m.insert(Modifiers::SHIFT);
    }
    if mods.contains(M::ALT_MASK) {
        m.insert(Modifiers::ALT);
    }
    if mods.contains(M::SUPER_MASK) {
        m.insert(Modifiers::SUPER);
    }
    m
}

/// 快捷键匹配表。
pub struct KeyMap {
    map: HashMap<(String, ModSet), Action>,
}

impl KeyMap {
    pub fn from_bindings(bindings: &[KeyBinding]) -> Self {
        let mut map = HashMap::new();
        for b in bindings {
            let action = Action::from_str(&b.action);
            let mods = ModSet::from_binding(&b.mods);
            map.insert((b.key.clone(), mods), action);
        }
        KeyMap { map }
    }

    /// 查找匹配的 action。
    pub fn lookup(&self, keyval: gdk::Key, mods: gdk::ModifierType) -> Option<Action> {
        // 优先用 keyval 的 unicode 字符（区分大小写）
        let key_str = match keyval.to_unicode() {
            Some(c) => c.to_string(),
            None => keyval.name()?.to_string().to_lowercase(),
        };
        let modset = ModSet::from_modifiers(modifiers_from_gdk(mods));
        self.map.get(&(key_str, modset)).copied()
    }

    /// 纯字符串查找（单测 / 配置校验用，不依赖 GDK）。
    pub fn lookup_str(&self, key: &str, mods: &[&str]) -> Option<Action> {
        let mods: Vec<String> = mods.iter().map(|s| (*s).to_string()).collect();
        let modset = ModSet::from_binding(&mods);
        self.map.get(&(key.to_string(), modset)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_keybindings;

    #[test]
    fn from_bindings_ignores_unknown_action() {
        let bs = vec![KeyBinding {
            key: "x".into(),
            mods: vec!["alt".into()],
            action: "nonsense".into(),
        }];
        let km = KeyMap::from_bindings(&bs);
        assert!(km
            .map
            .contains_key(&("x".into(), ModSet::from_binding(&["alt".into()]))));
    }

    /// 对应：Alt+N → new_window。
    #[test]
    fn test_keymap_alt_n_new_window() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("n", &["alt"]), Some(Action::NewWindow));
    }

    /// 对应：Alt+Shift+D → 竖直分割 pane。
    #[test]
    fn test_keymap_alt_shift_d_vertical_pane() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(
            km.lookup_str("D", &["alt", "shift"]),
            Some(Action::NewPaneVertical)
        );
    }

    /// 对应：Alt+1..9 切 tab。
    #[test]
    fn test_keymap_alt_digits_switch_tabs() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("1", &["alt"]), Some(Action::SwitchTab1));
        assert_eq!(km.lookup_str("5", &["alt"]), Some(Action::SwitchTab5));
        assert_eq!(km.lookup_str("9", &["alt"]), Some(Action::SwitchTab9));
    }

    /// 对应：Alt+0 → 最后一个 tab。
    #[test]
    fn test_keymap_alt_0_switch_tab_last() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("0", &["alt"]), Some(Action::SwitchTabLast));
    }

    /// 对应：自定义快捷键覆盖默认同键位。
    #[test]
    fn test_keymap_custom_override_same_chord() {
        let mut bs = default_keybindings();
        bs.push(KeyBinding {
            key: "n".into(),
            mods: vec!["alt".into()],
            action: "new_tab".into(),
        });
        let km = KeyMap::from_bindings(&bs);
        // HashMap 后写覆盖
        assert_eq!(km.lookup_str("n", &["alt"]), Some(Action::NewTab));
    }

    /// 对应：同一组合键绑定两个 action → 取最后注册的。
    #[test]
    fn test_keymap_duplicate_chord_last_wins() {
        let bs = vec![
            KeyBinding {
                key: "p".into(),
                mods: vec!["alt".into()],
                action: "search".into(),
            },
            KeyBinding {
                key: "p".into(),
                mods: vec!["alt".into()],
                action: "command_palette".into(),
            },
        ];
        let km = KeyMap::from_bindings(&bs);
        assert_eq!(km.lookup_str("p", &["alt"]), Some(Action::CommandPalette));
    }

    #[test]
    fn test_keymap_empty_bindings_lookup_none() {
        let km = KeyMap::from_bindings(&[]);
        assert_eq!(km.lookup_str("n", &["alt"]), None);
    }

    #[test]
    fn test_keymap_alt_p_command_palette() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("p", &["alt"]), Some(Action::CommandPalette));
    }
}
