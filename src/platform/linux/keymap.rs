//! 快捷键匹配：把配置里的 `[[keybindings]]` 解析成可匹配结构，供
//! `EventControllerKey` 回调用。

use std::collections::HashMap;

use gtk4::gdk;

use crate::config::{Action, KeyBinding, ModSet};

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
        let key_str = if let Some(c) = keyval.to_unicode() {
            c.to_string()
        } else if let Some(n) = keyval.name() {
            n.to_string().to_lowercase()
        } else {
            return None;
        };
        let modset = ModSet::from_gdk(mods);
        self.map.get(&(key_str, modset)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
