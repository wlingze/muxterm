//! macOS 主菜单的纯数据描述。
//!
//! 对照 `src/frontend/macos/App/AppDelegate.swift::buildMenu`：结构、顺序、
//! 快捷键都照抄；差别只在「gpui 还没有实现的动作」——那些项仍然出现在
//! 菜单里（用户看到的是同一份菜单），但 `action == None`，平台层把它禁用。
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
    /// `None`＝Swift 有、gpui 还没接的动作（菜单里灰显）。
    pub action: Option<KeyAction>,
    /// 显示在菜单右侧的快捷键；`None` 表示没有。
    pub key: Option<MenuKey>,
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

/// macOS 主菜单：App / File / Window / Edit / View，顺序与 Swift 一致。
pub fn main_menu() -> Vec<Menu> {
    vec![
        app_menu(),
        file_menu(),
        window_menu(),
        edit_menu(),
        view_menu(),
    ]
}

/// 第一项是应用菜单：标题会被 macOS 换成 app 名。
fn app_menu() -> Menu {
    Menu {
        title: "Muxterm".to_string(),
        entries: vec![
            item("About Muxterm", None, None),
            // 对照 `MainWindowController.openPreferences`：Cmd-, 打开设置。
            item(
                "Preferences…",
                Some(KeyAction::OpenSettings),
                Some(MenuKey::command(",")),
            ),
            // 自更新面板尚未接入 gpui。
            item("Check for Updates…", None, None),
            MenuEntry::Separator,
            item(
                "Quit Muxterm",
                Some(KeyAction::Quit),
                Some(MenuKey::command("q")),
            ),
        ],
    }
}

fn file_menu() -> Menu {
    Menu {
        title: "File".to_string(),
        entries: vec![
            item(
                "New Tab",
                Some(KeyAction::NewTab),
                Some(MenuKey::command("t")),
            ),
            // 重命名 workspace 需要 Core 的 rename 接口，gpui 还没接。
            item("Rename Workspace…", None, None),
            item(
                "Close Pane",
                Some(KeyAction::ClosePane),
                Some(MenuKey::command("w")),
            ),
            item(
                "Close Window",
                Some(KeyAction::CloseWindow),
                Some(MenuKey::command_shift("w")),
            ),
        ],
    }
}

fn window_menu() -> Menu {
    let mut entries = vec![
        item("Rename Tab…", None, None),
        item("Move Tab Left", None, None),
        item("Move Tab Right", None, None),
        MenuEntry::Separator,
        item(
            "Previous Tab",
            Some(KeyAction::PreviousTab),
            Some(MenuKey::command("{")),
        ),
        item(
            "Next Tab",
            Some(KeyAction::NextTab),
            Some(MenuKey::command("}")),
        ),
    ];
    for number in 1..=9 {
        entries.push(item(
            &format!("Switch to Tab {number}"),
            Some(KeyAction::SwitchTab(number)),
            Some(MenuKey::command(&number.to_string())),
        ));
    }
    entries.push(item(
        "Switch to Last Tab",
        Some(KeyAction::SwitchLastTab),
        Some(MenuKey::command("0")),
    ));
    Menu {
        title: "Window".to_string(),
        entries,
    }
}

fn edit_menu() -> Menu {
    Menu {
        title: "Edit".to_string(),
        entries: vec![
            // Copy 走终端选区；Select All 还没有全选实现（不带快捷键，菜单里置灰）。
            item("Copy", Some(KeyAction::Copy), Some(MenuKey::command("c"))),
            item("Paste", Some(KeyAction::Paste), Some(MenuKey::command("v"))),
            item("Select All", None, None),
        ],
    }
}

