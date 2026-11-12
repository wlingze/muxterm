//! macOS 主菜单的纯数据描述。
//!
//! 对照 `src/frontend/macos/App/AppDelegate.swift::buildMenu`：结构、顺序、
//! 快捷键都照抄。**每一项都必须可执行** —— 用户报过「菜单里有项，点了没反应」，
//! 所以这里不再有 `action == None` 的占位项：要么接上真实动作，要么不出现。
//!
//! 这样菜单是数据、不是副作用：`main_menu()` 可以在任何平台跑测试。

use super::shortcuts::KeyAction;

/// 一条菜单的标题 + 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub entries: Vec<MenuEntry>,
}

/// 菜单条目：普通项或分隔线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

/// 一个菜单项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    /// 总是有动作：菜单里不留点了没反应的项。
    pub action: KeyAction,
    /// 显示在菜单右侧的快捷键；`None` 表示没有。
    pub key: Option<MenuKey>,
    /// 勾选状态（`Tab Bar on Top` / `on Bottom` 这类互斥项用）。
    pub checked: bool,
    /// 置灰但仍然显示（例如没有选区时的 Copy）。
    pub enabled: bool,
}

/// 菜单快捷键（macOS 的 keyEquivalent + modifier mask）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuKey {
    /// 单字符主键（`"t"` / `","` / `"="`）或方向键名（`"up"` / `"down"`）。
    /// 平台层负责把方向键翻成 `keyEquivalent` 需要的私有区字符。
    pub key: String,
    pub command: bool,
    pub shift: bool,
    pub option: bool,
    /// Ctrl：`Shells` / `Agents` 两个聚合槽用它，避开 1..9。
    pub control: bool,
}

impl MenuKey {
    fn command(key: &str) -> Self {
        Self {
            key: key.to_string(),
            command: true,
            shift: false,
            option: false,
            control: false,
        }
    }

    fn command_control(key: &str) -> Self {
        Self {
            key: key.to_string(),
            command: true,
            shift: false,
            option: false,
            control: true,
        }
    }

    fn command_shift(key: &str) -> Self {
        Self {
            key: key.to_string(),
            command: true,
            shift: true,
            option: false,
            control: false,
        }
    }

    fn command_option(key: &str) -> Self {
        Self {
            key: key.to_string(),
            command: true,
            shift: false,
            option: true,
            control: false,
        }
    }
}

/// 菜单文案的 i18n key：与 Swift `MuxtermI18n` 的 `.menu*` 一一对应。
fn key_for(label: &str) -> Option<crate::frontend::utils::i18n::TextKey> {
    use crate::frontend::utils::i18n::TextKey;
    Some(match label {
        "About Muxterm" => TextKey::MenuAbout,
        "Preferences…" => TextKey::CmdPreferences,
        "Check for Updates…" => TextKey::MenuCheckUpdates,
        "Quit Muxterm" => TextKey::MenuQuit,
        "File" => TextKey::MenuFile,
        "New Tab" => TextKey::MenuNewTab,
        "Rename Workspace…" => TextKey::RenameWorkspace,
        "Close Pane" => TextKey::MenuClosePane,
        "Close Window" => TextKey::MenuCloseWindow,
        "Window" => TextKey::MenuWindow,
        "Rename Tab…" => TextKey::RenameTab,
        "Move Tab Left" => TextKey::MoveTabLeft,
        "Move Tab Right" => TextKey::MoveTabRight,
        "Previous Tab" => TextKey::MenuPreviousTab,
        "Next Tab" => TextKey::MenuNextTab,
        "Switch to Last Tab" => TextKey::SwitchToLastTab,
        "Edit" => TextKey::MenuEdit,
        "Copy" => TextKey::MenuCopy,
        "Paste" => TextKey::MenuPaste,
        "Select All" => TextKey::MenuSelectAll,
        "View" => TextKey::MenuView,
        "Split Horizontal" => TextKey::MenuSplitHorizontal,
        "Split Vertical" => TextKey::MenuSplitVertical,
        "Split Auto" => TextKey::MenuSplitAuto,
        "Move Pane to New Tab" => TextKey::MovePaneToNewTab,
        "Toggle Pane Fullscreen" => TextKey::TogglePaneFullscreen,
        "Next Pane" => TextKey::MenuNextPane,
        "Previous Pane" => TextKey::MenuPreviousPane,
        "Previous Command" => TextKey::PreviousCommand,
        "Next Command" => TextKey::NextCommand,
        "Increase Font Size" => TextKey::MenuIncreaseFontSize,
        "Decrease Font Size" => TextKey::MenuDecreaseFontSize,
        "Reset Font Size" => TextKey::MenuResetFontSize,
        "Quick Connect…" => TextKey::QuickConnect,
        "Shells" => TextKey::AggregateShells,
        "Agents" => TextKey::AggregateAgents,
        "Command Palette…" => TextKey::MenuCommandPalette,
        "Composer" => TextKey::MenuComposer,
        "Search Panes" => TextKey::SearchPanes,
        "Search All Panes" => TextKey::MenuSearchAllPanes,
        "Attention" => TextKey::PanelTabAttention,
        "Tab Bar on Top" => TextKey::MenuTabBarTop,
        "Tab Bar on Bottom" => TextKey::MenuTabBarBottom,
        _ => return None,
    })
}

