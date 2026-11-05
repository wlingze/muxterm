//! 窗口级快捷键表与相邻 Tab 策略。
//!
//! 移植自 `chrome/KeyBindings.swift` 与 `chrome/KeyBindingsConfig.swift`。
//!
//! 这里是**窗口动作**表，与 `terminal::keymap` 的按键字节编码是两件事：
//! 后者决定把什么字节发进 PTY，前者决定窗口要不要吃掉这次按键。
//!
//! macOS 表是事实标准，所以 `command` 字段照抄；gpui 在 macOS 上把
//! `Modifiers::platform()` 映射成 command，在 Linux 上映射成 control，
//! 由平台适配层负责，本模块保持纯粹可测。

use std::collections::{HashMap, HashSet};

/// macOS 客户端快捷键动作（与菜单 / Cmd 映射一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    NewTab,
    SplitHorizontal,
    SplitVertical,
    SplitAuto,
    CloseWindow,
    ClosePane,
    /// 关闭当前 tab（Swift 命令面板里的 Close Tab；配置里可写 `close_tab`）。
    CloseTab,
    /// 1-based。
    SwitchTab(usize),
    SwitchLastTab,
    /// 当前 tab 的前一个 / 后一个。Cmd-1..9 盖不住第 10 个以后的 tab。
    PreviousTab,
    NextTab,
    NextPane,
    PrevPane,
    /// 在 OSC 133 命令刻度之间向前 / 向后跳转；不向 pane 发送按键。
    PreviousCommand,
    NextCommand,
    CommandPalette,
    QuickConnect,
    Attention,
    SearchWorkspace,
    SearchGlobal,
    Quit,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    TogglePaneFullscreen,
    /// 整个窗口全屏，与 pane zoom 分开。
    ToggleWindowFullscreen,
    /// 只在当前桌面里把窗口铺满，不进入全屏 Space。
    ToggleWindowZoom,
    OpenSettings,
    /// 独立输入框：写一段话再复制或写进当前 pane。
    OpenComposer,
    CycleLayout,
    ToggleSidebar,
    /// 进入前端 Shells 聚合槽；不会向终端发送清屏字符。
    OpenShells,
    /// 进入前端 Agents 聚合槽；只投影已有源 Tab。
    OpenAgents,
    /// 1-based 打开顺序；0 永远是最后一个。
    SwitchWorkspace(usize),
}

impl KeyAction {
    /// 中文注释：动作是否属于「切换 tab」，供测试与菜单勾选使用。
    pub fn is_tab_switch(self) -> bool {
        matches!(
            self,
            KeyAction::SwitchTab(_)
                | KeyAction::SwitchLastTab
                | KeyAction::PreviousTab
                | KeyAction::NextTab
        )
    }
}

/// 修饰键 + 主键（大小写无关）的纯数据描述。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub command: bool,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
    pub key: String,
}

impl KeyChord {
    /// 构造时统一小写，比较就不受键盘布局大小写影响。
    pub fn new(command: bool, shift: bool, option: bool, control: bool, key: &str) -> Self {
        Self {
            command,
            shift,
            option,
            control,
            key: key.to_lowercase(),
        }
    }

    /// `Cmd+X` 简写。
    pub fn command(key: &str) -> Self {
        Self::new(true, false, false, false, key)
    }

    /// `Cmd+Shift+X` 简写。
    pub fn command_shift(key: &str) -> Self {
        Self::new(true, true, false, false, key)
    }

    /// `Alt+X` 简写。
    pub fn option(key: &str) -> Self {
        Self::new(false, false, true, false, key)
    }
}

