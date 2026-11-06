//! Cmd-Shift-P 命令面板的投影（纯逻辑，无 gpui 依赖）。
//!
//! 对照 macOS `App/CommandPalette.swift` 与 `MainWindow.rootPaletteItems()`：
//! 一份命令目录 + 大小写无关的整串包含过滤 + 上下移动 + 回车执行。
//!
//! 目录里的每一条都必须能执行（没有灰掉的占位项）。文案走
//! `frontend::utils::i18n` catalog，所以「语言」切换会立刻反映到这份目录。

use crate::frontend::gpui::model::shortcuts::KeyAction;
use crate::frontend::utils::i18n::{self, Language, TextKey};

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

    /// 用 catalog key 构造：标题与说明都走 i18n。
    fn localized(title: TextKey, detail: TextKey, keywords: &str, action: PaletteAction) -> Self {
        Self::new(&i18n::tr(title), &i18n::tr(detail), keywords, action)
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
}

/// 一条命令对应的动作。窗口级动作复用 `KeyAction`，其余是面板独有的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    KeyAction(KeyAction),
    /// tab 栏挪到顶部 / 底部（写 Core `[ui] tab_bar_position`）。
    TabBarTop,
    TabBarBottom,
    /// Muxterm Info：版本 / commit / 调试日志 / GitHub issue。
    Info,
    /// Local：直接进本机 session 列表。
    OpenLocal,
    /// SSH：先选机器。
    OpenSsh,
    /// 重命名当前 tab / workspace：打开输入浮层。
    RenameTab,
    RenameWorkspace,
    /// 重排当前 tab。
    MoveTabLeft,
    MoveTabRight,
    /// 语言：换成语言子菜单。
    Languages,
    /// 语言子菜单里的一项。
    SetLanguage(Language),
    /// 主题在浅/深之间切换。
    ToggleTheme,
    /// 状态栏来源在 tmux / theme 之间切换。
    ToggleStatusBarMode,
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
        PaletteItem::localized(
            TextKey::Local,
            TextKey::LocalTmuxSessions,
            "local tmux attach new 本地",
            PaletteAction::OpenLocal,
        ),
        PaletteItem::localized(
            TextKey::Ssh,
            TextKey::SshHosts,
            "ssh remote host tmux attach new 主机",
            PaletteAction::OpenSsh,
        ),
        PaletteItem::localized(
            TextKey::QuickConnect,
            TextKey::QuickConnectDetail,
            "quick connect workspace attention search 快速连接 工作区 提醒 搜索",
            PaletteAction::KeyAction(KeyAction::QuickConnect),
        ),
        PaletteItem::localized(
            TextKey::NewTab,
            TextKey::NewTabDetail,
            "new tab 新建 标签页",
            PaletteAction::KeyAction(KeyAction::NewTab),
        ),
        PaletteItem::localized(
            TextKey::RenameTab,
            TextKey::RenameTabDetail,
            "rename tab title 重命名 标签页",
            PaletteAction::RenameTab,
        ),
        PaletteItem::localized(
            TextKey::RenameWorkspace,
            TextKey::RenameWorkspaceDetail,
            "rename workspace title 重命名 工作区",
            PaletteAction::RenameWorkspace,
        ),
        PaletteItem::localized(
            TextKey::MoveTabLeft,
            TextKey::MoveTabLeftDetail,
            "move tab left reorder 向左 移动 标签页",
            PaletteAction::MoveTabLeft,
        ),
        PaletteItem::localized(
            TextKey::MoveTabRight,
            TextKey::MoveTabRightDetail,
            "move tab right reorder 向右 移动 标签页",
            PaletteAction::MoveTabRight,
        ),
        PaletteItem::localized(
            TextKey::SplitPaneHorizontal,
            TextKey::SplitPaneHorizontalDetail,
            "split pane horizontal 水平 分割",
            PaletteAction::KeyAction(KeyAction::SplitHorizontal),
        ),
        PaletteItem::localized(
            TextKey::SplitPaneVertical,
            TextKey::SplitPaneVerticalDetail,
            "split pane vertical 竖直 上下",
            PaletteAction::KeyAction(KeyAction::SplitVertical),
        ),
        PaletteItem::localized(
            TextKey::WindowFullscreenEnter,
            TextKey::WindowFullscreenEnterDetail,
            "window fullscreen screen 窗口全屏 整个窗口",
            PaletteAction::KeyAction(KeyAction::ToggleWindowFullscreen),
        ),
        PaletteItem::localized(
            TextKey::FillDesktop,
            TextKey::FillDesktopDetail,
            "zoom maximize desktop window 最大化 当前桌面 占满",
            PaletteAction::KeyAction(KeyAction::ToggleWindowZoom),
        ),
        PaletteItem::localized(
            TextKey::CmdPreferences,
            TextKey::CmdPreferencesDetail,
            "settings preferences configuration 设置 偏好",
            PaletteAction::KeyAction(KeyAction::OpenSettings),
        ),
        PaletteItem::localized(
            TextKey::TogglePaneFullscreen,
            TextKey::TogglePaneFullscreenDetail,
            "pane fullscreen zoom toggle 全屏 放大",
            PaletteAction::KeyAction(KeyAction::TogglePaneFullscreen),
        ),
        PaletteItem::localized(
            TextKey::CycleLayout,
            TextKey::CycleLayoutDetail,
            "layout cycle rotate next-layout 布局 切换 上下 左右",
            PaletteAction::KeyAction(KeyAction::CycleLayout),
        ),
        PaletteItem::localized(
            TextKey::NextPane,
            TextKey::NextPaneDetail,
            "pane next cmd bracket 下一个",
            PaletteAction::KeyAction(KeyAction::NextPane),
        ),
        PaletteItem::localized(
            TextKey::PreviousPane,
            TextKey::PreviousPaneDetail,
            "pane previous prev cmd bracket 上一个",
            PaletteAction::KeyAction(KeyAction::PrevPane),
        ),
        PaletteItem::localized(
            TextKey::PreviousCommand,
            TextKey::PreviousCommandDetail,
            "previous command mark timeline history 上一条 命令",
            PaletteAction::KeyAction(KeyAction::PreviousCommand),
        ),
        PaletteItem::localized(
            TextKey::NextCommand,
            TextKey::NextCommandDetail,
            "next command mark timeline history 下一条 命令",
            PaletteAction::KeyAction(KeyAction::NextCommand),
        ),
        PaletteItem::localized(
            TextKey::SearchPanes,
            TextKey::SearchPanesDetail,
            "search find pane terminal history 搜索 查找",
            PaletteAction::KeyAction(KeyAction::SearchWorkspace),
        ),
        PaletteItem::localized(
            TextKey::ClosePane,
            TextKey::ClosePaneDetail,
            "close pane 关闭",
            PaletteAction::KeyAction(KeyAction::ClosePane),
        ),
        PaletteItem::localized(
            TextKey::CloseTab,
            TextKey::CloseTabDetail,
            "close tab 关闭 标签页",
            PaletteAction::KeyAction(KeyAction::CloseTab),
        ),
        PaletteItem::localized(
            TextKey::CloseWindow,
            TextKey::CloseWindowDetail,
            "close window 关闭",
            PaletteAction::KeyAction(KeyAction::CloseWindow),
        ),
        PaletteItem::localized(
            TextKey::Language,
            TextKey::LanguageDetail,
            "language locale 语言",
            PaletteAction::Languages,
        ),
        PaletteItem::localized(
            TextKey::MenuIncreaseFontSize,
            TextKey::MenuIncreaseFontSizeDetail,
            "font size increase zoom text 字体 放大",
            PaletteAction::KeyAction(KeyAction::IncreaseFontSize),
        ),
        PaletteItem::localized(
            TextKey::MenuDecreaseFontSize,
            TextKey::MenuDecreaseFontSizeDetail,
            "font size decrease zoom text 字体 缩小",
            PaletteAction::KeyAction(KeyAction::DecreaseFontSize),
        ),
        PaletteItem::localized(
            TextKey::MenuResetFontSize,
            TextKey::MenuResetFontSizeDetail,
            "font size reset default 字体 重置",
            PaletteAction::KeyAction(KeyAction::ResetFontSize),
        ),
        PaletteItem::localized(
            TextKey::ThemeSwitchTo,
            TextKey::ThemeDetail,
            "theme light dark 主题 浅色 深色",
            PaletteAction::ToggleTheme,
        ),
        PaletteItem::localized(
            TextKey::StatusBarMode,
            TextKey::StatusBarModeDetail,
            "statusbar status bar mode tmux theme 状态栏 模式",
            PaletteAction::ToggleStatusBarMode,
        ),
        PaletteItem::localized(
            TextKey::MenuTabBarTop,
            TextKey::TabBarTopDetail,
            "tab bar position top 标签栏 顶部",
            PaletteAction::TabBarTop,
        ),
        PaletteItem::localized(
            TextKey::MenuTabBarBottom,
            TextKey::TabBarBottomDetail,
            "tab bar position bottom 标签栏 底部",
            PaletteAction::TabBarBottom,
        ),
        PaletteItem::localized(
            TextKey::QuitMuxterm,
            TextKey::QuitMuxtermDetail,
            "quit exit 退出",
            PaletteAction::KeyAction(KeyAction::Quit),
        ),
        PaletteItem::localized(
            TextKey::MuxtermInfo,
            TextKey::MuxtermInfoDetail,
            "about info version commit debug log github issue 关于 版本 日志",
            PaletteAction::Info,
        ),
    ];

    // `Switch to Tab N` 插在第 8 条之后（与 macOS 的 insertion 索引一致）。
    let tab_items: Vec<PaletteItem> = tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            PaletteItem::new(
                &i18n::tr_args(TextKey::SwitchToTab, &[("index", &(index + 1).to_string())]),
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
                PaletteItem::localized(
                    TextKey::SwitchToLastTab,
                    TextKey::SwitchToLastTabDetail,
                    "switch last final tab 0 最后 标签页",
                    PaletteAction::KeyAction(KeyAction::SwitchLastTab),
                ),
            );
        }
    }
    items
}