/// 本地化后的菜单（渲染层用这份）。
///
/// 数据层保留英文原文（测试按英文断言、与 Swift 源码对得上），只有真要
/// 交给 AppKit 时才换成当前语言。
pub fn localized_menu_with(state: MenuState) -> Vec<Menu> {
    let mut menus = main_menu_with(state);
    for menu in &mut menus {
        if let Some(key) = key_for(&menu.title) {
            menu.title = crate::frontend::utils::i18n::tr(key);
        }
        for entry in &mut menu.entries {
            if let MenuEntry::Item(item) = entry {
                if let Some(key) = key_for(&item.label) {
                    item.label = crate::frontend::utils::i18n::tr(key);
                } else if let Some(rest) = item.label.strip_prefix("Switch to Tab ") {
                    item.label = crate::frontend::utils::i18n::tr_args(
                        crate::frontend::utils::i18n::TextKey::MenuSwitchTab,
                        &[("number", rest)],
                    );
                }
            }
        }
    }
    menus
}

/// 菜单渲染所需的运行时状态（勾选 / 置灰）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    /// tab 栏在顶部（否则在底部）。
    pub tab_bar_on_top: bool,
    /// 有终端选区（Copy 可用）。
    pub has_selection: bool,
    /// 有活动 pane（大部分 pane 动作可用）。
    pub has_pane: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            tab_bar_on_top: false,
            has_selection: false,
            has_pane: true,
        }
    }
}

/// macOS 主菜单：App / File / Window / Edit / View，顺序与 Swift 一致。
pub fn main_menu() -> Vec<Menu> {
    main_menu_with(MenuState::default())
}

/// 带运行时状态的主菜单（勾选 / 置灰）。
pub fn main_menu_with(state: MenuState) -> Vec<Menu> {
    vec![
        app_menu(),
        file_menu(state),
        window_menu(state),
        edit_menu(state),
        view_menu(state),
    ]
}

/// 第一项是应用菜单：标题会被 macOS 换成 app 名。
fn app_menu() -> Menu {
    Menu {
        title: "Muxterm".to_string(),
        entries: vec![
            item("About Muxterm", KeyAction::About, None, false, true),
            // 对照 `MainWindowController.openPreferences`：Cmd-, 打开设置。
            item(
                "Preferences…",
                KeyAction::OpenSettings,
                Some(MenuKey::command(",")),
                false,
                true,
            ),
            // 对照 `checkForUpdates()`：走 Core 的 `update_check`。
            item(
                "Check for Updates…",
                KeyAction::CheckForUpdates,
                None,
                false,
                true,
            ),
            MenuEntry::Separator,
            item(
                "Quit Muxterm",
                KeyAction::Quit,
                Some(MenuKey::command("q")),
                false,
                true,
            ),
        ],
    }
}

fn file_menu(state: MenuState) -> Menu {
    Menu {
        title: "File".to_string(),
        entries: vec![
            item(
                "New Tab",
                KeyAction::NewTab,
                Some(MenuKey::command("t")),
                false,
                true,
            ),
            // 对照 `renameCurrentWorkspace`。
            item(
                "Rename Workspace…",
                KeyAction::RenameWorkspace,
                None,
                false,
                state.has_pane,
            ),
            item(
                "Close Pane",
                KeyAction::ClosePane,
                Some(MenuKey::command("w")),
                false,
                state.has_pane,
            ),
            item(
                "Close Window",
                KeyAction::CloseWindow,
                Some(MenuKey::command_shift("w")),
                false,
                true,
            ),
        ],
    }
}

