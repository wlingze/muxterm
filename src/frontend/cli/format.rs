//! CLI 输出格式化：把 State 查询结果格式化为 JSON 或 text。
//!
//! 不依赖 serde_json（避免增加依赖），手写 JSON 序列化。

use crate::frontend::ffi_client::{
    ClientLayout, ClientPane, ClientTab, ClientWorkspace, FfiClient,
};
use muxterm_protocol::layout::{LayoutNode, SplitDir};
use muxterm_protocol::state::State;
use muxterm_protocol::{PaneId, TabId};

pub use muxterm_protocol::daemon::OutputFormat;

/// 格式化查询结果输出。
pub fn format_output(state: &dyn State, cmd: &super::CliCommand, format: OutputFormat) -> String {
    use super::CliCommand::*;
    match cmd {
        ListWorkspaces => format_workspaces(state, format),
        ListTabs => format_tabs(state, format),
        ListPanes { tab } => format_panes(state, *tab, format),
        ListLayout => format_layout(state, format),
        CapturePane { target, lines } => format_capture(state, *target, *lines, format),
        DisplayMessage {
            target,
            format: fmt_str,
        } => format_display(state, *target, fmt_str),
        _ => String::new(), // 非 query 命令无输出
    }
}

/// Format a query from the owned FFI DTO surface.
///
/// This is the CLI counterpart to [`format_output`].  The legacy formatter is
/// retained for the shell daemon until its IPC snapshot is migrated; ordinary
/// CLI routing must not borrow `State` or a concrete Runtime.
pub fn format_ffi_output(
    client: &FfiClient,
    workspace_id: &str,
    cmd: &super::CliCommand,
    format: OutputFormat,
) -> anyhow::Result<String> {
    use super::CliCommand::*;

    if matches!(cmd, ListWorkspaces) {
        return format_ffi_workspaces(client, format);
    }

    let snapshot = ffi_workspace_snapshot(client, workspace_id)?;
    let output = match cmd {
        ListTabs => format_ffi_tabs(&snapshot, format),
        ListPanes { tab } => format_ffi_panes(&snapshot, tab.map(|id| id.0), format),
        ListLayout => format_ffi_layout(&snapshot, format),
        CapturePane { target, lines } => {
            format_ffi_capture(&snapshot, target.map(|id| id.0), *lines)
        }
        DisplayMessage { target, format } => format_ffi_display(&snapshot, target.0, format),
        _ => String::new(),
    };
    Ok(output)
}

struct FfiWorkspaceSnapshot {
    workspace: ClientWorkspace,
    tabs: Vec<ClientTab>,
    panes: Vec<(u32, Vec<ClientPane>)>,
    layouts: Vec<(u32, Option<ClientLayout>)>,
    outputs: Vec<(u32, Vec<u8>)>,
}

fn ffi_workspace_snapshot(
    client: &FfiClient,
    workspace_id: &str,
) -> anyhow::Result<FfiWorkspaceSnapshot> {
    let workspace = client
        .workspace_list()?
        .into_iter()
        .find(|item| item.id == workspace_id)
        .ok_or_else(|| anyhow::anyhow!("Core workspace not found: {workspace_id}"))?;
    let tabs = client.get_workspace_tabs(workspace_id);
    let panes: Vec<(u32, Vec<ClientPane>)> = tabs
        .iter()
        .map(|tab| (tab.id, client.get_workspace_panes(workspace_id, tab.id)))
        .collect();
    let layouts = tabs
        .iter()
        .map(|tab| (tab.id, client.get_workspace_layout(workspace_id, tab.id)))
        .collect();
    let outputs = panes
        .iter()
        .flat_map(|(_, panes)| panes.iter().map(|pane| pane.id))
        .map(|pane_id| {
            (
                pane_id,
                client.get_workspace_pane_output(workspace_id, pane_id),
            )
        })
        .collect();
    Ok(FfiWorkspaceSnapshot {
        workspace,
        tabs,
        panes,
        layouts,
        outputs,
    })
}

