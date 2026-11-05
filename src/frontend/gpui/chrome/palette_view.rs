//! Cmd-Shift-P 命令面板的投影（纯逻辑，无 gpui 依赖）。
//!
//! 对照 macOS `App/CommandPalette.swift` 与 `MainWindow.rootPaletteItems()`：
//! 一份命令目录 + 大小写无关的整串包含过滤 + 上下移动 + 回车执行。
//!
//! 前端还没有对应实现的命令（改名、语言、状态栏模式等）仍然列出来，
//! 只是标成不可执行 —— 与 macOS 面板保持同一份目录，而不是悄悄少几条。

use crate::frontend::gpui::model::shortcuts::KeyAction;

/// 面板里一行命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub title: String,
    pub detail: String,
    /// 额外搜索词（英文 + 中文，与 macOS 的 `keywords` 同义）。
    pub keywords: String,
    pub action: PaletteAction,
}

impl PaletteItem {
    fn new(title: &str, detail: &str, keywords: &str, action: PaletteAction) -> Self {
        Self {
            title: title.to_string(),
            detail: detail.to_string(),
            keywords: keywords.to_string(),
            action,
        }
    }

    /// 过滤用的整串：标题 + 说明 + 关键词，全部小写。
    pub fn search_text(&self) -> String {
        format!("{} {} {}", self.title, self.detail, self.keywords).to_lowercase()
    }

    /// 命中的中文关键词也要能高亮出来，这里只回答“是否命中”。
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.search_text().contains(&query)
    }

    /// 这一行现在能不能执行。
    pub fn available(&self) -> bool {
        self.action != PaletteAction::Unavailable
    }
}

/// 一条命令对应的动作。窗口级动作复用 `KeyAction`，其余是面板独有的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    KeyAction(KeyAction),
    /// tab 栏挪到顶部 / 底部（写 Core `[ui] tab_bar_position`）。
    TabBarTop,
    TabBarBottom,
    /// 前端还没有实现：列出来但灰掉。
    Unavailable,
}

/// 面板状态：查询串 + 选中下标。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    /// 选中项在**过滤后**列表里的下标。
    pub selected: usize,
}

impl PaletteState {
    /// 打开时清空查询，选中第一行（与 macOS `present` 一致）。
    pub fn show(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn dismiss(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    /// 追加一个字符：查询变化后选中回到第一行（与 macOS 重新过滤一致）。
    pub fn push_char(&mut self, ch: char, _visible: usize) {
        self.query.push(ch);
        self.selected = 0;
    }

    /// 退格：选中项夹回新的列表范围。
    pub fn backspace(&mut self, visible: usize) {
        self.query.pop();
        self.selected = self.selected.min(visible.saturating_sub(1));
    }

    /// 上下移动，循环到底（macOS 的 `CompactPanelKeyNavigation` 不回绕，
    /// 这里按列表夹取）。
    pub fn move_selection(&mut self, delta: isize, visible: usize) {
        if visible == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, visible as isize - 1) as usize;
    }
}

/// 过滤：保持目录顺序，只留下命中项。
pub fn filter<'a>(items: &'a [PaletteItem], query: &str) -> Vec<&'a PaletteItem> {
    items.iter().filter(|item| item.matches(query)).collect()
}