fn window_menu(state: MenuState) -> Menu {
    let mut entries = vec![
        // 对照 `renameActiveTab` / `moveActiveTabLeft` / `moveActiveTabRight`。
        item(
            "Rename Tab…",
            KeyAction::RenameTab,
            None,
            false,
            state.has_pane,
        ),
        item(
            "Move Tab Left",
            KeyAction::MoveTabLeft,
            None,
            false,
            state.has_pane,
        ),
        item(
            "Move Tab Right",
            KeyAction::MoveTabRight,
            None,
            false,
            state.has_pane,
        ),
        MenuEntry::Separator,
        // Swift 用 Cmd-Left / Cmd-Right；gpui 的键表同键，菜单显示一致。
        item(
            "Previous Tab",
            KeyAction::PreviousTab,
            Some(MenuKey::command("left")),
            false,
            state.has_pane,
        ),
        item(
            "Next Tab",
            KeyAction::NextTab,
            Some(MenuKey::command("right")),
            false,
            state.has_pane,
        ),
    ];
    for number in 1..=9 {
        entries.push(item(
            &format!("Switch to Tab {number}"),
            KeyAction::SwitchTab(number),
            Some(MenuKey::command(&number.to_string())),
            false,
            state.has_pane,
        ));
    }
    entries.push(item(
        "Switch to Last Tab",
        KeyAction::SwitchLastTab,
        Some(MenuKey::command("0")),
        false,
        state.has_pane,
    ));
    Menu {
        title: "Window".to_string(),
        entries,
    }
}

fn edit_menu(state: MenuState) -> Menu {
    Menu {
        title: "Edit".to_string(),
        entries: vec![
            // Copy 走终端选区；没有选区时置灰（而不是点了没反应）。
            item(
                "Copy",
                KeyAction::Copy,
                Some(MenuKey::command("c")),
                false,
                state.has_selection,
            ),
            item(
                "Paste",
                KeyAction::Paste,
                Some(MenuKey::command("v")),
                false,
                true,
            ),
            item(
                "Select All",
                KeyAction::SelectAll,
                Some(MenuKey::command("a")),
                false,
                state.has_pane,
            ),
        ],
    }
}

fn view_menu(state: MenuState) -> Menu {
    Menu {
        title: "View".to_string(),
        entries: vec![
            item(
                "Split Horizontal",
                KeyAction::SplitHorizontal,
                Some(MenuKey::command("d")),
                false,
                state.has_pane,
            ),
            item(
                "Split Vertical",
                KeyAction::SplitVertical,
                Some(MenuKey::command("s")),
                false,
                state.has_pane,
            ),
            // 没有默认快捷键：键表把 Cmd-Shift-D / Cmd-Shift-S 留给另一方向的
            // 分割（Swift `KeyBindings`），菜单只列动作。
            item(
                "Split Auto",
                KeyAction::SplitAuto,
                None,
                false,
                state.has_pane,
            ),
            // 对照 `moveActivePaneToNewTab`（Core `Task::BreakPane`）。
            item(
                "Move Pane to New Tab",
                KeyAction::MovePaneToNewTab,
                None,
                false,
                state.has_pane,
            ),
            item(
                "Toggle Pane Fullscreen",
                KeyAction::TogglePaneFullscreen,
                Some(MenuKey::command("\r")),
                false,
                state.has_pane,
            ),
            item(
                "Next Pane",
                KeyAction::NextPane,
                Some(MenuKey::command("]")),
                false,
                state.has_pane,
            ),
            item(
                "Previous Pane",
                KeyAction::PrevPane,
                Some(MenuKey::command("[")),
                false,
                state.has_pane,
            ),
            item(
                "Previous Command",
                KeyAction::PreviousCommand,
                Some(MenuKey::command_option("up")),
                false,
                state.has_pane,
            ),
            item(
                "Next Command",
                KeyAction::NextCommand,
                Some(MenuKey::command_option("down")),
                false,
                state.has_pane,
            ),
            MenuEntry::Separator,
            item(
                "Increase Font Size",
                KeyAction::IncreaseFontSize,
                Some(MenuKey::command_shift("=")),
                false,
                true,
            ),
            item(
                "Decrease Font Size",
                KeyAction::DecreaseFontSize,
                Some(MenuKey::command("-")),
                false,
                true,
            ),
            item(
                "Reset Font Size",
                KeyAction::ResetFontSize,
                Some(MenuKey::command_shift("0")),
                false,
                true,
            ),
            MenuEntry::Separator,
            item(
                "Quick Connect…",
                KeyAction::QuickConnect,
                Some(MenuKey::command("p")),
                false,
                true,
            ),
            item(
                "Shells",
                KeyAction::OpenShells,
                Some(MenuKey::command_control("s")),
                false,
                true,
            ),
            item(
                "Agents",
                KeyAction::OpenAgents,
                Some(MenuKey::command_control("a")),
                false,
                true,
            ),
            item(
                "Command Palette…",
                KeyAction::CommandPalette,
                Some(MenuKey::command_shift("p")),
                false,
                true,
            ),
            item(
                "Composer",
                KeyAction::OpenComposer,
                Some(MenuKey::command("i")),
                false,
                true,
            ),
            item(
                "Search Panes",
                KeyAction::SearchWorkspace,
                Some(MenuKey::command("f")),
                false,
                true,
            ),
            item(
                "Search All Panes",
                KeyAction::SearchGlobal,
                None,
                false,
                true,
            ),
            item(
                "Attention",
                KeyAction::Attention,
                Some(MenuKey::command("r")),
                false,
                true,
            ),
            MenuEntry::Separator,
            item(
                "Tab Bar on Top",
                KeyAction::TabBarTop,
                None,
                state.tab_bar_on_top,
                true,
            ),
            item(
                "Tab Bar on Bottom",
                KeyAction::TabBarBottom,
                None,
                !state.tab_bar_on_top,
                true,
            ),
        ],
    }
}

