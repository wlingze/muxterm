//! 命令面板（opencode 风格悬浮框）。
//!
//! 顶部输入框 + 下方模糊匹配命令列表。命令来源覆盖：
//! - 页面操作（split/new tab/close/switch）
//! - tmux 会话操作（attach / new）
//! - SSH 操作
//! - settings / CLI 全部命令
//!
//! 纯逻辑模块：命令定义 + 模糊过滤 + 选中状态。渲染在 `render.rs` 完成。

use ratatui::widgets::ListState;

/// 面板命令（id + 分组 + 展示 label + 匹配关键词）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteCommand {
    /// 稳定 id，用于分发到具体动作。
    pub id: &'static str,
    /// 分组名（attach / new / ssh / settings / session / window / tab / pane / cli）。
    pub group: &'static str,
    /// 展示文本（用于模糊匹配 + 显示）。
    pub label: &'static str,
    /// 额外关键词（别名，便于搜索）。
    pub keywords: &'static str,
}

impl PaletteCommand {
    fn cmd(
        id: &'static str,
        group: &'static str,
        label: &'static str,
        keywords: &'static str,
    ) -> Self {
        Self {
            id,
            group,
            label,
            keywords,
        }
    }

    /// 是否匹配查询（大小写不敏感，label 或 keywords 子串 / 子序列）。
    pub fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        let hay = format!("{} {}", self.label, self.keywords).to_lowercase();
        fuzzy_match(&q, &hay)
    }
}

/// 模糊匹配：子串或字符子序列（顺序保持）。
pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t.contains(&q) {
        return true;
    }
    let mut ti = t.chars();
    for qc in q.chars() {
        loop {
            match ti.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// 全部命令清单。
pub fn all_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand::cmd("new_tab", "window", "New tab (window)", "neww window"),
        PaletteCommand::cmd(
            "new_pane_h",
            "pane",
            "Split pane horizontal",
            "split h left right",
        ),
        PaletteCommand::cmd(
            "new_pane_v",
            "pane",
            "Split pane vertical",
            "split v top bottom",
        ),
        PaletteCommand::cmd("close_pane", "pane", "Close active pane", "kill killp"),
        PaletteCommand::cmd(
            "close_tab",
            "window",
            "Close active tab",
            "killw kill window",
        ),
        PaletteCommand::cmd("switch_pane_next", "pane", "Switch to next pane", "next"),
        PaletteCommand::cmd(
            "switch_pane_prev",
            "pane",
            "Switch to previous pane",
            "prev",
        ),
        PaletteCommand::cmd(
            "switch_tab_next",
            "window",
            "Switch to next tab",
            "window next",
        ),
        PaletteCommand::cmd(
            "switch_tab_prev",
            "window",
            "Switch to previous tab",
            "window prev",
        ),
        PaletteCommand::cmd(
            "session_attach",
            "session",
            "Attach to tmux session",
            "attach",
        ),
        PaletteCommand::cmd(
            "session_new",
            "session",
            "Create new tmux session",
            "new session new-session",
        ),
        PaletteCommand::cmd(
            "session_list",
            "session",
            "List tmux sessions",
            "ls list-session",
        ),
        PaletteCommand::cmd("session_detach", "session", "Detach current", "detach"),
        PaletteCommand::cmd(
            "ssh_connect",
            "ssh",
            "Connect over SSH",
            "ssh connect remote",
        ),
        PaletteCommand::cmd("ssh_disconnect", "ssh", "Disconnect SSH", "ssh disconnect"),
        PaletteCommand::cmd(
            "open_config",
            "settings",
            "Open configuration file",
            "config settings",
        ),
        PaletteCommand::cmd(
            "reload_config",
            "settings",
            "Reload configuration",
            "config reload",
        ),
        PaletteCommand::cmd("preferences", "settings", "Preferences", "settings"),
        PaletteCommand::cmd(
            "cli_new_session",
            "cli",
            "CLI: new-session <name>",
            "cli new session new",
        ),
        PaletteCommand::cmd(
            "cli_kill_session",
            "cli",
            "CLI: kill-session <target>",
            "cli kill session kill-session",
        ),
        PaletteCommand::cmd(
            "cli_list_sessions",
            "cli",
            "CLI: list-sessions",
            "cli ls list-sessions",
        ),
        PaletteCommand::cmd(
            "cli_attach_session",
            "cli",
            "CLI: attach-session <target>",
            "cli attach attach-session",
        ),
        PaletteCommand::cmd("cli_detach", "cli", "CLI: detach", "cli detach"),
        PaletteCommand::cmd(
            "cli_new_window",
            "cli",
            "CLI: new-window [-n name]",
            "cli neww new-window window",
        ),
        PaletteCommand::cmd(
            "cli_kill_window",
            "cli",
            "CLI: kill-window <target>",
            "cli killw kill-window",
        ),
        PaletteCommand::cmd(
            "cli_list_windows",
            "cli",
            "CLI: list-windows",
            "cli lsw list-windows",
        ),
        PaletteCommand::cmd(
            "cli_select_window",
            "cli",
            "CLI: select-window <target>",
            "cli selectw select-window",
        ),
        PaletteCommand::cmd(
            "cli_new_tab",
            "cli",
            "CLI: new-tab [-n name]",
            "cli new-tab tab",
        ),
        PaletteCommand::cmd(
            "cli_kill_tab",
            "cli",
            "CLI: kill-tab <target>",
            "cli kill-tab",
        ),
        PaletteCommand::cmd(
            "cli_list_tabs",
            "cli",
            "CLI: list-tabs",
            "cli lst list-tabs",
        ),
        PaletteCommand::cmd(
            "cli_select_tab",
            "cli",
            "CLI: select-tab <target>",
            "cli select-tab",
        ),
        PaletteCommand::cmd(
            "cli_split_pane",
            "cli",
            "CLI: split-pane [-h|-v]",
            "cli splitp split-pane",
        ),
        PaletteCommand::cmd(
            "cli_kill_pane",
            "cli",
            "CLI: kill-pane <target>",
            "cli killp kill-pane",
        ),
        PaletteCommand::cmd(
            "cli_list_panes",
            "cli",
            "CLI: list-panes",
            "cli lsp list-panes",
        ),
        PaletteCommand::cmd(
            "cli_select_pane",
            "cli",
            "CLI: select-pane <target>",
            "cli selectp select-pane",
        ),
        PaletteCommand::cmd(
            "cli_resize_pane",
            "cli",
            "CLI: resize-pane <target> [-x w] [-y h]",
            "cli resizep resize-pane",
        ),
        PaletteCommand::cmd(
            "cli_send_keys",
            "cli",
            "CLI: send-keys <text>",
            "cli send send-keys",
        ),
        PaletteCommand::cmd(
            "cli_capture_pane",
            "cli",
            "CLI: capture-pane <target>",
            "cli capturep capture-pane",
        ),
        PaletteCommand::cmd("cli_list_layout", "cli", "CLI: list-layout", "cli layout"),
        PaletteCommand::cmd(
            "cli_display_message",
            "cli",
            "CLI: display-message <target>",
            "cli display-message",
        ),
    ]
}
/// 面板状态（输入框 + 选中项）。
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// 输入框内容。
    pub input: String,
    /// 当前展示的命令（过滤后）。
    pub items: Vec<PaletteCommand>,
    /// 全部命令（缓存）。
    pub all: Vec<PaletteCommand>,
    /// 选中下标（相对 items）。
    pub list: ListState,
}

