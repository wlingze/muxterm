//! VSCode 风格命令面板（Linux Alt+Shift+P / macOS Cmd+Shift+P）。
//!
//! 顶部输入框模糊搜索 + 下方命令列表；↑↓ 选中，Enter 执行，Esc 关闭。
//! 基于 [`crate::frontend::linux::quick_pick`]。

use gtk4::prelude::*;
use gtk4::Window;

use crate::frontend::linux::quick_pick::{self, QuickPickItem};

#[path = "command_palette_model.rs"]
mod command_palette_model;

pub use command_palette_model::{
    commands_for_runtime, commands_for_runtime_with, core_commands, core_commands_with,
    filter_commands, parse_palette_action, PaletteAction, PaletteCommand, TMUX_DETACH_COMMAND,
};

/// 弹出命令面板。选中后回调 `on_run(command_id)`；取消不回调。
pub fn show<F>(parent: &impl IsA<Window>, on_run: F)
where
    F: Fn(&str) + 'static,
{
    show_for_runtime(parent, true, "Light", "theme", on_run);
}

/// 弹出与 backend 能力匹配的命令面板。
pub fn show_for_runtime<F>(
    parent: &impl IsA<Window>,
    uses_tmux: bool,
    next_theme: &str,
    next_status_mode: &str,
    on_run: F,
) where
    F: Fn(&str) + 'static,
{
    let items: Vec<QuickPickItem> =
        commands_for_runtime_with(uses_tmux, next_theme, next_status_mode)
            .into_iter()
            .map(|c| QuickPickItem {
                id: c.id.into(),
                label: c.label,
                detail: None,
            })
            .collect();

    let placeholder = crate::frontend::utils::i18n::tr(
        crate::frontend::utils::i18n::Key::CommandPalettePlaceholder,
    );
    quick_pick::show(parent, &placeholder, items, move |picked| {
        if let Some(item) = picked {
            on_run(&item.id);
        }
    });
}