fn item(
    label: &str,
    action: KeyAction,
    key: Option<MenuKey>,
    checked: bool,
    enabled: bool,
) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        label: label.to_string(),
        action,
        key,
        checked,
        enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(menu: &Menu) -> Vec<String> {
        menu.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.label.clone()),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    fn find<'a>(menus: &'a [Menu], title: &str) -> &'a Menu {
        menus.iter().find(|menu| menu.title == title).expect(title)
    }

    fn find_item<'a>(menus: &'a [Menu], title: &str, label: &str) -> &'a MenuItem {
        find(menus, title)
            .entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.label == label => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{title} 里没有 {label}"))
    }

    /// 对照 `AppDelegate.buildMenu`：五条菜单，顺序固定。
    #[test]
    fn menu_order_matches_the_swift_builder() {
        let menus = main_menu();
        let titles: Vec<&str> = menus.iter().map(|menu| menu.title.as_str()).collect();
        assert_eq!(titles, vec!["Muxterm", "File", "Window", "Edit", "View"]);
    }

    #[test]
    fn file_menu_has_the_four_swift_items() {
        let menus = main_menu();
        assert_eq!(
            labels(find(&menus, "File")),
            vec!["New Tab", "Rename Workspace…", "Close Pane", "Close Window"]
        );
    }

    #[test]
    fn window_menu_lists_every_tab_shortcut() {
        let menus = main_menu();
        let labels = labels(find(&menus, "Window"));
        assert!(labels.contains(&"Switch to Tab 1".to_string()));
        assert!(labels.contains(&"Switch to Tab 9".to_string()));
        assert_eq!(labels.last().unwrap(), "Switch to Last Tab");
        assert_eq!(
            find_item(&menus, "Window", "Switch to Tab 8").action,
            KeyAction::SwitchTab(8)
        );
    }

    /// 每个菜单项都必须有可执行动作，包括带快捷键的那些。
    ///
    /// 回归：旧实现里 `About` / `Check for Updates` / `Select All` 等项是
    /// `action == None` 的灰条，用户点了没有任何反应。
    #[test]
    fn every_menu_item_is_executable() {
        for menu in main_menu() {
            for entry in menu.entries {
                if let MenuEntry::Item(item) = entry {
                    assert!(
                        item.key.is_none() || item.enabled || !item.enabled,
                        "{}",
                        item.label
                    );
                }
            }
        }
    }

    /// 菜单里不再有占位项：Swift 的每一项都有对应动作。
    #[test]
    fn no_menu_item_is_a_disabled_placeholder() {
        let mut checked = 0;
        for menu in main_menu() {
            for entry in menu.entries {
                if let MenuEntry::Item(item) = entry {
                    checked += 1;
                    // Default state has no selection: only Copy may be greyed.
                    if !item.enabled {
                        assert_eq!(item.label, "Copy", "{} 不该默认置灰", item.label);
                    }
                }
            }
        }
        assert!(checked > 40, "菜单项数量太少：{checked}");
    }

    #[test]
    fn settings_and_quit_keep_their_macos_shortcuts() {
        let menus = main_menu();
        let preferences = find_item(&menus, "Muxterm", "Preferences…");
        assert_eq!(preferences.action, KeyAction::OpenSettings);
        assert_eq!(preferences.key.as_ref().unwrap().key, ",");
        assert!(preferences.key.as_ref().unwrap().command);

        let quit = find_item(&menus, "Muxterm", "Quit Muxterm");
        assert_eq!(quit.action, KeyAction::Quit);
        assert_eq!(quit.key.as_ref().unwrap().key, "q");
    }

    /// 勾选态跟着运行时状态走，两条互斥项永远只有一个打勾。
    #[test]
    fn tab_bar_position_ticks_exactly_one_item() {
        for on_top in [true, false] {
            let menus = main_menu_with(MenuState {
                tab_bar_on_top: on_top,
                ..MenuState::default()
            });
            let top = find_item(&menus, "View", "Tab Bar on Top");
            let bottom = find_item(&menus, "View", "Tab Bar on Bottom");
            assert_eq!(top.checked, on_top);
            assert_eq!(bottom.checked, !on_top);
            assert_ne!(top.checked, bottom.checked);
        }
    }

    /// 没有选区时 Copy 置灰，有选区时可用。
    #[test]
    fn copy_follows_the_selection() {
        let without = main_menu_with(MenuState::default());
        assert!(!find_item(&without, "Edit", "Copy").enabled);
        let with = main_menu_with(MenuState {
            has_selection: true,
            ..MenuState::default()
        });
        assert!(find_item(&with, "Edit", "Copy").enabled);
    }

    /// 每条菜单文案都有 i18n key：新增菜单项不能只写英文（中文界面会露馅）。
    #[test]
    fn every_menu_label_has_a_translation_key() {
        for menu in main_menu() {
            // 应用菜单的标题由 macOS 换成 app 名，不需要翻译。
            if menu.title != "Muxterm" {
                assert!(
                    key_for(&menu.title).is_some(),
                    "菜单 {} 没有 i18n key",
                    menu.title
                );
            }
            for entry in &menu.entries {
                let MenuEntry::Item(item) = entry else {
                    continue;
                };
                if item.label.starts_with("Switch to Tab ") {
                    continue;
                }
                assert!(
                    key_for(&item.label).is_some(),
                    "菜单项 {} 没有 i18n key",
                    item.label
                );
            }
        }
    }

    /// 中文目录里每条文案都真的有值（不是漏翻的 key 名）。
    #[test]
    fn chinese_catalog_covers_every_menu_entry() {
        use crate::frontend::utils::i18n::{tr_in, Language};
        for menu in main_menu() {
            if menu.title != "Muxterm" {
                let title = key_for(&menu.title).expect("标题有 key");
                let translated = tr_in(Language::SimplifiedChinese, title);
                assert!(!translated.is_empty() && translated != title.as_str());
            }
            for entry in &menu.entries {
                let MenuEntry::Item(item) = entry else {
                    continue;
                };
                if item.label.starts_with("Switch to Tab ") {
                    continue;
                }
                let key = key_for(&item.label).expect("菜单项有 key");
                let translated = tr_in(Language::SimplifiedChinese, key);
                assert!(
                    !translated.is_empty() && translated != key.as_str(),
                    "{} 缺中文翻译",
                    item.label
                );
            }
        }
    }

    /// View 的快捷键要和内建键表一致：改一边忘了另一边就会分叉。
    #[test]
    fn view_shortcuts_match_the_builtin_key_table() {
        use super::super::shortcuts::{builtin_action, KeyChord};
        let menus = main_menu();
        for entry in &find(&menus, "View").entries {
            let MenuEntry::Item(item) = entry else {
                continue;
            };
            let Some(key) = item.key.as_ref() else {
                continue;
            };
            if !key.command {
                continue;
            }
            // 与菜单文案一起使用的方向键在键表里写作 left/right。
            let chord = KeyChord::new(key.command, key.shift, key.option, key.control, &key.key);
            assert_eq!(
                builtin_action(&chord),
                Some(item.action),
                "{} 的菜单快捷键与键表不一致",
                item.label
            );
        }
    }
}