impl PaletteState {
    pub fn new() -> Self {
        let all = all_commands();
        let mut list = ListState::default();
        list.select(Some(0));
        Self {
            all,
            items: Vec::new(),
            input: String::new(),
            list,
        }
    }

    /// 根据输入重新过滤。
    pub fn refresh(&mut self) {
        let q = self.input.clone();
        let filtered: Vec<PaletteCommand> =
            self.all.iter().filter(|c| c.matches(&q)).cloned().collect();
        let n = filtered.len();
        self.items = filtered;
        self.list.select(Some(0));
        let _ = n;
    }

    /// 当前选中命令。
    pub fn selected(&self) -> Option<&PaletteCommand> {
        self.items.get(self.list.selected().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_commands_nonempty_unique_ids() {
        let cmds = all_commands();
        assert!(cmds.len() >= 20, "命令数 {}", cmds.len());
        let mut ids = std::collections::HashSet::new();
        for c in &cmds {
            assert!(!c.label.is_empty());
            assert!(ids.insert(c.id), "重复 id {}", c.id);
        }
    }

    #[test]
    fn palette_has_attach_new_ssh_settings() {
        let cmds = all_commands();
        let ids: Vec<_> = cmds.iter().map(|c| c.id).collect();
        for need in [
            "session_attach",
            "session_new",
            "ssh_connect",
            "open_config",
        ] {
            assert!(ids.contains(&need), "缺少 {need}");
        }
        let groups: std::collections::HashSet<_> = cmds.iter().map(|c| c.group).collect();
        for g in ["session", "ssh", "settings", "pane", "cli"] {
            assert!(groups.contains(g), "缺少分组 {g}");
        }
    }

    #[test]
    fn fuzzy_match_works() {
        assert!(fuzzy_match("attach", "Attach to tmux session"));
        assert!(fuzzy_match("ssh", "Connect over SSH"));
        assert!(fuzzy_match("spv", "Split pane vertical"));
        assert!(!fuzzy_match("zzz", "Split pane"));
    }

    #[test]
    fn refresh_filters_by_query() {
        let mut p = PaletteState::new();
        p.refresh();
        assert_eq!(p.items.len(), p.all.len());
        p.input = "split".into();
        p.refresh();
        assert!(p.items.iter().any(|c| c.id == "new_pane_h"));
        assert!(!p.items.iter().any(|c| c.id == "open_config"));
    }
}
