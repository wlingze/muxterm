//! 快捷键匹配：把配置里的 `[[keybindings]]` 解析成可匹配结构，供
//! `EventControllerKey` 回调用。

use std::collections::HashMap;

use gtk4::gdk;

use crate::core::config::{Action, KeyBinding, ModSet, Modifiers};

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

/// 特殊键（Return/plus/minus/F 键等）按键名匹配。
fn is_special_key_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "plus"
            | "minus"
            | "return"
            | "kp_enter"
            | "escape"
            | "tab"
            | "backspace"
            | "delete"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "insert"
            | "page_up"
            | "page_down"
            | "kp_add"
            | "kp_subtract"
    ) || (name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit()))
        || (name.starts_with("KP_") && name[3..].chars().all(|c| c.is_ascii_digit()))
}

impl KeyMap {
    pub fn from_bindings(bindings: &[KeyBinding]) -> Self {
        let mut map = HashMap::new();
        for b in bindings {
            let action = Action::from_str(&b.action);
            let mods = ModSet::from_binding(&b.mods);
            map.insert((b.key.to_lowercase(), mods), action);
        }
        KeyMap { map }
    }

    /// 查找匹配的 action。
    pub fn lookup(&self, keyval: gdk::Key, mods: gdk::ModifierType) -> Option<Action> {
        // GTK4 会把 Shift 算进 keyval（`C` vs `c`）并从 mods 里吃掉
        // SHIFT_MASK。不补回来的话 Ctrl+Shift+C 会变成 Ctrl+C（`\\003`）。
        let mods = restore_consumed_shift(keyval, mods);
        // 特殊键（Return/plus/minus/F 键等）按键名匹配（大小写不敏感）；
        // `[` / `]` 在 GDK 里叫 bracketleft/bracketright，Alt 组合下
        // to_unicode() 可能为 None，必须按键名归一。
        // 普通字符键优先用 unicode（区分大小写），保证 `d` 与 `D` 不同。
        let key_str = match keyval.name() {
            Some(name) => {
                let lower = name.to_ascii_lowercase();
                match lower.as_str() {
                    "bracketleft" => "[".to_string(),
                    "bracketright" => "]".to_string(),
                    _ if is_special_key_name(&name) => lower,
                    _ => match keyval.to_unicode() {
                        Some(c) => c.to_ascii_lowercase().to_string(),
                        None => lower,
                    },
                }
            }
            None => keyval.to_unicode()?.to_ascii_lowercase().to_string(),
        };
        let modset = ModSet::from_modifiers(modifiers_from_gdk(mods));
        self.map.get(&(key_str, modset)).copied()
    }

    /// 纯字符串查找（单测 / 配置校验用，不依赖 GDK）。
    pub fn lookup_str(&self, key: &str, mods: &[&str]) -> Option<Action> {
        let mods: Vec<String> = mods.iter().map(|s| (*s).to_string()).collect();
        let modset = ModSet::from_binding(&mods);
        self.map.get(&(key.to_lowercase(), modset)).copied()
    }
}

