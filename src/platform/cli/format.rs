//! CLI 输出格式化：把 State 查询结果格式化为 JSON 或 text。
//!
//! 不依赖 serde_json（避免增加依赖），手写 JSON 序列化。

use crate::core::model::state::State;
use crate::core::types::{PaneId, TabId};

/// 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OutputFormat {
    Json,
    Text,
}

impl OutputFormat {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "text" | "txt" => OutputFormat::Text,
            _ => OutputFormat::Json,
        }
    }
}

/// 格式化查询结果输出。
pub fn format_output(
    state: &dyn State,
    cmd: &super::command::CliCommand,
    format: OutputFormat,
) -> String {
    use super::command::CliCommand::*;
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
        DumpState => format_dump_state(state),
        _ => String::new(), // 非 query 命令无输出
    }
}

/// 完整状态快照（供 TUI DaemonRuntime 反序列化）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    pub workspace_name: String,
    pub workspace_runtime: String,
    pub tabs: Vec<crate::core::model::state::TabInfo>,
    pub panes: Vec<crate::core::model::state::PaneInfo>,
    pub layouts: Vec<crate::core::model::layout::TabLayout>,
    /// pane_id.0 → 累计输出（lossy UTF-8；含 ANSI）。
    pub outputs: Vec<(u32, String)>,
    pub status: crate::core::model::state::BackendStatus,
    pub active_tab: Option<u32>,
    pub active_pane: Option<u32>,
}

fn format_dump_state(state: &dyn State) -> String {
    let mut tabs = Vec::new();
    let mut panes = Vec::new();
    let mut layouts = Vec::new();
    let mut outputs = Vec::new();

    for t in state.tabs() {
        tabs.push(t.clone());
        if let Some(layout) = state.layout(&t.id) {
            layouts.push(layout.clone());
        }
        for p in state.panes(&t.id) {
            panes.push(p.clone());
            if let Some(out) = state.pane_output(&p.id) {
                outputs.push((p.id.0, String::from_utf8_lossy(out).into_owned()));
            }
        }
    }

    let snap = StateSnapshot {
        workspace_name: state.workspace_name().to_string(),
        workspace_runtime: state.workspace_runtime().to_string(),
        tabs,
        panes,
        layouts,
        outputs,
        status: state.status(),
        active_tab: state.active_tab().map(|t| t.id.0),
        active_pane: state.active_pane().map(|p| p.id.0),
    };
    serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into())
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

fn layout_node_to_json(node: &crate::core::model::layout::LayoutNode) -> String {
    use crate::core::model::layout::LayoutNode;
    match node {
        LayoutNode::Leaf(pid) => format!(r#""@{}""#, pid.0),
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            let dir_str = match dir {
                crate::core::model::layout::SplitDir::Horizontal => "horizontal",
                crate::core::model::layout::SplitDir::Vertical => "vertical",
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
    use crate::core::model::backend::mock::MockRuntime;
    use crate::platform::cli::command::CliCommand;

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

    #[test]
    fn format_dump_state_has_workspace() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::DumpState, OutputFormat::Json);
        assert!(out.contains(r#""workspace_name":"mock""#));
    }
}
