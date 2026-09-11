//! Linux workspace sidebar.
//!
//! The sidebar is the resizable left column of the main window. It is opened
//! from the window title bar and lists every workspace currently held by the
//! Core pool, including the active workspace and background workspaces.

use gtk4::prelude::*;
use gtk4::Paned;

#[path = "workspace_sidebar_model.rs"]
mod workspace_sidebar_model;
pub use workspace_sidebar_model::{
    ActivityIndicator, AgentSidebarItem, CommandSidebarItem, WorkspaceSidebarItem,
};

#[path = "workspace_sidebar_view.rs"]
mod workspace_sidebar_view;
pub use workspace_sidebar_view::WorkspaceSidebar;
/// 折叠段仍要露出标题；`i32::MAX` 会把整段（含 COMMANDS 标题）压成 0。
const SIDEBAR_SECTION_HEADER_PX: i32 = 32;
/// GtkPaned 分割条自身占位，折叠端高度要额外留出，否则标题落在 handle 底下点不到。
const SIDEBAR_SPLIT_HANDLE_PX: i32 = 8;

fn sidebar_split_position(
    start_open: bool,
    end_open: bool,
    total: i32,
    saved: i32,
    end_headers: i32,
) -> i32 {
    // GtkPaned 未分配时 height=0。此时若算成 1px，列表会被钉死，
    // 侧栏只剩 WORKSPACES/AGENTS 标题、看不到行。
    if total <= 2 {
        return saved.max(1);
    }
    let total = total.max(1);
    let end_headers = end_headers.max(SIDEBAR_SECTION_HEADER_PX);
    match (start_open, end_open) {
        (true, true) => {
            let min_end = end_headers.max(80);
            let min_start = SIDEBAR_SECTION_HEADER_PX.max(80);
            if total <= min_start + min_end {
                (total / 2).max(1)
            } else {
                saved.clamp(min_start, total - min_end)
            }
        }
        (true, false) => {
            let keep = end_headers.saturating_add(SIDEBAR_SPLIT_HANDLE_PX);
            if total <= keep + SIDEBAR_SECTION_HEADER_PX {
                (total / 2).max(1)
            } else {
                total - keep
            }
        }
        (false, true) | (false, false) => SIDEBAR_SECTION_HEADER_PX.min(total).max(1),
    }
}

fn apply_sidebar_split(
    paned: &Paned,
    start_open: bool,
    end_open: bool,
    saved: i32,
    end_headers: i32,
) {
    let total = paned.height();
    // 与 layout_host 相同：realize 前不要 set_position，否则 1px 会粘住。
    if total <= 2 {
        return;
    }
    let want = sidebar_split_position(start_open, end_open, total, saved, end_headers);
    if (paned.position() - want).abs() > 1 {
        paned.set_position(want);
    }
}

fn persist_sidebar_divider(position: i32, total: i32) -> Option<i32> {
    let total = total.max(1);
    if position <= 1 || position >= total.saturating_sub(1) {
        return None;
    }
    Some(position)
}