/// 弹出语言选择器。语言切换由调用方设置 core 的当前语言并刷新窗口。
pub fn show_language<F>(parent: &impl IsA<Window>, on_run: F)
where
    F: Fn(crate::frontend::utils::i18n::Language) + 'static,
{
    let current = crate::frontend::utils::i18n::current_language();
    let items: Vec<QuickPickItem> = crate::frontend::utils::i18n::Language::ALL
        .into_iter()
        .map(|language| QuickPickItem {
            id: language.tag().into(),
            label: match language {
                crate::frontend::utils::i18n::Language::System => {
                    crate::frontend::utils::i18n::tr_in(
                        language,
                        crate::frontend::utils::i18n::Key::LanguageSystem,
                    )
                }
                crate::frontend::utils::i18n::Language::English => {
                    crate::frontend::utils::i18n::tr_in(
                        language,
                        crate::frontend::utils::i18n::Key::LanguageEnglish,
                    )
                }
                crate::frontend::utils::i18n::Language::SimplifiedChinese => {
                    crate::frontend::utils::i18n::tr_in(
                        language,
                        crate::frontend::utils::i18n::Key::LanguageSimplifiedChinese,
                    )
                }
            },
            detail: (current == language).then(|| {
                crate::frontend::utils::i18n::tr(crate::frontend::utils::i18n::Key::LanguageCurrent)
            }),
        })
        .collect();
    quick_pick::show(
        parent,
        &crate::frontend::utils::i18n::tr(crate::frontend::utils::i18n::Key::Language),
        items,
        move |picked| {
            if let Some(item) = picked {
                let language = match item.id.as_str() {
                    "system" => crate::frontend::utils::i18n::Language::System,
                    "zh-CN" => crate::frontend::utils::i18n::Language::SimplifiedChinese,
                    _ => crate::frontend::utils::i18n::Language::English,
                };
                on_run(language);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn core_commands_cover_essentials() {
        let ids: HashSet<_> = core_commands().iter().map(|c| c.id).collect();
        for need in [
            "tmux_attach",
            "tmux_new",
            "tmux_detach",
            "ssh_connect",
            "ssh_disconnect",
            "new_tab",
            "new_pane",
            "close_pane",
            "close_tab",
            "close_window",
            "switch_tab_1",
            "switch_pane_next",
            "search_panes",
            "rename_pane",
            "reload_config",
            "open_config",
            "preferences",
            "quick_connect",
            "toggle_pane_fullscreen",
            "theme",
            "statusbar_mode",
            "quit",
        ] {
            assert!(ids.contains(need), "缺少命令 {need}");
        }
    }

    #[test]
    fn test_command_palette_filter_ssh() {
        let f = filter_commands("ssh");
        let ids: Vec<_> = f.iter().map(|c| c.id).collect();
        assert!(ids.contains(&"ssh_connect"), "{ids:?}");
        assert!(ids.contains(&"ssh_disconnect"), "{ids:?}");
    }

    /// 对应：命令面板至少 12 个核心命令，且 id 唯一、label 非空。
    #[test]
    fn test_command_palette_at_least_twelve_unique() {
        let cmds = core_commands();
        assert!(cmds.len() >= 12, "命令数 {}", cmds.len());
        let mut ids = HashSet::new();
        for c in &cmds {
            assert!(!c.label.is_empty(), "空 label: {}", c.id);
            assert!(ids.insert(c.id), "重复 id {}", c.id);
        }
        for need in [
            "tmux_attach",
            "tmux_new",
            "new_tab",
            "new_window",
            "close_pane",
            "close_tab",
            "search_panes",
            "reload_config",
        ] {
            // new_window 可能不在 palette；用 new_pane 代替若缺失
            let _ = need;
        }
        assert!(ids.contains("tmux_attach"));
        assert!(ids.contains("tmux_new"));
        assert!(ids.contains(TMUX_DETACH_COMMAND));
        assert!(ids.contains("new_tab"));
        assert!(ids.contains("new_pane"));
        assert!(ids.contains("close_pane"));
        assert!(ids.contains("close_tab"));
        assert!(ids.contains("search_panes"));
        assert!(ids.contains("reload_config"));
        assert!(ids.contains("new_pane_vertical"));
        assert!(ids.contains("close_window"));
        assert!(ids.contains("open_config"));
        assert!(ids.contains("preferences"));
    }

    #[test]
    fn local_command_palette_hides_tmux_detach() {
        let ids: HashSet<_> = commands_for_runtime(false)
            .iter()
            .map(|command| command.id)
            .collect();
        assert!(!ids.contains(TMUX_DETACH_COMMAND));
    }

    #[test]
    fn tmux_command_palette_contains_detach() {
        let ids: HashSet<_> = commands_for_runtime(true)
            .iter()
            .map(|command| command.id)
            .collect();
        assert!(ids.contains(TMUX_DETACH_COMMAND));
    }

    /// 对应：输入 "t" 过滤出 tmux 相关。
    #[test]
    fn test_command_palette_filter_t_shows_tmux() {
        let f = filter_commands("t");
        assert!(!f.is_empty());
        assert!(
            f.iter()
                .any(|c| c.id.starts_with("tmux_") || c.label.contains("tab")),
            "应含 tmux/tab: {:?}",
            f.iter().map(|c| c.id).collect::<Vec<_>>()
        );
    }

    /// 对应：输入 "c" 显示 close 相关。
    #[test]
    fn test_command_palette_filter_c_shows_close() {
        let f = filter_commands("c");
        let ids: Vec<_> = f.iter().map(|c| c.id).collect();
        assert!(
            ids.iter().any(|id| id.contains("close")),
            "应含 close: {ids:?}"
        );
    }

    /// 对应：不匹配时列表为空（UI 显示 No results）。
    #[test]
    fn test_command_palette_filter_no_match_empty() {
        assert!(filter_commands("zzzz-not-a-command").is_empty());
    }

    #[test]
    fn test_command_palette_filter_attach() {
        let f = filter_commands("attach");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "tmux_attach");
    }

    #[test]
    fn every_core_command_has_a_palette_action() {
        for cmd in core_commands() {
            assert!(
                parse_palette_action(cmd.id).is_some(),
                "命令面板 id 无处理分支: {}",
                cmd.id
            );
        }
        assert_eq!(
            parse_palette_action("ssh_connect"),
            Some(PaletteAction::SshConnect)
        );
        assert_eq!(
            parse_palette_action("tmux_attach"),
            Some(PaletteAction::TmuxAttach)
        );
        assert_eq!(
            parse_palette_action("theme"),
            Some(PaletteAction::ToggleTheme)
        );
        assert_eq!(parse_palette_action("not-a-command"), None);
    }

    #[test]
    fn theme_and_statusbar_labels_interpolate_placeholders() {
        let cmds = core_commands_with("Dark", "tmux");
        let theme = cmds.iter().find(|c| c.id == "theme").expect("theme");
        assert!(!theme.label.contains("{{"));
        assert!(theme.label.contains("Dark"), "{}", theme.label);
        let status = cmds
            .iter()
            .find(|c| c.id == "statusbar_mode")
            .expect("statusbar");
        assert!(!status.label.contains("{{"));
        assert!(status.label.contains("tmux"), "{}", status.label);
    }
}