fn format_ffi_workspaces(client: &FfiClient, format: OutputFormat) -> anyhow::Result<String> {
    let workspaces = client.workspace_list()?;
    match format {
        OutputFormat::Json => Ok(serde_json::to_string(
            &workspaces
                .iter()
                .map(|workspace| {
                    serde_json::json!({
                        "id": workspace.id,
                        "name": workspace.name,
                        "runtime": workspace.runtime,
                        "transport": "local",
                        "in_pool": true,
                    })
                })
                .collect::<Vec<_>>(),
        )?),
        OutputFormat::Text => Ok(workspaces
            .iter()
            .map(|workspace| {
                format!(
                    "{} ({}): {}",
                    workspace.name,
                    workspace.runtime,
                    if workspace.active { "attached" } else { "open" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")),
    }
}

fn format_ffi_tabs(snapshot: &FfiWorkspaceSnapshot, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string(
            &snapshot
                .tabs
                .iter()
                .map(|tab| {
                    serde_json::json!({
                        "id": format!("t{}", tab.id),
                        "name": tab.name,
                        "panes": snapshot
                            .panes
                            .iter()
                            .find(|(id, _)| *id == tab.id)
                            .map_or(0, |(_, panes)| panes.len()),
                        "active": tab.is_active,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into()),
        OutputFormat::Text => snapshot
            .tabs
            .iter()
            .map(|tab| {
                let panes = snapshot
                    .panes
                    .iter()
                    .find(|(id, _)| *id == tab.id)
                    .map_or(0, |(_, panes)| panes.len());
                format!(
                    "t{}: {}{} ({} panes)",
                    tab.id,
                    tab.name,
                    if tab.is_active { "*" } else { " " },
                    panes
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_ffi_panes(
    snapshot: &FfiWorkspaceSnapshot,
    tab_id: Option<u32>,
    format: OutputFormat,
) -> String {
    let tab_id = tab_id.or_else(|| {
        snapshot
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .map(|tab| tab.id)
    });
    let panes = tab_id
        .and_then(|id| snapshot.panes.iter().find(|(tab, _)| *tab == id))
        .map(|(_, panes)| panes.as_slice())
        .unwrap_or(&[]);
    match format {
        OutputFormat::Json => serde_json::to_string(
            &panes
                .iter()
                .map(|pane| {
                    serde_json::json!({
                        "id": format!("@{}", pane.id),
                        "active": pane.is_active,
                        "size": {"w": pane.cols, "h": pane.rows},
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into()),
        OutputFormat::Text => panes
            .iter()
            .map(|pane| {
                format!(
                    "@{}{} {}x{}",
                    pane.id,
                    if pane.is_active { "*" } else { " " },
                    pane.cols,
                    pane.rows
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_ffi_layout(snapshot: &FfiWorkspaceSnapshot, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string(
            &snapshot
                .tabs
                .iter()
                .map(|tab| {
                    let tree = snapshot
                        .layouts
                        .iter()
                        .find(|(id, _)| *id == tab.id)
                        .and_then(|(_, layout)| layout.as_ref())
                        .map(ffi_layout_node_to_json)
                        .unwrap_or_else(|| serde_json::Value::Null.to_string());
                    serde_json::json!({
                        "id": format!("t{}", tab.id),
                        "name": tab.name,
                        "active": tab.is_active,
                        "tree": serde_json::from_str::<serde_json::Value>(&tree)
                            .unwrap_or(serde_json::Value::Null),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into()),
        OutputFormat::Text => {
            if snapshot.tabs.is_empty() {
                return "(no tab)".into();
            }
            let mut output = format!(
                "workspace {}: {}\n",
                snapshot.workspace.runtime, snapshot.workspace.name
            );
            for (index, tab) in snapshot.tabs.iter().enumerate() {
                let prefix = if index + 1 == snapshot.tabs.len() {
                    "└─"
                } else {
                    "├─"
                };
                let active = if tab.is_active { " [active]" } else { "" };
                output.push_str(&format!(
                    "{} tab t{}: {}{}\n",
                    prefix, tab.id, tab.name, active
                ));
                if let Some(Some(layout)) = snapshot
                    .layouts
                    .iter()
                    .find(|(id, _)| *id == tab.id)
                    .map(|(_, layout)| layout.as_ref())
                {
                    for (leaf_index, pane_id) in ffi_layout_leaves(layout).iter().enumerate() {
                        let leaf_prefix = if index + 1 == snapshot.tabs.len() {
                            "   "
                        } else {
                            "│  "
                        };
                        let last = if leaf_index + 1 == ffi_layout_leaves(layout).len() {
                            "└─"
                        } else {
                            "├─"
                        };
                        let size = snapshot
                            .panes
                            .iter()
                            .flat_map(|(_, panes)| panes.iter())
                            .find(|pane| pane.id == *pane_id)
                            .map(|pane| format!("{}x{}", pane.cols, pane.rows))
                            .unwrap_or_default();
                        let active_mark = if ffi_pane_is_active(snapshot, tab.id, *pane_id) {
                            " [active]"
                        } else {
                            ""
                        };
                        output.push_str(&format!(
                            "{}   {} @{} {}{}\n",
                            leaf_prefix, last, pane_id, size, active_mark
                        ));
                    }
                }
            }
            output.trim_end().into()
        }
    }
}

fn ffi_layout_node_to_json(layout: &ClientLayout) -> String {
    match layout {
        ClientLayout::Leaf { pane_id } => format!("\"@{pane_id}\""),
        ClientLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => format!(
            "{{\"type\":\"split\",\"dir\":\"{}\",\"ratio\":{},\"first\":{},\"second\":{}}}",
            if *horizontal {
                "horizontal"
            } else {
                "vertical"
            },
            ratio,
            ffi_layout_node_to_json(first),
            ffi_layout_node_to_json(second)
        ),
    }
}

fn ffi_layout_leaves(layout: &ClientLayout) -> Vec<u32> {
    match layout {
        ClientLayout::Leaf { pane_id } => vec![*pane_id],
        ClientLayout::Split { first, second, .. } => {
            let mut leaves = ffi_layout_leaves(first);
            leaves.extend(ffi_layout_leaves(second));
            leaves
        }
    }
}

fn ffi_pane_is_active(snapshot: &FfiWorkspaceSnapshot, tab_id: u32, pane_id: u32) -> bool {
    snapshot
        .panes
        .iter()
        .find(|(id, _)| *id == tab_id)
        .and_then(|(_, panes)| panes.iter().find(|pane| pane.id == pane_id))
        .is_some_and(|pane| pane.is_active)
}

fn format_ffi_capture(
    snapshot: &FfiWorkspaceSnapshot,
    pane_id: Option<u32>,
    lines: Option<usize>,
) -> String {
    let pane_id = pane_id.or_else(|| {
        snapshot
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .and_then(|tab| {
                snapshot
                    .panes
                    .iter()
                    .find(|(id, _)| *id == tab.id)
                    .and_then(|(_, panes)| panes.iter().find(|pane| pane.is_active))
                    .map(|pane| pane.id)
            })
    });
    let text = pane_id
        .and_then(|id| snapshot.outputs.iter().find(|(pane, _)| *pane == id))
        .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();
    let Some(lines) = lines else {
        return text;
    };
    let all_lines: Vec<&str> = text.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    all_lines[start..].join("\n")
}

fn format_ffi_display(snapshot: &FfiWorkspaceSnapshot, pane_id: u32, format: &str) -> String {
    let pane = snapshot
        .panes
        .iter()
        .flat_map(|(_, panes)| panes.iter())
        .find(|pane| pane.id == pane_id);
    let Some(pane) = pane else {
        return String::new();
    };
    format
        .replace("#{pane_id}", &format!("@{}", pane.id))
        .replace("#{pane_active}", &pane.is_active.to_string())
        .replace("#{pane_width}", &pane.cols.to_string())
        .replace("#{pane_height}", &pane.rows.to_string())
        .replace("#{pane_title}", &pane.title)
}

fn format_workspaces(state: &dyn State, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => {
            let item = format!(
                r#"{{"id":"local/{}/{}","name":"{}","runtime":"{}","transport":"local","in_pool":true}}"#,
                state.workspace_runtime(),
                json_escape(state.workspace_name()),
                json_escape(state.workspace_name()),
                state.workspace_runtime()
            );
            format!("[{item}]")
        }
        OutputFormat::Text => {
            format!(
                "{} ({}): attached",
                state.workspace_name(),
                state.workspace_runtime()
            )
        }
    }
}

fn format_tabs(state: &dyn State, format: OutputFormat) -> String {
    let tabs = state.tabs();
    match format {
        OutputFormat::Json => {
            let items: Vec<String> = tabs
                .iter()
                .map(|t| {
                    let panes = state.panes(&t.id).len();
                    format!(
                        r#"{{"id":"t{}","name":"{}","panes":{},"active":{}}}"#,
                        t.id.0,
                        json_escape(&t.name),
                        panes,
                        t.active
                    )
                })
                .collect();
            format!("[{}]", items.join(","))
        }
        OutputFormat::Text => tabs
            .iter()
            .map(|t| {
                let panes = state.panes(&t.id).len();
                let mark = if t.active { "*" } else { " " };
                format!("t{}: {}{} ({} panes)", t.id.0, t.name, mark, panes)
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_panes(state: &dyn State, tab: Option<TabId>, format: OutputFormat) -> String {
    let tab_id = tab.or_else(|| state.active_tab().map(|t| t.id));
    let panes = tab_id.map(|tid| state.panes(&tid)).unwrap_or_default();
    match format {
        OutputFormat::Json => {
            let items: Vec<String> = panes
                .iter()
                .map(|p| {
                    format!(
                        r#"{{"id":"@{}","active":{},"size":{{"w":{},"h":{}}}}}"#,
                        p.id.0, p.active, p.cols, p.rows
                    )
                })
                .collect();
            format!("[{}]", items.join(","))
        }
        OutputFormat::Text => panes
            .iter()
            .map(|p| {
                let mark = if p.active { "*" } else { " " };
                format!("@{}{} {}x{}", p.id.0, mark, p.cols, p.rows)
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_layout(state: &dyn State, format: OutputFormat) -> String {
    let tabs = state.tabs();
    match format {
        OutputFormat::Json => {
            let tab_items: Vec<String> = tabs
                .iter()
                .map(|t| {
                    let tree = state
                        .layout(&t.id)
                        .map(|tl| layout_node_to_json(&tl.tree))
                        .unwrap_or_else(|| "null".to_string());
                    format!(
                        r#"{{"id":"t{}","name":"{}","active":{},"tree":{}}}"#,
                        t.id.0,
                        json_escape(&t.name),
                        t.active,
                        tree
                    )
                })
                .collect();
            format!("[{}]", tab_items.join(","))
        }
        OutputFormat::Text => {
            if tabs.is_empty() {
                return "(no tab)".to_string();
            }
            let mut out = format!(
                "workspace {}: {}\n",
                state.workspace_runtime(),
                state.workspace_name()
            );
            for (i, t) in tabs.iter().enumerate() {
                let prefix = if i == tabs.len() - 1 {
                    "└─"
                } else {
                    "├─"
                };
                let active = if t.active { " [active]" } else { "" };
                out.push_str(&format!(
                    "{} tab t{}: {}{}\n",
                    prefix, t.id.0, t.name, active
                ));
                if let Some(tl) = state.layout(&t.id) {
                    let leaves = tl.tree.leaves();
                    for (j, pid) in leaves.iter().enumerate() {
                        let leaf_prefix = if i == tabs.len() - 1 { "   " } else { "│  " };
                        let last_leaf = if j == leaves.len() - 1 {
                            "└─"
                        } else {
                            "├─"
                        };
                        let size = state
                            .pane(pid)
                            .map(|p| format!("{}x{}", p.cols, p.rows))
                            .unwrap_or_default();
                        let active_mark = if *pid == tl.active { " [active]" } else { "" };
                        out.push_str(&format!(
                            "{}   {} @{} {} {}{}\n",
                            leaf_prefix, last_leaf, pid.0, size, "", active_mark
                        ));
                    }
                }
            }
            out.trim_end().to_string()
        }
    }
}

fn layout_node_to_json(node: &LayoutNode) -> String {
    match node {
        LayoutNode::Leaf(pid) => format!(r#""@{}""#, pid.0),
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            let dir_str = match dir {
                SplitDir::Horizontal => "horizontal",
                SplitDir::Vertical => "vertical",
            };
            format!(
                r#"{{"type":"split","dir":"{}","ratio":{},"first":{},"second":{}}}"#,
                dir_str,
                ratio,
                layout_node_to_json(first),
                layout_node_to_json(second)
            )
        }
    }
}

fn format_capture(
    state: &dyn State,
    target: Option<PaneId>,
    lines: Option<usize>,
    _format: OutputFormat,
) -> String {
    let pane = target.or_else(|| state.active_pane().map(|p| p.id));
    pane.and_then(|pid| state.pane_output(&pid))
        .map(|output| {
            let text = String::from_utf8_lossy(output);
            if let Some(n) = lines {
                let all_lines: Vec<&str> = text.lines().collect();
                let start = all_lines.len().saturating_sub(n);
                all_lines[start..].join("\n")
            } else {
                text.to_string()
            }
        })
        .unwrap_or_default()
}

fn format_display(state: &dyn State, target: PaneId, fmt_str: &str) -> String {
    let pane = state.pane(&target);
    if let Some(p) = pane {
        fmt_str
            .replace("#{pane_id}", &format!("@{}", p.id.0))
            .replace("#{pane_active}", &p.active.to_string())
            .replace("#{pane_width}", &p.cols.to_string())
            .replace("#{pane_height}", &p.rows.to_string())
            .replace("#{pane_title}", &p.title)
    } else {
        String::new()
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use muxterm_core::runtime::mock::MockRuntime;
    use muxterm_protocol::command::CliCommand;

    fn mock_with_pane() -> MockRuntime {
        MockRuntime::with_single_pane()
    }

    #[test]
    fn format_workspaces_json() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListWorkspaces, OutputFormat::Json);
        assert!(out.contains(r#""name":"mock""#));
        assert!(out.starts_with('['));
    }

    #[test]
    fn format_workspaces_text() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListWorkspaces, OutputFormat::Text);
        assert!(out.contains("mock"));
    }

    #[test]
    fn format_tabs_json() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListTabs, OutputFormat::Json);
        assert!(out.contains(r#""id":"t1""#));
    }

    #[test]
    fn format_panes_json() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListPanes { tab: None }, OutputFormat::Json);
        assert!(out.contains(r#""id":"@1""#));
    }

    #[test]
    fn format_layout_json() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListLayout, OutputFormat::Json);
        assert!(out.contains(r#""id":"t1""#));
    }

    fn ffi_snapshot_with_split() -> FfiWorkspaceSnapshot {
        FfiWorkspaceSnapshot {
            workspace: ClientWorkspace {
                id: "local/shell/cli".into(),
                name: "cli".into(),
                runtime: "shell".into(),
                active: true,
                resolved_target: None,
            },
            tabs: vec![ClientTab {
                id: 1,
                name: "shell".into(),
                is_active: true,
            }],
            panes: vec![(
                1,
                vec![
                    ClientPane {
                        id: 1,
                        cols: 80,
                        rows: 24,
                        is_active: false,
                        title: "first".into(),
                    },
                    ClientPane {
                        id: 2,
                        cols: 80,
                        rows: 24,
                        is_active: true,
                        title: "second".into(),
                    },
                ],
            )],
            layouts: vec![(
                1,
                Some(ClientLayout::Split {
                    horizontal: true,
                    ratio: 500,
                    first: Box::new(ClientLayout::Leaf { pane_id: 1 }),
                    second: Box::new(ClientLayout::Leaf { pane_id: 2 }),
                }),
            )],
            outputs: vec![(1, b"first".to_vec()), (2, b"second".to_vec())],
        }
    }

    #[test]
    fn format_ffi_layout_uses_pane_activity() {
        let snapshot = ffi_snapshot_with_split();
        let out = format_ffi_layout(&snapshot, OutputFormat::Text);
        assert!(out.contains("@1 80x24\n"));
        assert!(out.contains("@2 80x24 [active]"));
        assert!(!out.contains("@1 80x24 [active]"));
    }
}