/// 内置快捷键表。`Cmd+[` / `Cmd+]` 切 pane 必须保持不变。
pub fn builtin_action(chord: &KeyChord) -> Option<KeyAction> {
    let key = chord.key.as_str();

    // Cmd+T 新建 tab
    if chord.command && !chord.shift && !chord.option && key == "t" {
        return Some(KeyAction::NewTab);
    }
    // Cmd-D 左右、Cmd-Shift-D 上下；Cmd-S 上下、Cmd-Shift-S 左右。
    // Cmd-V 必须留给粘贴。自动切分不再占用默认键。
    if chord.command && !chord.option && !chord.control {
        if key == "d" {
            return Some(if chord.shift {
                KeyAction::SplitVertical
            } else {
                KeyAction::SplitHorizontal
            });
        }
        if key == "s" {
            return Some(if chord.shift {
                KeyAction::SplitHorizontal
            } else {
                KeyAction::SplitVertical
            });
        }
    }
    // Cmd+W 只关当前层；整个窗口显式使用 Cmd+Shift+W。
    if chord.command && !chord.option && !chord.control && key == "w" {
        return Some(if chord.shift {
            KeyAction::CloseWindow
        } else {
            KeyAction::ClosePane
        });
    }
    // Cmd+1..9 切 tab；Cmd+Ctrl+N 留给 Workspace 切换。
    if chord.command && !chord.option && !chord.shift && !chord.control {
        if let Some(n) = single_digit(key) {
            return Some(KeyAction::SwitchTab(n));
        }
        if key == "0" {
            return Some(KeyAction::SwitchLastTab);
        }
        // Cmd-Left / Cmd-Right：相邻 tab。第 10 个以后没有数字键。
        if key == "left" {
            return Some(KeyAction::PreviousTab);
        }
        if key == "right" {
            return Some(KeyAction::NextTab);
        }
    }
    // Cmd+[ / Cmd+]：上一个 / 下一个 pane（焦点跟随）
    if chord.command && !chord.shift && !chord.option {
        if key == "[" {
            return Some(KeyAction::PrevPane);
        }
        if key == "]" {
            return Some(KeyAction::NextPane);
        }
    }

    // Cmd+Option+↑/↓：在 shell 命令时间线中前后跳转。保留普通
    // ↑/↓、Cmd+↑/↓ 给 shell 与 TUI 自身。
    if chord.command && chord.option && !chord.shift && !chord.control {
        if key == "up" {
            return Some(KeyAction::PreviousCommand);
        }
        if key == "down" {
            return Some(KeyAction::NextCommand);
        }
    }

    // Cmd+Ctrl+A/S：进入 Agents / Shells 前端聚合槽。
    if chord.command && chord.control && !chord.option && !chord.shift {
        if key == "a" {
            return Some(KeyAction::OpenAgents);
        }
        if key == "s" {
            return Some(KeyAction::OpenShells);
        }
    }

    // Cmd+I：独立输入框。终端正在输入时也能直接用，不抢当前输入。
    if chord.command && !chord.shift && !chord.option && !chord.control && key == "i" {
        return Some(KeyAction::OpenComposer);
    }

    // Cmd+P：QuickConnect 面板。右侧 workspace 图标隐藏，但快捷键保留。
    if chord.command && !chord.shift && !chord.option && !chord.control && key == "p" {
        return Some(KeyAction::QuickConnect);
    }
    // Cmd+Shift+P：旧命令面板（保留）。
    if chord.command && chord.shift && !chord.option && key == "p" {
        return Some(KeyAction::CommandPalette);
    }
    // Cmd+R：通知 / Attention 面板。
    if chord.command && !chord.shift && !chord.option && !chord.control && key == "r" {
        return Some(KeyAction::Attention);
    }
    // Cmd+F：当前 Workspace
    if chord.command && !chord.shift && !chord.option && key == "f" {
        return Some(KeyAction::SearchWorkspace);
    }
    // Cmd+= / Cmd++ 增大字体，Cmd+- 减小，Cmd+Shift+0 重置。
    if chord.command && !chord.option {
        if key == "=" || key == "+" {
            return Some(KeyAction::IncreaseFontSize);
        }
        if key == "-" {
            return Some(KeyAction::DecreaseFontSize);
        }
        if key == "0" && chord.shift && !chord.control {
            return Some(KeyAction::ResetFontSize);
        }
    }
    if chord.control && !chord.command && !chord.shift && !chord.option && key == "0" {
        return Some(KeyAction::ResetFontSize);
    }
    // Cmd+Shift+L：布局格式循环（tmux next-layout / 左右变上下）。
    if chord.command && chord.shift && !chord.option && !chord.control && key == "l" {
        return Some(KeyAction::CycleLayout);
    }
    // Colemak 字母：Cmd-Shift-F 是整个窗口全屏，Cmd-Shift-T 只铺满当前桌面。
    // 不再绑定 E。Cmd-Shift-Enter 仍可全屏；Cmd/Alt-Enter 只放大 pane。
    if chord.command && !chord.option && !chord.control {
        if chord.shift && (key == "f" || key == "\r" || key == "\n") {
            return Some(KeyAction::ToggleWindowFullscreen);
        }
        if chord.shift && key == "t" {
            return Some(KeyAction::ToggleWindowZoom);
        }
        if !chord.shift && key == "," {
            return Some(KeyAction::OpenSettings);
        }
    }
    if (key == "\r" || key == "\n")
        && !chord.shift
        && !chord.control
        && ((chord.command && !chord.option) || (chord.option && !chord.command))
    {
        return Some(KeyAction::TogglePaneFullscreen);
    }

    // Alt+T / Alt+S / Alt+V / Alt+[ / Alt+] / Alt+1..9（兼容 TUI）
    if chord.option && !chord.command {
        match key {
            "t" => return Some(KeyAction::NewTab),
            "s" => return Some(KeyAction::SplitHorizontal),
            "v" => return Some(KeyAction::SplitVertical),
            "[" => return Some(KeyAction::PrevPane),
            "]" => return Some(KeyAction::NextPane),
            "0" => return Some(KeyAction::SwitchLastTab),
            _ => {
                if let Some(n) = single_digit(key) {
                    return Some(KeyAction::SwitchTab(n));
                }
            }
        }
    }

    // Cmd-B 切换主窗口侧边栏。
    if chord.command && !chord.shift && !chord.option && !chord.control && key == "b" {
        return Some(KeyAction::ToggleSidebar);
    }

    // Cmd+Ctrl+1..9 固定打开顺序；Cmd+Ctrl+0 永远最后一个。
    if chord.command && chord.control && !chord.shift && !chord.option {
        if key == "0" {
            return Some(KeyAction::SwitchWorkspace(0));
        }
        if let Some(n) = single_digit(key) {
            return Some(KeyAction::SwitchWorkspace(n));
        }
    }

    // Ctrl+Q 退出。Ctrl+D 不属于窗口快捷键：它必须作为 EOF
    // 发送给当前 pane 的前台进程，由 shell / 程序决定是否退出。
    if chord.control && !chord.command && key == "q" {
        return Some(KeyAction::Quit);
    }

    None
}

