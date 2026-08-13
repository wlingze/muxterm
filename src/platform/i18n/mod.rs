//! 跨平台界面文案 catalog。
//!
//! key/value catalog 是平台层的公共资源；core、CLI、GTK 和其他平台前端都
//! 通过 key 取文案，不把某个平台的控件类型带进核心模型。

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// JSON catalog 中允许使用的文案 key。
///
/// JSON 仍然是文案源；所有 Rust 平台通过这个 typed key 访问，避免在业务
/// 代码里重复手写字符串。新增/删除 JSON key 时，catalog parity 测试会立即
/// 报错，防止 enum 与资源悄悄漂移。
macro_rules! define_text_keys {
    ($( $variant:ident => $key:literal ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum TextKey {
            $( $variant ),+
        }

        impl TextKey {
            pub const ALL: &'static [Self] = &[
                $( Self::$variant ),+
            ];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $key ),+
                }
            }
        }
    };
}

define_text_keys! {
    Cancel => "cancel",
    ChooseDirectoryMessage => "choose_directory_message",
    ChooseRemoteDirectory => "choose_remote_directory",
    ChooseSshHost => "choose_ssh_host",
    ChooseTmuxDirectory => "choose_tmux_directory",
    ChooseTmuxSession => "choose_tmux_session",
    ClosePane => "close_pane",
    ClosePaneDetail => "close_pane_detail",
    CloseTab => "close_tab",
    CloseTabDetail => "close_tab_detail",
    CloseWindow => "close_window",
    CloseWindowDetail => "close_window_detail",
    CmdNewPane => "cmd_new_pane",
    CmdNewPaneVertical => "cmd_new_pane_vertical",
    CmdOpenConfig => "cmd_open_config",
    CmdPreferences => "cmd_preferences",
    CmdReloadConfig => "cmd_reload_config",
    CmdRenamePane => "cmd_rename_pane",
    CmdSearchPanes => "cmd_search_panes",
    CmdSshConnect => "cmd_ssh_connect",
    CmdSshDisconnect => "cmd_ssh_disconnect",
    CmdSwitchPaneNext => "cmd_switch_pane_next",
    CmdSwitchPanePrevious => "cmd_switch_pane_previous",
    CmdSwitchTab => "cmd_switch_tab",
    CmdTmuxAttach => "cmd_tmux_attach",
    CmdTmuxDetach => "cmd_tmux_detach",
    CmdTmuxNew => "cmd_tmux_new",
    CommandPalette => "command_palette",
    CommandPalettePlaceholder => "command_palette_placeholder",
    CreateAndAttach => "create_and_attach",
    Detach => "detach",
    DetachDetail => "detach_detail",
    ErrorBridgeConnect => "error_bridge_connect",
    ErrorBridgeCreate => "error_bridge_create",
    ErrorClosePane => "error_close_pane",
    ErrorCloseTab => "error_close_tab",
    ErrorCommandFailed => "error_command_failed",
    ErrorCoreDiscoveryInvalidJson => "error_core_discovery_invalid_json",
    ErrorCoreDiscoveryInvalidUtf8 => "error_core_discovery_invalid_utf8",
    ErrorCoreDiscoveryNoResponse => "error_core_discovery_no_response",
    ErrorCorePoll => "error_core_poll",
    ErrorCoreUnavailable => "error_core_unavailable",
    ErrorCreateFailed => "error_create_failed",
    ErrorMainWindowUnavailable => "error_main_window_unavailable",
    ErrorNewTab => "error_new_tab",
    ErrorNoSshHosts => "error_no_ssh_hosts",
    ErrorPaletteFailed => "error_palette_failed",
    ErrorResizeClient => "error_resize_client",
    ErrorResizeDivider => "error_resize_divider",
    ErrorResizePane => "error_resize_pane",
    ErrorSendControl => "error_send_control",
    ErrorSendInput => "error_send_input",
    ErrorSplitPane => "error_split_pane",
    ErrorSshConfig => "error_ssh_config",
    ErrorSshHostDiscovery => "error_ssh_host_discovery",
    ErrorSwitchPane => "error_switch_pane",
    ErrorSwitchTab => "error_switch_tab",
    ErrorTmuxSessionCreation => "error_tmux_session_creation",
    ErrorTmuxSessionDiscovery => "error_tmux_session_discovery",
    FreeformUseTypedTarget => "freeform_use_typed_target",
    HintPalette => "hint_palette",
    HintPane => "hint_pane",
    HintNewTab => "hint_new_tab",
    HintQuit => "hint_quit",
    HintSplit => "hint_split",
    HintVerticalSplit => "hint_vertical_split",
    Language => "language",
    LanguageCurrent => "language_current",
    LanguageDetail => "language_detail",
    LanguageEnglish => "language_english",
    LanguageSimplifiedChinese => "language_simplified_chinese",
    LanguageSystem => "language_system",
    LayoutSyncing => "layout_syncing",
    Local => "local",
    LocalTmuxSessions => "local_tmux_sessions",
    MenuAbout => "menu_about",
    MenuClosePane => "menu_close_pane",
    MenuCloseWindow => "menu_close_window",
    MenuCommandPalette => "menu_command_palette",
    MenuCopy => "menu_copy",
    MenuDecreaseFontSize => "menu_decrease_font_size",
    MenuEdit => "menu_edit",
    MenuFile => "menu_file",
    MenuIncreaseFontSize => "menu_increase_font_size",
    MenuNewTab => "menu_new_tab",
    MenuNextPane => "menu_next_pane",
    MenuPaste => "menu_paste",
    MenuPreviousPane => "menu_previous_pane",
    MenuQuit => "menu_quit",
    MenuResetFontSize => "menu_reset_font_size",
    MenuSelectAll => "menu_select_all",
    MenuSplitHorizontal => "menu_split_horizontal",
    MenuSplitVertical => "menu_split_vertical",
    MenuSwitchTab => "menu_switch_tab",
    MenuTabBarBottom => "menu_tab_bar_bottom",
    MenuTabBarTop => "menu_tab_bar_top",
    MenuView => "menu_view",
    MenuWindow => "menu_window",
    NewSession => "new_session",
    NewSessionDetail => "new_session_detail",
    NewTab => "new_tab",
    NewTabDetail => "new_tab_detail",
    NewTabTooltip => "new_tab_tooltip",
    NextPane => "next_pane",
    NextPaneDetail => "next_pane_detail",
    Pane => "pane",
    PaneAccessibility => "pane_accessibility",
    PaneRenameAction => "pane_rename_action",
    PaneRenameCancel => "pane_rename_cancel",
    PaneRenameHint => "pane_rename_hint",
    PaneRenameTitle => "pane_rename_title",
    PaneSearchPlaceholder => "pane_search_placeholder",
    Panes => "panes",
    PreviousPane => "previous_pane",
    PreviousPaneDetail => "previous_pane_detail",
    QuitMuxterm => "quit_muxterm",
    QuitMuxtermDetail => "quit_muxterm_detail",
    RemoteDirectoryMessage => "remote_directory_message",
    SplitPaneHorizontal => "split_pane_horizontal",
    SplitPaneHorizontalDetail => "split_pane_horizontal_detail",
    SplitPaneVertical => "split_pane_vertical",
    SplitPaneVerticalDetail => "split_pane_vertical_detail",
    Ssh => "ssh",
    SshHosts => "ssh_hosts",
    StatusConnected => "status_connected",
    StatusConnecting => "status_connecting",
    StatusDisconnected => "status_disconnected",
    StatusError => "status_error",
    StatusExited => "status_exited",
    StatusUnknown => "status_unknown",
    StatusBarModeSwitchTo => "statusbar_mode_switch_to",
    StatusBarModeDetail => "statusbar_mode_detail",
    Tabs => "tabs",
    TabsAccessibility => "tabs_accessibility",
    TerminalOutputSnippet => "terminal_output_snippet",
    TerminalPane => "terminal_pane",
    ThemeSwitchTo => "theme_switch_to",
    ThemeDetail => "theme_detail",
    TmuxAttachPlaceholder => "tmux_attach_placeholder",
    TmuxAttached => "tmux_attached",
    TmuxCreateDetail => "tmux_create_detail",
    TmuxCreateNew => "tmux_create_new",
    TmuxDaysAgo => "tmux_days_ago",
    TmuxHoursAgo => "tmux_hours_ago",
    TmuxMinutesAgo => "tmux_minutes_ago",
    TmuxSecondsAgo => "tmux_seconds_ago",
    TmuxSessionDetail => "tmux_session_detail",
    TmuxUnknown => "tmux_unknown",
    TmuxWindows => "tmux_windows",
    WindowCloseHint => "window_close_hint",
}

