//! VSCode 风格命令面板（Alt+P）。
//!
//! 顶部输入框模糊搜索 + 下方命令列表；↑↓ 选中，Enter 执行，Esc 关闭。
//! 基于 [`crate::platform::linux::quick_pick`]。

use gtk4::prelude::*;
use gtk4::Window;

use crate::platform::linux::quick_pick::{self, QuickPickItem};

/// 一条核心命令。
#[derive(Debug, Clone, Copy)]
pub struct PaletteCommand {
    pub id: &'static str,
    pub label: &'static str,
}

/// 硬编码核心命令清单。
pub fn core_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            id: "tmux_attach",
            label: "tmux: attach to session",
        },
        PaletteCommand {
            id: "tmux_new",
            label: "tmux: create new session",
        },
        PaletteCommand {
            id: "tmux_detach",
            label: "tmux: detach current",
        },
        PaletteCommand {
            id: "new_tab",
            label: "new tab",
        },
        PaletteCommand {
            id: "new_pane",
            label: "new pane (horizontal)",
        },
        PaletteCommand {
            id: "new_pane_vertical",
            label: "new pane (vertical)",
        },
        PaletteCommand {
            id: "close_pane",
            label: "close pane",
        },
        PaletteCommand {
            id: "close_tab",
            label: "close tab",
        },
        PaletteCommand {
            id: "close_window",
            label: "close window",
        },
        PaletteCommand {
            id: "switch_tab_1",
            label: "switch to tab 1",
        },
        PaletteCommand {
            id: "switch_tab_2",
            label: "switch to tab 2",
        },
        PaletteCommand {
            id: "switch_tab_3",
            label: "switch to tab 3",
        },
        PaletteCommand {
            id: "switch_tab_4",
            label: "switch to tab 4",
        },
        PaletteCommand {
            id: "switch_tab_5",
            label: "switch to tab 5",
        },
        PaletteCommand {
            id: "switch_tab_6",
            label: "switch to tab 6",
        },
        PaletteCommand {
            id: "switch_tab_7",
            label: "switch to tab 7",
        },
        PaletteCommand {
            id: "switch_tab_8",
            label: "switch to tab 8",
        },
        PaletteCommand {
            id: "switch_tab_9",
            label: "switch to tab 9",
        },
        PaletteCommand {
            id: "switch_pane_prev",
            label: "switch pane prev",
        },
        PaletteCommand {
            id: "switch_pane_next",
            label: "switch pane next",
        },
        PaletteCommand {
            id: "search_panes",
            label: "search panes",
        },
        PaletteCommand {
            id: "rename_pane",
            label: "rename pane",
        },
        PaletteCommand {
            id: "reload_config",
            label: "reload config",
        },
        PaletteCommand {
            id: "open_config",
            label: "open config file",
        },
        PaletteCommand {
            id: "preferences",
            label: "preferences",
        },
    ]
}

/// 弹出命令面板。选中后回调 `on_run(command_id)`；取消不回调。
pub fn show<F>(parent: &impl IsA<Window>, on_run: F)
where
    F: Fn(&str) + 'static,
{
    let items: Vec<QuickPickItem> = core_commands()
        .into_iter()
        .map(|c| QuickPickItem {
            id: c.id.into(),
            label: c.label.into(),
            detail: None,
        })
        .collect();

    quick_pick::show(parent, "Type a command…", items, move |picked| {
        if let Some(item) = picked {
            on_run(&item.id);
        }
    });
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
}