/// 只有单个 `1..=9` 的数字键才算 tab 号；`"10"` 不是。
fn single_digit(key: &str) -> Option<usize> {
    let bytes = key.as_bytes();
    if bytes.len() != 1 {
        return None;
    }
    match bytes[0] {
        b'1'..=b'9' => Some((bytes[0] - b'0') as usize),
        _ => None,
    }
}

/// 解析键位。`custom` 来自 Core 的 `[[keybindings]]` / `shortcuts.overrides`，
/// 命中时优先于内置默认；`suppressed` 是被显式清空 bindings 的动作，
/// 它的内置键也必须一起失效。
pub fn action(
    chord: &KeyChord,
    custom: Option<&HashMap<KeyChord, KeyAction>>,
    suppressed: &HashSet<KeyAction>,
) -> Option<KeyAction> {
    if let Some(action) = custom.and_then(|map| map.get(chord)) {
        return Some(*action);
    }
    let builtin = builtin_action(chord)?;
    if suppressed.contains(&builtin) {
        return None;
    }
    Some(builtin)
}

/// 数字键只覆盖前 9 个 tab。第 10 个及以后靠相邻偏移，到头就停。
pub fn adjacent_tab_index(active_index: usize, count: usize, offset: isize) -> Option<usize> {
    if count == 0 || active_index >= count || offset == 0 {
        return None;
    }
    let destination = active_index as isize + offset;
    if destination < 0 || destination >= count as isize {
        return None;
    }
    Some(destination as usize)
}

// ============================================================================
// 配置解析（Core `configDescribeJSON` 的 `values`）
// ============================================================================