/// 简写别名：平台模块可以使用 `Key::CommandPalette`，不暴露 JSON 字符串。
#[allow(unused_imports)]
pub use TextKey as Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    System,
    English,
    SimplifiedChinese,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::System, Self::English, Self::SimplifiedChinese];

    pub fn from_tag(tag: &str) -> Self {
        if tag
            .split(['-', '_', '.', '@'])
            .next()
            .is_some_and(|part| part.eq_ignore_ascii_case("zh"))
        {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }

    pub const fn tag(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
        }
    }

    fn resolved(self) -> Self {
        match self {
            Self::System => detect_language(),
            language => language,
        }
    }
}

static CURRENT_LANGUAGE: OnceLock<RwLock<Language>> = OnceLock::new();

fn language_cell() -> &'static RwLock<Language> {
    CURRENT_LANGUAGE.get_or_init(|| RwLock::new(Language::System))
}

/// 当前进程的界面语言。默认是 `System`，每次取文案时按
/// `MUXTERM_LANG`、`LC_ALL`、`LC_MESSAGES`、`LANG` 顺序检测，无法识别时使用
/// English；这样系统语言变化后无需重启进程即可重新解析。
pub fn current_language() -> Language {
    match language_cell().read() {
        Ok(language) => *language,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// 当前语言选择解析后的界面语言；`System` 会重新读取环境语言。
pub fn resolved_language() -> Language {
    current_language().resolved()
}

/// 设置当前进程的界面语言。
pub fn set_language(language: Language) {
    match language_cell().write() {
        Ok(mut current) => *current = language,
        Err(poisoned) => *poisoned.into_inner() = language,
    }
}

/// 返回当前语言的文案；缺失 key 时返回 key 本身，避免 UI 因 catalog 问题 panic。
pub fn tr(key: TextKey) -> String {
    tr_in(current_language(), key)
}

/// 使用指定语言取文案，适合测试和生成平台选项。
pub fn tr_in(language: Language, key: TextKey) -> String {
    catalog(language)
        .get(key.as_str())
        .or_else(|| catalog(Language::English).get(key.as_str()))
        .cloned()
        .unwrap_or_else(|| key.as_str().to_string())
}

/// 取文案并替换 `{{name}}` 占位符。
pub fn tr_args(key: TextKey, args: &[(&str, &str)]) -> String {
    let mut text = tr(key);
    for (name, value) in args {
        text = text.replace(&format!("{{{{{name}}}}}"), value);
    }
    text
}

fn detect_language() -> Language {
    ["MUXTERM_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| Language::from_tag(&value))
        .next()
        .unwrap_or(Language::English)
}

fn catalog(language: Language) -> &'static HashMap<String, String> {
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    static ZH_CN: OnceLock<HashMap<String, String>> = OnceLock::new();
    match language.resolved() {
        // `detect_language` 当前只返回具体 catalog；保留无害 fallback，避免
        // 外部新增语言来源时 catalog 读取路径变成 panic。
        Language::System => EN.get_or_init(|| load(include_str!("locales/en.json"))),
        Language::English => EN.get_or_init(|| load(include_str!("locales/en.json"))),
        Language::SimplifiedChinese => {
            ZH_CN.get_or_init(|| load(include_str!("locales/zh-CN.json")))
        }
    }
}

fn load(raw: &str) -> HashMap<String, String> {
    // catalog 是嵌入资源，解析失败时使用空 catalog；tr_in 会回退到英文，
    // 英文也失败时返回 typed key 名称，保证 UI 报错而不是 panic。
    serde_json::from_str(raw).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    static LANGUAGE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn catalogs_have_the_same_keys() {
        let expected: HashSet<_> = TextKey::ALL.iter().map(|key| key.as_str()).collect();
        let en = catalog(Language::English);
        let zh = catalog(Language::SimplifiedChinese);
        let en_keys: HashSet<_> = en.keys().map(String::as_str).collect();
        let zh_keys: HashSet<_> = zh.keys().map(String::as_str).collect();
        assert_eq!(expected, en_keys);
        assert_eq!(expected, zh_keys);

        let mac_en: HashMap<String, String> =
            serde_json::from_str(include_str!("../macos/Resources/i18n/en.json")).unwrap();
        let mac_zh: HashMap<String, String> =
            serde_json::from_str(include_str!("../macos/Resources/i18n/zh-CN.json")).unwrap();
        let mac_en_keys: HashSet<_> = mac_en.keys().map(String::as_str).collect();
        let mac_zh_keys: HashSet<_> = mac_zh.keys().map(String::as_str).collect();
        assert_eq!(expected, mac_en_keys);
        assert_eq!(expected, mac_zh_keys);
    }

    #[test]
    fn language_tags_and_arguments_are_stable() {
        let _guard = LANGUAGE_TEST_LOCK.lock().unwrap();
        assert_eq!(
            Language::from_tag("zh_CN.UTF-8"),
            Language::SimplifiedChinese
        );
        assert_eq!(Language::from_tag("en_US.UTF-8"), Language::English);
        assert_eq!(
            tr_in(Language::English, Key::TmuxWindows),
            "{{count}} windows"
        );
        set_language(Language::English);
        assert_eq!(tr_args(Key::TmuxWindows, &[("count", "3")]), "3 windows");
        assert_eq!(Language::System.tag(), "system");
        assert!(!tr(Key::CommandPalette).is_empty());
    }

    #[test]
    fn switching_language_changes_existing_key_lookup_immediately() {
        let _guard = LANGUAGE_TEST_LOCK.lock().unwrap();
        set_language(Language::English);
        assert_eq!(tr(Key::CommandPalette), "Command Palette");

        set_language(Language::SimplifiedChinese);
        assert_eq!(tr(Key::CommandPalette), "命令面板");

        // 测试不能把全局语言状态留在中文，避免影响同进程后续测试或调用方。
        set_language(Language::English);
    }

    #[test]
    fn malformed_catalog_falls_back_without_panic() {
        assert!(load("{not-json").is_empty());
    }
}