/// 语言子菜单：Swift `showLanguageOptions()` 的等价列表。
///
/// 当前语言带 `· Current` 后缀；选择后立刻 `set_language` 并写回 Core 配置。
pub fn language_items(current: Language) -> Vec<PaletteItem> {
    let marker = i18n::tr(TextKey::LanguageCurrent);
    let languages = [
        (Language::System, TextKey::LanguageSystem),
        (Language::English, TextKey::LanguageEnglish),
        (
            Language::SimplifiedChinese,
            TextKey::LanguageSimplifiedChinese,
        ),
    ];
    languages
        .into_iter()
        .map(|(language, key)| {
            let title = i18n::tr(key);
            let detail = if language == current {
                format!("· {marker}")
            } else {
                String::new()
            };
            PaletteItem::new(
                &title,
                &detail,
                &format!("language locale 语言 {}", language.tag()),
                PaletteAction::SetLanguage(language),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录文案走 i18n，测试必须钉住一种语言；否则 LANG=zh 的机器上断言会漂。
    fn english() {
        i18n::set_language(Language::English);
    }

    #[test]
    fn root_catalog_matches_macos_order() {
        english();
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
        english();
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
        english();
        let items = root_items(&["zsh".into()]);
        assert_eq!(items[8].title, "Switch to Tab 1");
        assert!(!items.iter().any(|item| item.title == "Switch to Last Tab"));
    }

    #[test]
    fn filter_is_case_insensitive_substring_over_title_detail_keywords() {
        english();
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
        english();
        let items = root_items(&[]);
        assert_eq!(filter(&items, "  ").len(), items.len());
    }

    #[test]
    fn every_command_has_a_real_action() {
        english();
        let items = root_items(&[]);
        let actions: Vec<PaletteAction> = items.iter().map(|item| item.action).collect();
        assert_eq!(
            actions.len(),
            items.len(),
            "每条命令都必须映射到一个真实动作"
        );
        for expected in [
            PaletteAction::OpenLocal,
            PaletteAction::OpenSsh,
            PaletteAction::RenameTab,
            PaletteAction::RenameWorkspace,
            PaletteAction::MoveTabLeft,
            PaletteAction::MoveTabRight,
            PaletteAction::Languages,
            PaletteAction::ToggleTheme,
            PaletteAction::ToggleStatusBarMode,
        ] {
            assert!(
                actions.contains(&expected),
                "目录缺少 {expected:?}：{:?}",
                items
                    .iter()
                    .map(|item| item.title.clone())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// 语言子菜单：三种语言，当前语言带标记。
    #[test]
    fn language_menu_marks_the_current_language() {
        english();
        let items = language_items(Language::English);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "Follow System");
        assert_eq!(items[1].detail, "· Current");
        assert!(items[2].detail.is_empty());
        assert!(matches!(
            items[2].action,
            PaletteAction::SetLanguage(Language::SimplifiedChinese)
        ));
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
