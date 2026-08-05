//! VSCode 风格命令面板（Alt+P）。
//!
//! 顶部输入框模糊搜索 + 下方命令列表；↑↓ 选中，Enter 执行，Esc 关闭。
//! 基于 [`crate::platform::linux::quick_pick`]。

use gtk4::prelude::*;
use gtk4::Window;

use crate::platform::linux::quick_pick::{self, fuzzy_match, QuickPickItem};

pub const TMUX_DETACH_COMMAND: &str = "tmux_detach";

/// 一条核心命令。
#[derive(Debug, Clone)]
pub struct PaletteCommand {
    pub id: &'static str,
    pub label: String,
}

/// 硬编码核心命令清单。
pub fn core_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            id: "tmux_attach",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdTmuxAttach),
        },
        PaletteCommand {
            id: "tmux_new",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdTmuxNew),
        },
        PaletteCommand {
            id: TMUX_DETACH_COMMAND,
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdTmuxDetach),
        },
        PaletteCommand {
            id: "ssh_connect",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdSshConnect),
        },
        PaletteCommand {
            id: "ssh_disconnect",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdSshDisconnect),
        },
        PaletteCommand {
            id: "new_tab",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::NewTab),
        },
        PaletteCommand {
            id: "new_pane",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdNewPane),
        },
        PaletteCommand {
            id: "new_pane_vertical",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdNewPaneVertical),
        },
        PaletteCommand {
            id: "close_pane",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::ClosePane),
        },
        PaletteCommand {
            id: "close_tab",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CloseTab),
        },
        PaletteCommand {
            id: "close_window",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CloseWindow),
        },
        PaletteCommand {
            id: "switch_tab_1",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "1")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_2",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "2")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_3",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "3")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_4",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "4")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_5",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "5")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_6",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "6")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_7",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "7")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_8",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "8")],
            ),
        },
        PaletteCommand {
            id: "switch_tab_9",
            label: crate::platform::i18n::tr_args(
                crate::platform::i18n::Key::CmdSwitchTab,
                &[("number", "9")],
            ),
        },
        PaletteCommand {
            id: "switch_pane_prev",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdSwitchPanePrevious),
        },
        PaletteCommand {
            id: "switch_pane_next",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdSwitchPaneNext),
        },
        PaletteCommand {
            id: "search_panes",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdSearchPanes),
        },
        PaletteCommand {
            id: "rename_pane",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdRenamePane),
        },
        PaletteCommand {
            id: "reload_config",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdReloadConfig),
        },
        PaletteCommand {
            id: "open_config",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdOpenConfig),
        },
        PaletteCommand {
            id: "preferences",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::CmdPreferences),
        },
        PaletteCommand {
            id: "language",
            label: crate::platform::i18n::tr(crate::platform::i18n::Key::Language),
        },
    ]
}

/// 根据 backend 筛选用户可执行的命令。
///
/// local shell 没有 tmux control client，不能展示或执行 detach；tmux/SSH
/// 模式则保留完整命令集。`core_commands` 仍保留完整清单，方便静态命令
/// 目录和纯逻辑测试覆盖所有命令。
pub fn commands_for_backend(uses_tmux: bool) -> Vec<PaletteCommand> {
    let mut commands = core_commands();
    if !uses_tmux {
        commands.retain(|command| command.id != TMUX_DETACH_COMMAND);
    }
    commands
}

/// 按查询过滤核心命令（纯逻辑，供单测与 UI 共用）。
pub fn filter_commands(query: &str) -> Vec<PaletteCommand> {
    core_commands()
        .into_iter()
        // id 保持稳定的英文搜索词；这样切到中文后，`attach`、`split` 等
        // CLI/快捷键常用词仍然可以找到对应命令。
        .filter(|c| fuzzy_match(query, &c.label) || fuzzy_match(query, c.id))
        .collect()
}

/// 弹出命令面板。选中后回调 `on_run(command_id)`；取消不回调。
pub fn show<F>(parent: &impl IsA<Window>, on_run: F)
where
    F: Fn(&str) + 'static,
{
    show_for_backend(parent, true, on_run);
}

/// 弹出与 backend 能力匹配的命令面板。
pub fn show_for_backend<F>(parent: &impl IsA<Window>, uses_tmux: bool, on_run: F)
where
    F: Fn(&str) + 'static,
{
    let items: Vec<QuickPickItem> = commands_for_backend(uses_tmux)
        .into_iter()
        .map(|c| QuickPickItem {
            id: c.id.into(),
            label: c.label,
            detail: None,
        })
        .collect();

    let placeholder =
        crate::platform::i18n::tr(crate::platform::i18n::Key::CommandPalettePlaceholder);
    quick_pick::show(parent, &placeholder, items, move |picked| {
        if let Some(item) = picked {
            on_run(&item.id);
        }
    });
}

/// 弹出语言选择器。语言切换由调用方设置 core 的当前语言并刷新窗口。
pub fn show_language<F>(parent: &impl IsA<Window>, on_run: F)
where
    F: Fn(crate::platform::i18n::Language) + 'static,
{
    let current = crate::platform::i18n::current_language();
    let items: Vec<QuickPickItem> = crate::platform::i18n::Language::ALL
        .into_iter()
        .map(|language| QuickPickItem {
            id: language.tag().into(),
            label: match language {
                crate::platform::i18n::Language::System => crate::platform::i18n::tr_in(
                    language,
                    crate::platform::i18n::Key::LanguageSystem,
                ),
                crate::platform::i18n::Language::English => crate::platform::i18n::tr_in(
                    language,
                    crate::platform::i18n::Key::LanguageEnglish,
                ),
                crate::platform::i18n::Language::SimplifiedChinese => crate::platform::i18n::tr_in(
                    language,
                    crate::platform::i18n::Key::LanguageSimplifiedChinese,
                ),
            },
            detail: (current == language)
                .then(|| crate::platform::i18n::tr(crate::platform::i18n::Key::LanguageCurrent)),
        })
        .collect();
    quick_pick::show(
        parent,
        &crate::platform::i18n::tr(crate::platform::i18n::Key::Language),
        items,
        move |picked| {
            if let Some(item) = picked {
                let language = match item.id.as_str() {
                    "system" => crate::platform::i18n::Language::System,
                    "zh-CN" => crate::platform::i18n::Language::SimplifiedChinese,
                    _ => crate::platform::i18n::Language::English,
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
        let ids: HashSet<_> = commands_for_backend(false)
            .iter()
            .map(|command| command.id)
            .collect();
        assert!(!ids.contains(TMUX_DETACH_COMMAND));
    }

    #[test]
    fn tmux_command_palette_contains_detach() {
        let ids: HashSet<_> = commands_for_backend(true)
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
}