/// 配置里的 action 名 → 窗口动作。未知名字返回 `None`，不猜。
pub fn action_from_name(name: &str) -> Option<KeyAction> {
    let fixed = match name {
        "new_window" | "new_tab" => KeyAction::NewTab,
        "new_pane" => KeyAction::SplitHorizontal,
        "new_pane_vertical" => KeyAction::SplitVertical,
        "split_auto" | "new_pane_auto" => KeyAction::SplitAuto,
        "switch_pane_prev" => KeyAction::PrevPane,
        "switch_pane_next" => KeyAction::NextPane,
        "previous_command" | "command_prev" => KeyAction::PreviousCommand,
        "next_command" | "command_next" => KeyAction::NextCommand,
        "close_pane" => KeyAction::ClosePane,
        "close_tab" => KeyAction::CloseTab,
        "close_window" => KeyAction::CloseWindow,
        "command_palette" => KeyAction::CommandPalette,
        "quick_connect" => KeyAction::QuickConnect,
        "search" => KeyAction::SearchWorkspace,
        // Swift 的 `KeyBindingsConfig.action(from:)` 漏了这两个名字，于是
        // `name(for:)` 能写出 `attention` / `search_global`，却再也读不回来，
        // 设置窗口保存一次就把用户配置吃掉了。这里补齐，让映射闭合。
        "search_global" => KeyAction::SearchGlobal,
        "attention" => KeyAction::Attention,
        "quit" => KeyAction::Quit,
        "increase_font_size" => KeyAction::IncreaseFontSize,
        "decrease_font_size" => KeyAction::DecreaseFontSize,
        "reset_font_size" => KeyAction::ResetFontSize,
        "toggle_pane_fullscreen" => KeyAction::TogglePaneFullscreen,
        "toggle_window_fullscreen" => KeyAction::ToggleWindowFullscreen,
        "toggle_window_zoom" => KeyAction::ToggleWindowZoom,
        "open_settings" => KeyAction::OpenSettings,
        "open_composer" | "composer" => KeyAction::OpenComposer,
        "cycle_layout" | "next_layout" => KeyAction::CycleLayout,
        "switch_tab_last" => KeyAction::SwitchLastTab,
        "switch_tab_previous" | "previous_tab" => KeyAction::PreviousTab,
        "switch_tab_next" | "next_tab" => KeyAction::NextTab,
        "toggle_sidebar" => KeyAction::ToggleSidebar,
        "open_shells" => KeyAction::OpenShells,
        "open_agents" => KeyAction::OpenAgents,
        "switch_workspace_last" => KeyAction::SwitchWorkspace(0),
        _ => {
            if let Some(rest) = name.strip_prefix("switch_workspace_") {
                return rest.parse::<usize>().ok().and_then(|n| {
                    (1..=9)
                        .contains(&n)
                        .then_some(KeyAction::SwitchWorkspace(n))
                });
            }
            if let Some(rest) = name.strip_prefix("switch_tab_") {
                return rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| (1..=9).contains(&n).then_some(KeyAction::SwitchTab(n)));
            }
            return None;
        }
    };
    Some(fixed)
}

/// 动作对应的稳定配置标识符（与 Core action_catalog 一致）。
pub fn action_name(action: KeyAction) -> Option<String> {
    let fixed = match action {
        KeyAction::NewTab => "new_tab",
        KeyAction::SplitHorizontal => "new_pane",
        KeyAction::SplitVertical => "new_pane_vertical",
        KeyAction::SplitAuto => "split_auto",
        KeyAction::CloseWindow => "close_window",
        KeyAction::ClosePane => "close_pane",
        KeyAction::CloseTab => "close_tab",
        KeyAction::SwitchLastTab => "switch_tab_last",
        KeyAction::PreviousTab => "previous_tab",
        KeyAction::NextTab => "next_tab",
        KeyAction::NextPane => "switch_pane_next",
        KeyAction::PrevPane => "switch_pane_prev",
        KeyAction::PreviousCommand => "previous_command",
        KeyAction::NextCommand => "next_command",
        KeyAction::CommandPalette => "command_palette",
        KeyAction::QuickConnect => "quick_connect",
        KeyAction::Attention => "attention",
        KeyAction::SearchWorkspace => "search",
        KeyAction::SearchGlobal => "search_global",
        KeyAction::Quit => "quit",
        KeyAction::IncreaseFontSize => "increase_font_size",
        KeyAction::DecreaseFontSize => "decrease_font_size",
        KeyAction::ResetFontSize => "reset_font_size",
        KeyAction::TogglePaneFullscreen => "toggle_pane_fullscreen",
        KeyAction::ToggleWindowFullscreen => "toggle_window_fullscreen",
        KeyAction::ToggleWindowZoom => "toggle_window_zoom",
        KeyAction::OpenSettings => "open_settings",
        KeyAction::OpenComposer => "open_composer",
        KeyAction::CycleLayout => "cycle_layout",
        KeyAction::ToggleSidebar => "toggle_sidebar",
        KeyAction::OpenShells => "open_shells",
        KeyAction::OpenAgents => "open_agents",
        KeyAction::SwitchTab(n) => return Some(format!("switch_tab_{n}")),
        KeyAction::SwitchWorkspace(0) => "switch_workspace_last",
        KeyAction::SwitchWorkspace(n) => return Some(format!("switch_workspace_{n}")),
    };
    Some(fixed.to_string())
}

/// 配置里的键名对齐 NSEvent：`return` 是回车，`plus`/`equal`/`minus` 是字符。
/// 只裁空白，不能裁换行：`key = "\r"` 解出来就是一个 CR 键。
pub fn normalized_key(raw: &str) -> String {
    let key = raw.to_lowercase();
    match key.trim() {
        "return" | "enter" => "\r".to_string(),
        "plus" => "+".to_string(),
        "equal" | "equals" => "=".to_string(),
        "minus" | "hyphen" => "-".to_string(),
        _ => key,
    }
}