fn view_menu() -> Menu {
    Menu {
        title: "View".to_string(),
        entries: vec![
            item(
                "Split Horizontal",
                Some(KeyAction::SplitHorizontal),
                Some(MenuKey::command("d")),
            ),
            item(
                "Split Vertical",
                Some(KeyAction::SplitVertical),
                Some(MenuKey::command("s")),
            ),
            item("Split Auto", Some(KeyAction::SplitAuto), None),
            // 把 pane 移到新 tab 的 Core 任务还没接。
            item("Move Pane to New Tab", None, None),
            item(
                "Toggle Pane Fullscreen",
                Some(KeyAction::TogglePaneFullscreen),
                Some(MenuKey::command("\r")),
            ),
            item(
                "Next Pane",
                Some(KeyAction::NextPane),
                Some(MenuKey::command("]")),
            ),
            item(
                "Previous Pane",
                Some(KeyAction::PrevPane),
                Some(MenuKey::command("[")),
            ),
            item(
                "Previous Command",
                Some(KeyAction::PreviousCommand),
                Some(MenuKey::command_option("up")),
            ),
            item(
                "Next Command",
                Some(KeyAction::NextCommand),
                Some(MenuKey::command_option("down")),
            ),
            item(
                "Increase Font Size",
                Some(KeyAction::IncreaseFontSize),
                Some(MenuKey::command_shift("=")),
            ),
            item(
                "Decrease Font Size",
                Some(KeyAction::DecreaseFontSize),
                Some(MenuKey::command("-")),
            ),
            item(
                "Reset Font Size",
                Some(KeyAction::ResetFontSize),
                Some(MenuKey::command_shift("0")),
            ),
            MenuEntry::Separator,
            item(
                "Quick Connect…",
                Some(KeyAction::QuickConnect),
                Some(MenuKey::command("p")),
            ),
            item(
                "Shells",
                Some(KeyAction::OpenShells),
                Some(MenuKey::command_control("s")),
            ),
            item(
                "Agents",
                Some(KeyAction::OpenAgents),
                Some(MenuKey::command_control("a")),
            ),
            item(
                "Command Palette…",
                Some(KeyAction::CommandPalette),
                Some(MenuKey::command_shift("p")),
            ),
            item(
                "Composer",
                Some(KeyAction::OpenComposer),
                Some(MenuKey::command("i")),
            ),
            item(
                "Search Panes",
                Some(KeyAction::SearchWorkspace),
                Some(MenuKey::command("f")),
            ),
            item("Search All Panes", Some(KeyAction::SearchGlobal), None),
            item(
                "Attention",
                Some(KeyAction::Attention),
                Some(MenuKey::command("r")),
            ),
            MenuEntry::Separator,
            item("Move Tab Bar to Top", None, None),
            item("Move Tab Bar to Bottom", None, None),
        ],
    }
}

fn item(label: &str, action: Option<KeyAction>, key: Option<MenuKey>) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        label: label.to_string(),
        action,
        key,
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
        // 1..9 的动作编号必须与显示一致。
        let switch_eight = find(&menus, "Window")
            .entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.label == "Switch to Tab 8" => Some(item),
                _ => None,
            })
            .expect("Switch to Tab 8");
        assert_eq!(switch_eight.action, Some(KeyAction::SwitchTab(8)));
    }

    /// 每个带快捷键的项都要有可执行动作；没有动作的项不能带快捷键，
    /// 否则用户按下去没反应（Swift 的这些项都指向真实 selector）。
    #[test]
    fn shortcut_items_are_all_executable() {
        for menu in main_menu() {
            for entry in menu.entries {
                if let MenuEntry::Item(item) = entry {
                    assert!(
                        item.key.is_none() || item.action.is_some(),
                        "{} 有快捷键但没有动作",
                        item.label
                    );
                }
            }
        }
    }

    #[test]
    fn settings_and_quit_keep_their_macos_shortcuts() {
        let menus = main_menu();
        let app = find(&menus, "Muxterm");
        let preferences = app
            .entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.label.starts_with("Preferences") => Some(item),
                _ => None,
            })
            .expect("Preferences");
        assert_eq!(preferences.action, Some(KeyAction::OpenSettings));
        assert_eq!(preferences.key.as_ref().unwrap().key, ",");
        assert!(preferences.key.as_ref().unwrap().command);

        let quit = app
            .entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.label.starts_with("Quit") => Some(item),
                _ => None,
            })
            .expect("Quit");
        assert_eq!(quit.action, Some(KeyAction::Quit));
        assert_eq!(quit.key.as_ref().unwrap().key, "q");
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
            let (Some(key), Some(action)) = (item.key.as_ref(), item.action) else {
                continue;
            };
            if !key.command {
                continue;
            }
            let chord = KeyChord::new(key.command, key.shift, key.option, key.control, &key.key);
            assert_eq!(
                builtin_action(&chord),
                Some(action),
                "{} 的菜单快捷键与键表不一致",
                item.label
            );
        }
    }
}