/// 命令目录。`tabs` 是当前 tab 的名字（生成 `Switch to Tab N`）。
///
/// 对照 `MainWindow.rootPaletteItems()`：命令顺序一致，动态项插在同样的位置。
pub fn root_items(tabs: &[String]) -> Vec<PaletteItem> {
    let mut items = vec![
        PaletteItem::new(
            "Local",
            "Choose a local tmux session",
            "local tmux attach new 本地",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "SSH",
            "Choose an SSH host, then a remote tmux session",
            "ssh remote host tmux attach new 主机",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Quick Connect",
            "Open Workspaces, Attention, and Search",
            "quick connect workspace attention search 快速连接 工作区 提醒 搜索",
            PaletteAction::KeyAction(KeyAction::QuickConnect),
        ),
        PaletteItem::new(
            "New Tab",
            "Create a local tab",
            "new tab 新建 标签页",
            PaletteAction::KeyAction(KeyAction::NewTab),
        ),
        PaletteItem::new(
            "Rename Tab…",
            "Change the current tab name",
            "rename tab title 重命名 标签页",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Rename Workspace…",
            "Change the current workspace name",
            "rename workspace title 重命名 工作区",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Move Tab Left",
            "Move the current tab one position left",
            "move tab left reorder 向左 移动 标签页",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Move Tab Right",
            "Move the current tab one position right",
            "move tab right reorder 向右 移动 标签页",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Split Pane Horizontally",
            "Split the current pane horizontally",
            "split pane horizontal 水平 分割",
            PaletteAction::KeyAction(KeyAction::SplitHorizontal),
        ),
        PaletteItem::new(
            "Split Pane Vertically",
            "Split the current pane vertically",
            "split pane vertical 竖直 上下",
            PaletteAction::KeyAction(KeyAction::SplitVertical),
        ),
        PaletteItem::new(
            "Enter Window Full Screen",
            "⇧⌘F / ⇧⌘↩",
            "window fullscreen screen 窗口全屏 整个窗口",
            PaletteAction::KeyAction(KeyAction::ToggleWindowFullscreen),
        ),
        PaletteItem::new(
            "Fill the Desktop",
            "⇧⌘T",
            "zoom maximize desktop window 最大化 当前桌面 占满",
            PaletteAction::KeyAction(KeyAction::ToggleWindowZoom),
        ),
        PaletteItem::new(
            "Preferences",
            "⌘,",
            "settings preferences configuration 设置 偏好",
            PaletteAction::KeyAction(KeyAction::OpenSettings),
        ),
        PaletteItem::new(
            "Toggle Pane Zoom",
            "Zoom the current pane to fill the window (Cmd+Enter)",
            "pane fullscreen zoom toggle 全屏 放大",
            PaletteAction::KeyAction(KeyAction::TogglePaneFullscreen),
        ),
        PaletteItem::new(
            "Cycle Layout",
            "Switch the current tab between side-by-side and stacked splits",
            "layout cycle rotate next-layout 布局 切换 上下 左右",
            PaletteAction::KeyAction(KeyAction::CycleLayout),
        ),
        PaletteItem::new(
            "Next Pane",
            "Switch to the next pane in the current tab",
            "pane next cmd bracket 下一个",
            PaletteAction::KeyAction(KeyAction::NextPane),
        ),
        PaletteItem::new(
            "Previous Pane",
            "Switch to the previous pane in the current tab",
            "pane previous prev cmd bracket 上一个",
            PaletteAction::KeyAction(KeyAction::PrevPane),
        ),
        PaletteItem::new(
            "Previous Command",
            "Jump to the previous command mark",
            "previous command mark timeline history 上一条 命令",
            PaletteAction::KeyAction(KeyAction::PreviousCommand),
        ),
        PaletteItem::new(
            "Next Command",
            "Jump to the next command mark",
            "next command mark timeline history 下一条 命令",
            PaletteAction::KeyAction(KeyAction::NextCommand),
        ),
        PaletteItem::new(
            "Search Panes",
            "Search terminal history across panes",
            "search find pane terminal history 搜索 查找",
            PaletteAction::KeyAction(KeyAction::SearchWorkspace),
        ),
        PaletteItem::new(
            "Close Pane",
            "Close the current pane",
            "close pane 关闭",
            PaletteAction::KeyAction(KeyAction::ClosePane),
        ),
        PaletteItem::new(
            "Close Tab",
            "Close the current tab",
            "close tab 关闭 标签页",
            PaletteAction::KeyAction(KeyAction::CloseTab),
        ),
        PaletteItem::new(
            "Close Window",
            "Close the current window",
            "close window 关闭",
            PaletteAction::KeyAction(KeyAction::CloseWindow),
        ),
        PaletteItem::new(
            "Language",
            "Choose the interface language",
            "language locale 语言",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Increase Font Size",
            "Make terminal text larger",
            "font size increase zoom text 字体 放大",
            PaletteAction::KeyAction(KeyAction::IncreaseFontSize),
        ),
        PaletteItem::new(
            "Decrease Font Size",
            "Make terminal text smaller",
            "font size decrease zoom text 字体 缩小",
            PaletteAction::KeyAction(KeyAction::DecreaseFontSize),
        ),
        PaletteItem::new(
            "Reset Font Size",
            "Restore the configured terminal font size",
            "font size reset default 字体 重置",
            PaletteAction::KeyAction(KeyAction::ResetFontSize),
        ),
        PaletteItem::new(
            "Theme",
            "Switch between light and dark",
            "theme light dark 主题 浅色 深色",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Status Bar Mode",
            "Switch between tmux and theme status bar",
            "statusbar status bar mode tmux theme 状态栏 模式",
            PaletteAction::Unavailable,
        ),
        PaletteItem::new(
            "Tab Bar on Top",
            "Move the tab bar to the top",
            "tab bar position top 标签栏 顶部",
            PaletteAction::TabBarTop,
        ),
        PaletteItem::new(
            "Tab Bar on Bottom",
            "Move the tab bar to the bottom",
            "tab bar position bottom 标签栏 底部",
            PaletteAction::TabBarBottom,
        ),
        PaletteItem::new(
            "Quit Muxterm",
            "Quit the application",
            "quit exit 退出",
            PaletteAction::KeyAction(KeyAction::Quit),
        ),
        PaletteItem::new(
            "Muxterm Info",
            "Version, debug log, and GitHub issue",
            "about info version commit debug log github issue 关于 版本 日志",
            PaletteAction::Unavailable,
        ),
    ];

    // `Switch to Tab N` 插在第 8 条之后（与 macOS 的 insertion 索引一致）。
    let tab_items: Vec<PaletteItem> = tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            PaletteItem::new(
                &format!("Switch to Tab {}", index + 1),
                name,
                &format!("switch tab {} {} 切换 标签页", index + 1, name),
                PaletteAction::KeyAction(KeyAction::SwitchTab(index + 1)),
            )
        })
        .collect();
    if !tab_items.is_empty() {
        let insertion = usize::min(8, items.len());
        let many = tab_items.len() > 1;
        items.splice(insertion..insertion, tab_items);
        if many {
            items.insert(
                insertion + tabs.len(),
                PaletteItem::new(
                    "Switch to Last Tab",
                    "Focus the final open tab",
                    "switch last final tab 0 最后 标签页",
                    PaletteAction::KeyAction(KeyAction::SwitchLastTab),
                ),
            );
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_catalog_matches_macos_order() {
        // 对照 `MainWindow.rootPaletteItems()`：前 8 条固定命令的顺序。
        let items = root_items(&[]);
        let titles: Vec<&str> = items
            .iter()
            .take(8)
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(
            titles,
            [
                "Local",
                "SSH",
                "Quick Connect",
                "New Tab",
                "Rename Tab…",
                "Rename Workspace…",
                "Move Tab Left",
                "Move Tab Right",
            ]
        );
    }

    #[test]
    fn tab_items_are_inserted_after_eighth_command() {
        // 对照 macOS：`insertion = min(8, items.count)`。
        let items = root_items(&["zsh".into(), "vim".into()]);
        assert_eq!(items[8].title, "Switch to Tab 1");
        assert_eq!(items[8].detail, "zsh");
        assert_eq!(items[9].title, "Switch to Tab 2");
        // 多个 tab 时补上“最后一个 tab”。
        assert_eq!(items[10].title, "Switch to Last Tab");
    }

    #[test]
    fn single_tab_does_not_add_last_tab_item() {
        let items = root_items(&["zsh".into()]);
        assert_eq!(items[8].title, "Switch to Tab 1");
        assert!(!items.iter().any(|item| item.title == "Switch to Last Tab"));
    }

    #[test]
    fn filter_is_case_insensitive_substring_over_title_detail_keywords() {
        // 对照 `PaletteItem.searchText` + `applyFilter`。
        let items = root_items(&[]);
        let by_title = filter(&items, "NEW TAB");
        assert_eq!(by_title[0].title, "New Tab");
        let by_detail = filter(&items, "local tmux");
        assert_eq!(by_detail[0].title, "Local");
        let by_keyword = filter(&items, "标签页");
        assert!(by_keyword.len() > 1);
        let none = filter(&items, "没有这条命令");
        assert!(none.is_empty());
    }

    #[test]
    fn empty_query_keeps_every_item() {
        let items = root_items(&[]);
        assert_eq!(filter(&items, "  ").len(), items.len());
    }

    #[test]
    fn unavailable_commands_are_listed_but_marked() {
        // 语言、改名这些前端还没有的条目仍然出现，只是不可执行。
        let items = root_items(&[]);
        let language = items
            .iter()
            .find(|item| item.title == "Language")
            .expect("Language 必须在目录里");
        assert!(!language.available());
        let new_tab = items
            .iter()
            .find(|item| item.title == "New Tab")
            .expect("New Tab 必须在目录里");
        assert!(new_tab.available());
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut state = PaletteState::default();
        state.show();
        state.move_selection(1, 3);
        assert_eq!(state.selected, 1);
        state.move_selection(1, 3);
        state.move_selection(1, 3);
        assert_eq!(state.selected, 2);
        state.move_selection(-5, 3);
        assert_eq!(state.selected, 0);
        state.move_selection(1, 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn typing_resets_selection_and_backspace_keeps_it_valid() {
        let mut state = PaletteState::default();
        state.show();
        state.selected = 2;
        state.push_char('n', 5);
        assert_eq!(state.query, "n");
        assert_eq!(state.selected, 0);
        state.selected = 4;
        state.backspace(2);
        assert_eq!(state.query, "");
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn dismiss_clears_query() {
        let mut state = PaletteState::default();
        state.show();
        state.push_char('x', 10);
        state.dismiss();
        assert!(!state.open);
        assert!(state.query.is_empty());
    }
}