fn binding(key: &str, mods: &[String], action_name: &str) -> Option<(KeyChord, KeyAction)> {
    let action = action_from_name(action_name)?;
    let normalized = normalized_key(key);
    if normalized.is_empty() {
        return None;
    }
    let chord = KeyChord::new(
        mods.iter().any(|m| m == "command" || m == "super"),
        mods.iter().any(|m| m == "shift"),
        mods.iter().any(|m| m == "option" || m == "alt"),
        mods.iter().any(|m| m == "control"),
        &normalized,
    );
    Some((chord, action))
}

fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 Core 的 `values`。同时吃旧 `keybindings` 和新 `shortcuts.overrides`；
/// 后者后写入，同键覆盖。空 bindings 表示关掉该动作。
pub fn parse_values(values: &serde_json::Value) -> HashMap<KeyChord, KeyAction> {
    let mut result = HashMap::new();

    if let Some(rows) = values.get("keybindings").and_then(|v| v.as_array()) {
        for row in rows {
            let (Some(key), Some(action_name)) = (
                row.get("key").and_then(|v| v.as_str()),
                row.get("action").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let mods = string_list(row.get("mods"));
            if let Some((chord, action)) = binding(key, &mods, action_name) {
                result.insert(chord, action);
            }
        }
    }

    if let Some(overrides) = values
        .get("shortcuts")
        .and_then(|v| v.get("overrides"))
        .and_then(|v| v.as_array())
    {
        for item in overrides {
            let (Some(action_name), Some(action)) = (
                item.get("action").and_then(|v| v.as_str()),
                item.get("action")
                    .and_then(|v| v.as_str())
                    .and_then(action_from_name),
            ) else {
                continue;
            };
            let bindings = item
                .get("bindings")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if bindings.is_empty() {
                result.retain(|_, value| *value != action);
                continue;
            }
            for row in &bindings {
                let Some(key) = row.get("key").and_then(|v| v.as_str()) else {
                    continue;
                };
                let mods = string_list(row.get("modifiers").or_else(|| row.get("mods")));
                if let Some((chord, action)) = binding(key, &mods, action_name) {
                    result.insert(chord, action);
                }
            }
        }
    }

    result
}

/// 空 bindings 的 override 才是禁用：Core 的 `resolve_effective_keybindings`
/// 对空数组会删掉该动作的全部预设键，非空则是在预设键之外**追加**。
/// 所以只有显式禁用的动作才需要把内置键也压掉；否则 Linux 时代写下的
/// Alt 类 override（`primary_key = "auto"` 在 macOS 是 Command）会把
/// Cmd-1 / Cmd-[ 这类内置键一起杀掉。
pub fn suppressed_actions(values: &serde_json::Value) -> HashSet<KeyAction> {
    let mut suppressed = HashSet::new();
    let Some(overrides) = values
        .get("shortcuts")
        .and_then(|v| v.get("overrides"))
        .and_then(|v| v.as_array())
    else {
        return suppressed;
    };
    for item in overrides {
        let Some(action) = item
            .get("action")
            .and_then(|v| v.as_str())
            .and_then(action_from_name)
        else {
            continue;
        };
        let empty = item
            .get("bindings")
            .and_then(|v| v.as_array())
            .map(|items| items.is_empty())
            .unwrap_or(true);
        if empty {
            suppressed.insert(action);
        }
    }
    suppressed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // === KeyBindingsTests（FlatChromeTests.swift）===

    /// 对照 `testAltZeroSwitchesToLastTab`。
    #[test]
    fn alt_zero_switches_to_last_tab() {
        assert_eq!(
            builtin_action(&KeyChord::option("0")),
            Some(KeyAction::SwitchLastTab)
        );
    }

    /// 对照 `testCommandZeroSwitchesToLastTab`。
    #[test]
    fn command_zero_switches_to_last_tab() {
        assert_eq!(
            builtin_action(&KeyChord::command("0")),
            Some(KeyAction::SwitchLastTab)
        );
    }

    /// 对照 `testCommandShiftZeroResetsFontSize`。
    #[test]
    fn command_shift_zero_resets_font_size() {
        assert_eq!(
            builtin_action(&KeyChord::command_shift("0")),
            Some(KeyAction::ResetFontSize)
        );
    }

    /// 对照 `testCommandArrowsSwitchAdjacentTabsPastTheNinth`。
    #[test]
    fn command_arrows_switch_adjacent_tabs_past_the_ninth() {
        assert_eq!(
            builtin_action(&KeyChord::command("left")),
            Some(KeyAction::PreviousTab)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("right")),
            Some(KeyAction::NextTab)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("9")),
            Some(KeyAction::SwitchTab(9))
        );
        assert_eq!(
            builtin_action(&KeyChord::command("10")),
            None,
            "没有第 10 个数字键；第 10 个 tab 只能靠左右相邻切换"
        );
        assert_eq!(builtin_action(&KeyChord::command_shift("left")), None);
        assert_eq!(
            builtin_action(&KeyChord::new(true, false, true, false, "right")),
            None
        );
        assert_eq!(
            builtin_action(&KeyChord::new(false, false, false, false, "left")),
            None
        );
        assert_eq!(
            action_from_name("switch_tab_previous"),
            Some(KeyAction::PreviousTab)
        );
        assert_eq!(action_from_name("next_tab"), Some(KeyAction::NextTab));
    }

    /// 对照 `testAdjacentTabStopsAtTheEnds`。
    #[test]
    fn adjacent_tab_stops_at_the_ends() {
        assert_eq!(adjacent_tab_index(8, 12, 1), Some(9));
        assert_eq!(adjacent_tab_index(9, 12, 1), Some(10));
        assert_eq!(adjacent_tab_index(11, 12, -1), Some(10));
        assert_eq!(
            adjacent_tab_index(11, 12, 1),
            None,
            "最后一个 tab 再向右必须停住"
        );
        assert_eq!(adjacent_tab_index(0, 12, -1), None);
        assert_eq!(adjacent_tab_index(3, 12, 0), None);
        assert_eq!(adjacent_tab_index(12, 12, -1), None);
    }

    /// 对照 `testCmdBracketPaneSwitchMapping`。
    #[test]
    fn command_bracket_maps_to_pane_switch() {
        assert_eq!(
            builtin_action(&KeyChord::command("[")),
            Some(KeyAction::PrevPane)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("]")),
            Some(KeyAction::NextPane)
        );
    }

    /// 对照 `testCmdBracketIgnoresShift`。
    #[test]
    fn command_bracket_ignores_shift() {
        assert_eq!(builtin_action(&KeyChord::command_shift("[")), None);
        assert_eq!(builtin_action(&KeyChord::command_shift("]")), None);
    }

    /// 对照 `testAltBracketAlsoSwitchesPane`。
    #[test]
    fn alt_bracket_also_switches_pane() {
        assert_eq!(
            builtin_action(&KeyChord::option("[")),
            Some(KeyAction::PrevPane)
        );
        assert_eq!(
            builtin_action(&KeyChord::option("]")),
            Some(KeyAction::NextPane)
        );
    }

    /// 对照 `testCommandTimelineShortcutsUseOnlyCommandOptionArrows`。
    #[test]
    fn command_timeline_shortcuts_use_only_command_option_arrows() {
        let up = KeyChord::new(true, false, true, false, "up");
        let down = KeyChord::new(true, false, true, false, "down");
        assert_eq!(builtin_action(&up), Some(KeyAction::PreviousCommand));
        assert_eq!(builtin_action(&down), Some(KeyAction::NextCommand));
        assert_eq!(builtin_action(&KeyChord::command("up")), None);
        assert_eq!(builtin_action(&KeyChord::option("down")), None);
    }

    /// 对照 `testCommonCmdShortcutsUnchanged`。
    #[test]
    fn common_command_shortcuts_unchanged() {
        assert_eq!(
            builtin_action(&KeyChord::command("t")),
            Some(KeyAction::NewTab)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("d")),
            Some(KeyAction::SplitHorizontal),
            "Cmd-D 固定左右切分，不再按形状自动选方向"
        );
        assert_eq!(
            builtin_action(&KeyChord::command_shift("d")),
            Some(KeyAction::SplitVertical),
            "Cmd-Shift-D 上下切分"
        );
        assert_eq!(
            builtin_action(&KeyChord::command("s")),
            Some(KeyAction::SplitVertical),
            "Cmd-S 上下切分"
        );
        assert_eq!(
            builtin_action(&KeyChord::command_shift("s")),
            Some(KeyAction::SplitHorizontal),
            "Cmd-Shift-S 左右切分"
        );
        assert_eq!(
            builtin_action(&KeyChord::command("v")),
            None,
            "Cmd-V 必须留给粘贴，不能再拦截成切分"
        );
        assert_eq!(
            builtin_action(&KeyChord::command("w")),
            Some(KeyAction::ClosePane)
        );
        assert_eq!(
            builtin_action(&KeyChord::command_shift("w")),
            Some(KeyAction::CloseWindow)
        );
        assert_eq!(
            builtin_action(&KeyChord::new(true, false, false, true, "w")),
            None
        );
        assert_eq!(
            builtin_action(&KeyChord::command("2")),
            Some(KeyAction::SwitchTab(2))
        );
        assert_eq!(
            builtin_action(&KeyChord::new(false, false, false, true, "d")),
            None,
            "Ctrl+D 必须留给当前 pane 的前台进程处理 EOF"
        );
        assert_eq!(
            builtin_action(&KeyChord::command_shift("p")),
            Some(KeyAction::CommandPalette)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("p")),
            Some(KeyAction::QuickConnect)
        );
    }

    /// 对照 `testCmdFontZoomShortcuts`。
    #[test]
    fn command_font_zoom_shortcuts() {
        assert_eq!(
            builtin_action(&KeyChord::command("=")),
            Some(KeyAction::IncreaseFontSize)
        );
        assert_eq!(
            builtin_action(&KeyChord::command_shift("=")),
            Some(KeyAction::IncreaseFontSize)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("+")),
            Some(KeyAction::IncreaseFontSize)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("-")),
            Some(KeyAction::DecreaseFontSize)
        );
        assert_eq!(
            builtin_action(&KeyChord::command_shift("0")),
            Some(KeyAction::ResetFontSize)
        );
    }

    /// 对照 `testCmdShiftLCyclesLayout`。
    #[test]
    fn command_shift_l_cycles_layout() {
        assert_eq!(
            builtin_action(&KeyChord::command_shift("l")),
            Some(KeyAction::CycleLayout)
        );
    }

    /// 对照 `testCmdEnterTogglesPaneFullscreen`。
    #[test]
    fn command_enter_toggles_pane_fullscreen() {
        assert_eq!(
            builtin_action(&KeyChord::command("\r")),
            Some(KeyAction::TogglePaneFullscreen)
        );
        assert_eq!(
            builtin_action(&KeyChord::command("\n")),
            Some(KeyAction::TogglePaneFullscreen)
        );
    }

    /// 对照 `testAltEnterTogglesPaneFullscreen`。
    #[test]
    fn alt_enter_toggles_pane_fullscreen() {
        assert_eq!(
            builtin_action(&KeyChord::option("\r")),
            Some(KeyAction::TogglePaneFullscreen)
        );
        assert_eq!(
            builtin_action(&KeyChord::option("\n")),
            Some(KeyAction::TogglePaneFullscreen)
        );
        assert_eq!(
            builtin_action(&KeyChord::new(true, false, true, false, "\r")),
            None
        );
    }

    // === ShortcutEditorTests（配置侧）===

    /// 对照 `testPresetStyleOverridesKeepBuiltinChords`：非空 override 是追加，
    /// 不能把内置 Cmd 键一起压掉。
    #[test]
    fn preset_style_overrides_keep_builtin_chords() {
        let values = json!({
            "shortcuts": {
                "preset": "qwerty",
                "primary_key": "auto",
                "overrides": [
                    {"action": "switch_tab_1", "bindings": [{"key": "1", "modifiers": ["alt"]}]},
                    {"action": "switch_pane_prev", "bindings": [{"key": "[", "modifiers": ["alt"]}]},
                    {"action": "switch_pane_next", "bindings": [{"key": "]", "modifiers": ["alt"]}]}
                ]
            }
        });
        let custom = parse_values(&values);
        let suppressed = suppressed_actions(&values);
        assert!(suppressed.is_empty(), "非空 override 不是禁用");
        for chord in [
            KeyChord::command("1"),
            KeyChord::command("["),
            KeyChord::command("]"),
        ] {
            assert!(
                action(&chord, Some(&custom), &suppressed).is_some(),
                "内置键必须仍然生效: {}",
                chord.key
            );
        }
        // Alt 追加绑定也生效。
        assert_eq!(
            action(&KeyChord::option("1"), Some(&custom), &suppressed),
            Some(KeyAction::SwitchTab(1))
        );
    }

    /// 对照 `testEmptyOverrideSuppressesBuiltinChord`：空 bindings 才是真禁用。
    #[test]
    fn empty_override_suppresses_builtin_chord() {
        let values = json!({
            "shortcuts": {
                "overrides": [{"action": "switch_pane_next", "bindings": []}]
            }
        });
        let custom = parse_values(&values);
        let suppressed = suppressed_actions(&values);
        assert_eq!(suppressed, HashSet::from([KeyAction::NextPane]));
        assert_eq!(
            action(&KeyChord::command("]"), Some(&custom), &suppressed),
            None
        );
        assert!(
            action(&KeyChord::command("["), Some(&custom), &suppressed).is_some(),
            "只禁用被显式关闭的那一个动作"
        );
    }

    /// 旧 `[[keybindings]]` 段也吃，且 mods 里的 `super` / `alt` 别名要认。
    #[test]
    fn legacy_keybindings_section_is_parsed() {
        let values = json!({
            "keybindings": [
                {"key": "d", "mods": ["super", "shift"], "action": "new_pane"},
                {"key": "Return", "mods": ["alt"], "action": "new_tab"},
                {"key": "plus", "mods": ["command"], "action": "increase_font_size"},
                {"key": "d", "mods": ["alt"], "action": "자모"} // 未知 action 被丢弃
            ]
        });
        let custom = parse_values(&values);
        assert_eq!(
            custom.get(&KeyChord::command_shift("d")),
            Some(&KeyAction::SplitHorizontal)
        );
        assert_eq!(
            custom.get(&KeyChord::option("\r")),
            Some(&KeyAction::NewTab),
            "return 必须归一成 CR"
        );
        assert_eq!(
            custom.get(&KeyChord::command("+")),
            Some(&KeyAction::IncreaseFontSize),
            "plus 必须归一成 +"
        );
        assert_eq!(custom.len(), 3, "未知 action 不进表");
    }

    /// 对照 `testOverrideDisablesBuiltinChord` 的语义：override 值覆盖同键默认。
    #[test]
    fn override_values_win_over_builtin() {
        let values = json!({
            "shortcuts": {
                "overrides": [{"action": "open_agents", "bindings": [{"key": "t", "modifiers": ["command"]}]}]
            }
        });
        let custom = parse_values(&values);
        assert_eq!(
            action(&KeyChord::command("t"), Some(&custom), &HashSet::new()),
            Some(KeyAction::OpenAgents),
            "自定义键位优先于内置"
        );
        assert_eq!(
            action(&KeyChord::command("d"), Some(&custom), &HashSet::new()),
            Some(KeyAction::SplitHorizontal),
            "未覆盖的键仍走默认"
        );
    }

    /// action 名与动作的双向映射必须闭合，否则设置窗口写不回去。
    #[test]
    fn action_names_round_trip() {
        let cases = [
            KeyAction::NewTab,
            KeyAction::SplitHorizontal,
            KeyAction::SplitVertical,
            KeyAction::SplitAuto,
            KeyAction::CloseWindow,
            KeyAction::ClosePane,
            KeyAction::SwitchTab(3),
            KeyAction::SwitchLastTab,
            KeyAction::PreviousTab,
            KeyAction::NextTab,
            KeyAction::NextPane,
            KeyAction::PrevPane,
            KeyAction::PreviousCommand,
            KeyAction::NextCommand,
            KeyAction::CommandPalette,
            KeyAction::QuickConnect,
            KeyAction::Attention,
            KeyAction::SearchWorkspace,
            KeyAction::SearchGlobal,
            KeyAction::Quit,
            KeyAction::IncreaseFontSize,
            KeyAction::DecreaseFontSize,
            KeyAction::ResetFontSize,
            KeyAction::TogglePaneFullscreen,
            KeyAction::ToggleWindowFullscreen,
            KeyAction::ToggleWindowZoom,
            KeyAction::OpenSettings,
            KeyAction::OpenComposer,
            KeyAction::CycleLayout,
            KeyAction::ToggleSidebar,
            KeyAction::OpenShells,
            KeyAction::OpenAgents,
            KeyAction::SwitchWorkspace(0),
            KeyAction::SwitchWorkspace(4),
        ];
        for case in cases {
            let name = action_name(case).expect("每个动作都要有配置名");
            assert_eq!(action_from_name(&name), Some(case), "往返失败: {name}");
        }
    }

    /// 配置名解析不接受越界编号，避免 `switch_tab_10` 这种假键位。
    #[test]
    fn out_of_range_action_names_are_rejected() {
        assert_eq!(action_from_name("switch_tab_10"), None);
        assert_eq!(action_from_name("switch_workspace_10"), None);
        assert_eq!(action_from_name("switch_tab_0"), None);
        assert_eq!(action_from_name("nonsense"), None);
    }
}