/// GTK4 `EventControllerKey` 的 mods 不含已被 keyval 消费的 Shift。
pub fn restore_consumed_shift(keyval: gdk::Key, mods: gdk::ModifierType) -> gdk::ModifierType {
    if keyval.to_unicode().is_some_and(|c| c.is_ascii_uppercase()) {
        mods | gdk::ModifierType::SHIFT_MASK
    } else {
        mods
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{default_keybindings, Action, KeyBinding};

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
            km.lookup_str("d", &["alt", "shift"]),
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

    /// 对应：Alt+Q 快速连接。
    #[test]
    fn test_keymap_alt_q_quick_connect() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("q", &["alt"]), Some(Action::QuickConnect));
    }

    /// 对应：Ctrl+Plus/Minus/0 字体缩放。
    #[test]
    fn test_keymap_font_zoom_bindings() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(
            km.lookup_str("plus", &["control"]),
            Some(Action::IncreaseFontSize)
        );
        assert_eq!(
            km.lookup_str("minus", &["control"]),
            Some(Action::DecreaseFontSize)
        );
        assert_eq!(
            km.lookup_str("0", &["control"]),
            Some(Action::ResetFontSize)
        );
    }

    /// 对应：Ctrl+Return 切换 pane 全屏。
    #[test]
    fn test_keymap_ctrl_return_pane_fullscreen() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(
            km.lookup_str("return", &["control"]),
            Some(Action::TogglePaneFullscreen)
        );
    }

    #[test]
    fn test_keymap_ctrl_q_quit() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("q", &["control"]), Some(Action::Quit));
    }

    #[test]
    fn test_keymap_ctrl_shift_c_v_copy_paste() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(
            km.lookup_str("c", &["control", "shift"]),
            Some(Action::Copy)
        );
        assert_eq!(
            km.lookup_str("v", &["control", "shift"]),
            Some(Action::Paste)
        );
        assert_eq!(km.lookup_str("c", &["control"]), None);
        assert_eq!(km.lookup_str("v", &["control"]), None);
    }

    #[test]
    fn test_keymap_custom_overrides_new_actions() {
        let mut bs = default_keybindings();
        bs.push(KeyBinding {
            key: "q".into(),
            mods: vec!["alt".into()],
            action: "quit".into(),
        });
        bs.push(KeyBinding {
            key: "return".into(),
            mods: vec!["control".into()],
            action: "command_palette".into(),
        });
        let km = KeyMap::from_bindings(&bs);
        assert_eq!(km.lookup_str("q", &["alt"]), Some(Action::Quit));
        assert_eq!(
            km.lookup_str("return", &["control"]),
            Some(Action::CommandPalette)
        );
        assert_eq!(km.lookup_str("q", &["control"]), Some(Action::Quit));
    }

    /// 对应：Alt+P 快速连接，Alt+Shift+P 命令面板（macOS 为 Cmd / Cmd+Shift）。
    /// Ctrl+P 不拦截，留给终端「上一个」。
    #[test]
    fn test_keymap_alt_p_quick_connect_shift_p_palette() {
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("p", &["alt"]), Some(Action::QuickConnect));
        assert_eq!(
            km.lookup_str("p", &["alt", "shift"]),
            Some(Action::CommandPalette)
        );
        assert_eq!(
            km.lookup_str("P", &["alt", "shift"]),
            Some(Action::CommandPalette)
        );
        assert_eq!(km.lookup_str("p", &["control"]), None);
        assert_eq!(km.lookup_str("p", &["control", "shift"]), None);
        assert_eq!(km.lookup_str("p", &["super"]), None);
        assert_eq!(km.lookup_str("p", &["super", "shift"]), None);
    }

    #[test]
    fn test_keymap_gdk_alt_shift_p_uppercase_is_palette() {
        use gdk::ModifierType as M;
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(
            km.lookup(gdk::Key::p, M::ALT_MASK),
            Some(Action::QuickConnect)
        );
        assert_eq!(
            km.lookup(gdk::Key::P, M::ALT_MASK | M::SHIFT_MASK),
            Some(Action::CommandPalette)
        );
        assert_eq!(km.lookup(gdk::Key::p, M::CONTROL_MASK), None);
        assert_eq!(
            km.lookup(gdk::Key::C, M::CONTROL_MASK | M::SHIFT_MASK),
            Some(Action::Copy)
        );
        assert_eq!(
            km.lookup(gdk::Key::V, M::CONTROL_MASK | M::SHIFT_MASK),
            Some(Action::Paste)
        );
        assert_eq!(km.lookup(gdk::Key::c, M::CONTROL_MASK), None);
        // 2310.log：GTK 吃掉 Shift 后只剩 CONTROL + 大写 C/V，必须仍是复制粘贴，
        // 不能当 Ctrl+C / Ctrl+V 漏进 send-keys（`\\003` / `\\026`）。
        assert_eq!(km.lookup(gdk::Key::C, M::CONTROL_MASK), Some(Action::Copy));
        assert_eq!(km.lookup(gdk::Key::V, M::CONTROL_MASK), Some(Action::Paste));
        assert_eq!(
            km.lookup(gdk::Key::P, M::ALT_MASK),
            Some(Action::CommandPalette)
        );
    }

    #[test]
    fn test_keymap_alt_brackets_switch_pane() {
        use gdk::ModifierType as M;
        let km = KeyMap::from_bindings(&default_keybindings());
        assert_eq!(km.lookup_str("[", &["alt"]), Some(Action::SwitchPanePrev));
        assert_eq!(km.lookup_str("]", &["alt"]), Some(Action::SwitchPaneNext));
        assert_eq!(
            km.lookup(gdk::Key::bracketleft, M::ALT_MASK),
            Some(Action::SwitchPanePrev)
        );
        assert_eq!(
            km.lookup(gdk::Key::bracketright, M::ALT_MASK),
            Some(Action::SwitchPaneNext)
        );
    }
}