fn widget_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_commands_keep_header_space() {
        let pos = sidebar_split_position(true, false, 400, 220, 64);
        assert_eq!(
            pos, 328,
            "Agents 展开、Commands 折叠时必须露出 Commands 标题（含 handle）"
        );
        assert_eq!(400 - pos, 72);
        let open = sidebar_split_position(true, true, 400, 220, 64);
        assert_eq!(open, 220);
        let clamped = sidebar_split_position(true, true, 400, 390, 64);
        assert!(
            clamped <= 320,
            "saved divider 贴底时必须给 Commands 留高度: {clamped}"
        );
        assert!(persist_sidebar_divider(i32::MAX, 400).is_none());
        assert!(persist_sidebar_divider(0, 400).is_none());
        assert_eq!(persist_sidebar_divider(180, 400), Some(180));
    }

    #[test]
    fn unallocated_split_keeps_saved_divider() {
        // GtkPaned 在 realize 前 height=0。若此时把 position 算成 1，
        // WORKSPACES/AGENTS 列表会被钉在 1px，侧栏只剩标题没有内容。
        assert_eq!(
            sidebar_split_position(true, true, 0, 260, 64),
            260,
            "height=0 must keep the construction divider"
        );
        assert_eq!(
            sidebar_split_position(true, false, 1, 220, 64),
            220,
            "height=1 must not collapse Agents onto the Commands header"
        );
        assert_eq!(sidebar_split_position(false, true, 0, 260, 64), 260);
    }

    use crate::ffi_client::{
        ClientActivitySnapshot, ClientAttentionPane, ClientPane, ClientTab, ClientWorkspace,
        ClientWorkspaceAttention,
    };
    use crate::linux::view_store::ViewStore;
    use muxterm_core::protocol::WorkspaceId;

    fn workspace_id(
        transport: &str,
        alias: Option<&str>,
        session: &str,
        runtime: &str,
        path: &str,
    ) -> WorkspaceId {
        WorkspaceId::new(transport, alias, session, runtime, path)
    }

    fn add_workspace(store: &mut ViewStore, id: &WorkspaceId, name: &str, active: bool) {
        store.replace_topology(
            ClientWorkspace {
                id: id.as_str(),
                name: name.into(),
                runtime: id.runtime.clone(),
                active,
                resolved_target: None,
            },
            vec![ClientTab {
                id: 1,
                name: "main".into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 1,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "shell".into(),
                }],
            )],
        );
    }

    fn activity(
        id: &WorkspaceId,
        status: &str,
        acknowledged: bool,
        process_name: Option<&str>,
        process_is_agent: bool,
        agent_name: Option<&str>,
    ) -> ClientActivitySnapshot {
        ClientActivitySnapshot {
            blocked_count: 0,
            workspaces: vec![ClientWorkspaceAttention {
                workspace_id: id.replica_id(),
                path: id.path.clone(),
                blocked: usize::from(status == "blocked"),
                done: usize::from(status == "done"),
                working: usize::from(status == "working"),
                panes: vec![ClientAttentionPane {
                    workspace_id: id.replica_id(),
                    pane_id: 1,
                    status: status.into(),
                    acknowledged,
                    last_line: String::new(),
                    seq: 1,
                    process_name: process_name.map(str::to_owned),
                    process_is_agent,
                    agent_name: agent_name.map(str::to_owned),
                    shell_name: Some("zsh".into()),
                }],
            }],
        }
    }

    #[test]
    fn workspace_rows_use_owned_topology_and_preserve_shortcuts() {
        let alpha = workspace_id("local", None, "alpha", "shell", "/work/alpha");
        let beta = workspace_id("ssh", Some("archmini"), "default", "herdr", "/work/beta");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &beta, "beta", false);
        add_workspace(&mut store, &alpha, "alpha", true);

        let items = WorkspaceSidebarItem::from_views(&store);
        assert_eq!(
            items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert!(items[0].active);
        assert_eq!(items[0].shortcut, Some(1));
        assert!(!items[1].active);
        assert_eq!(items[1].runtime, "herdr");
        assert_eq!(items[1].transport, "archmini");
        assert_eq!(items[1].shortcut, Some(2));

        let beta_key = beta.as_str();
        let visible_beta = WorkspaceSidebarItem::from_views_with_active(&store, Some(&beta_key));
        assert!(!visible_beta[0].active);
        assert!(visible_beta[1].active);
    }

    #[test]
    fn agent_rows_use_core_activity_identity_without_pane_title_guessing() {
        let id = workspace_id("local", None, "muxterm", "herdr", "/work/muxterm");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &id, "muxterm", true);
        let snapshot = activity(&id, "working", false, Some("codex"), true, Some("Codex"));

        let items = AgentSidebarItem::from_views(&store, &snapshot);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].workspace_id, id);
        assert_eq!(items[0].pane_id, 1);
        assert_eq!(items[0].title, "Codex");
        assert_eq!(items[0].detail, "muxterm@herdr@local · /work/muxterm");
        assert_eq!(items[0].indicator, ActivityIndicator::Running);

        let no_activity = ClientActivitySnapshot::default();
        assert!(AgentSidebarItem::from_views(&store, &no_activity).is_empty());
    }

    #[test]
    fn command_rows_follow_activity_lifecycle_and_ignore_agents() {
        let id = workspace_id("local", None, "command-workspace", "tmux", "/work/command");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &id, "command-workspace", true);

        let idle = activity(&id, "idle", true, Some("zsh"), false, None);
        assert!(CommandSidebarItem::from_views(&store, &idle).is_empty());

        let running = activity(&id, "working", true, Some("cargo test"), false, None);
        let rows = CommandSidebarItem::from_views(&store, &running);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "cargo test");
        assert_eq!(
            rows[0].detail,
            "command-workspace@tmux@local · /work/command"
        );
        assert_eq!(rows[0].indicator, ActivityIndicator::Running);

        let mut done = running.clone();
        done.workspaces[0].panes[0].status = "done".into();
        done.workspaces[0].panes[0].acknowledged = false;
        let rows = CommandSidebarItem::from_views(&store, &done);
        assert_eq!(rows[0].indicator, ActivityIndicator::Done);

        done.workspaces[0].panes[0].acknowledged = true;
        assert!(CommandSidebarItem::from_views(&store, &done).is_empty());

        let agent = activity(&id, "working", false, Some("codex"), true, Some("codex"));
        assert!(CommandSidebarItem::from_views(&store, &agent).is_empty());
        assert_eq!(
            AgentSidebarItem::from_views(&store, &agent)[0].title,
            "codex"
        );
    }

    #[test]
    fn command_rows_prefer_process_name_and_show_ssh_identity() {
        let id = workspace_id("ssh", Some("ryzen"), "default", "tmux", "/home/wlz/Devexx");
        let mut store = ViewStore::default();
        add_workspace(&mut store, &id, "Devexx", true);
        let snapshot = activity(
            &id,
            "working",
            true,
            Some("cargo test --workspace"),
            false,
            None,
        );

        let rows = CommandSidebarItem::from_views(&store, &snapshot);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "cargo test --workspace");
        assert_eq!(rows[0].detail, "Devexx@tmux@ryzen · /home/wlz/Devexx");
    }
}
