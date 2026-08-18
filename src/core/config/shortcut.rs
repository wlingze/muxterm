use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ============================================================================
// 快捷键
// ============================================================================

/// 一条快捷键绑定。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub mods: Vec<String>,
    pub action: String,
}

/// 快捷键动作（解析后用于匹配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    NewWindow,
    NewTab,
    NewPane,
    NewPaneVertical,
    SwitchTab1,
    SwitchTab2,
    SwitchTab3,
    SwitchTab4,
    SwitchTab5,
    SwitchTab6,
    SwitchTab7,
    SwitchTab8,
    SwitchTab9,
    SwitchTabLast,
    SwitchPanePrev,
    SwitchPaneNext,
    Search,
    CommandPalette,
    QuickConnect,
    Quit,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    TogglePaneFullscreen,
    Copy,
    Paste,
    /// 未知动作（保留原始字符串，匹配时忽略）。
    Unknown,
}

impl Action {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "new_window" => Action::NewWindow,
            "new_tab" => Action::NewTab,
            "new_pane" => Action::NewPane,
            "new_pane_vertical" => Action::NewPaneVertical,
            "switch_tab_1" => Action::SwitchTab1,
            "switch_tab_2" => Action::SwitchTab2,
            "switch_tab_3" => Action::SwitchTab3,
            "switch_tab_4" => Action::SwitchTab4,
            "switch_tab_5" => Action::SwitchTab5,
            "switch_tab_6" => Action::SwitchTab6,
            "switch_tab_7" => Action::SwitchTab7,
            "switch_tab_8" => Action::SwitchTab8,
            "switch_tab_9" => Action::SwitchTab9,
            "switch_tab_last" => Action::SwitchTabLast,
            "switch_pane_prev" => Action::SwitchPanePrev,
            "switch_pane_next" => Action::SwitchPaneNext,
            "search" => Action::Search,
            "command_palette" => Action::CommandPalette,
            "quick_connect" => Action::QuickConnect,
            "quit" => Action::Quit,
            "increase_font_size" => Action::IncreaseFontSize,
            "decrease_font_size" => Action::DecreaseFontSize,
            "reset_font_size" => Action::ResetFontSize,
            "toggle_pane_fullscreen" => Action::TogglePaneFullscreen,
            "copy" => Action::Copy,
            "paste" => Action::Paste,
            _ => Action::Unknown,
        }
    }
}

/// 默认快捷键（Alt+N/T/D/Shift+D/1-9/0/[ ]/R/P）。
pub fn default_keybindings() -> Vec<KeyBinding> {
    vec![
        kb("n", &["alt"], "new_window"),
        kb("t", &["alt"], "new_tab"),
        // 水平 / 竖直分割：与 TUI（Alt+S / Alt+V）对齐；保留 Alt+D 兼容 ARCHITECTURE
        kb("s", &["alt"], "new_pane"),
        kb("v", &["alt"], "new_pane_vertical"),
        kb("d", &["alt"], "new_pane"),
        kb("d", &["alt", "shift"], "new_pane_vertical"),
        kb("1", &["alt"], "switch_tab_1"),
        kb("2", &["alt"], "switch_tab_2"),
        kb("3", &["alt"], "switch_tab_3"),
        kb("4", &["alt"], "switch_tab_4"),
        kb("5", &["alt"], "switch_tab_5"),
        kb("6", &["alt"], "switch_tab_6"),
        kb("7", &["alt"], "switch_tab_7"),
        kb("8", &["alt"], "switch_tab_8"),
        kb("9", &["alt"], "switch_tab_9"),
        kb("0", &["alt"], "switch_tab_last"),
        kb("[", &["alt"], "switch_pane_prev"),
        kb("]", &["alt"], "switch_pane_next"),
        kb("r", &["alt"], "search"),
        // Linux Alt+P / macOS Cmd+P：QuickConnect。不绑 Ctrl+P，留给终端「上一个」。
        kb("p", &["alt"], "quick_connect"),
        // Linux Alt+Shift+P / macOS Cmd+Shift+P：命令面板
        kb("p", &["alt", "shift"], "command_palette"),
        kb("q", &["alt"], "quick_connect"),
        kb("q", &["control"], "quit"),
        kb("plus", &["control"], "increase_font_size"),
        kb("equal", &["control"], "increase_font_size"),
        kb("minus", &["control"], "decrease_font_size"),
        kb("0", &["control"], "reset_font_size"),
        kb("return", &["control"], "toggle_pane_fullscreen"),
        kb("c", &["control", "shift"], "copy"),
        kb("v", &["control", "shift"], "paste"),
    ]
}

fn kb(key: &str, mods: &[&str], action: &str) -> KeyBinding {
    KeyBinding {
        key: key.into(),
        mods: mods.iter().map(|s| s.to_string()).collect(),
        action: action.into(),
    }
}

/// 平台无关修饰键（自建 bitflags，不依赖任何 GUI 库）。
///
/// GTK / AppKit / WinUI 在各自 platform 层把原生修饰键转成此类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const CONTROL: Modifiers = Modifiers(0b0001);
    pub const SHIFT: Modifiers = Modifiers(0b0010);
    pub const ALT: Modifiers = Modifiers(0b0100);
    pub const SUPER: Modifiers = Modifiers(0b1000);

    /// 是否包含全部 `other` 标志位。
    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    /// 并入标志位。
    pub fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// 修饰键集合（规范化小写，排序存以便 Hash/Eq）。
///
/// 用于 keybinding 查找表；与 [`Modifiers`] 可互转。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ModSet(pub Vec<String>);

impl ModSet {
    /// 从平台无关 [`Modifiers`] 构造。
    pub fn from_modifiers(mods: Modifiers) -> Self {
        let mut v = Vec::new();
        if mods.contains(Modifiers::CONTROL) {
            v.push("control".into());
        }
        if mods.contains(Modifiers::SHIFT) {
            v.push("shift".into());
        }
        if mods.contains(Modifiers::ALT) {
            v.push("alt".into());
        }
        if mods.contains(Modifiers::SUPER) {
            v.push("super".into());
        }
        v.sort();
        ModSet(v)
    }

    pub fn from_binding(mods: &[String]) -> Self {
        let mut v: Vec<String> = mods.iter().map(|m| m.to_lowercase()).collect();
        v.sort();
        v.dedup();
        ModSet(v)
    }
}
