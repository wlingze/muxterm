//! CLI 输出格式化：把 State 查询结果格式化为 JSON 或 text。
//!
//! 不依赖 serde_json（避免增加依赖），手写 JSON 序列化。

use crate::core::model::state::State;
use crate::core::types::{PaneId, TabId, WindowId};

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
        ListSessions => format_sessions(state, format),
        ListWindows { .. } => format_windows(state, format),
        ListTabs { .. } => format_tabs(state, format),
        ListPanes { tab } => format_panes(state, *tab, format),
        ListLayout { window } => format_layout(state, *window, format),
        CapturePane { target, lines } => format_capture(state, *target, *lines, format),
        DisplayMessage {
            target,
            format: fmt_str,
        } => format_display(state, *target, fmt_str),
        _ => String::new(), // 非 query 命令无输出
    }
}

fn format_sessions(state: &dyn State, format: OutputFormat) -> String {
    let sessions = state.sessions();
    match format {
        OutputFormat::Json => {
            let items: Vec<String> = sessions
                .iter()
                .map(|s| {
                    let windows = state.active_window().map(|_| 1).unwrap_or(0);
                    format!(
                        r#"{{"id":"${}","name":"{}","windows":{},"attached":{}}}"#,
                        s.id.0,
                        json_escape(&s.name),
                        windows,
                        s.active_window.is_some()
                    )
                })
                .collect();
            format!("[{}]", items.join(","))
        }
        OutputFormat::Text => sessions
            .iter()
            .map(|s| {
                let windows = state.active_window().map(|_| 1).unwrap_or(0);
                let attached = if s.active_window.is_some() {
                    "attached"
                } else {
                    "detached"
                };
                format!(
                    "${}: {} ({} windows, {})",
                    s.id.0, s.name, windows, attached
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_windows(state: &dyn State, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => {
            let items: Vec<String> = state
                .sessions()
                .iter()
                .flat_map(|s| {
                    // State 没有 all_windows，用 active_window
                    if let Some(w) = state.active_window() {
                        let tabs = state.tabs(&w.id).len();
                        vec![format!(
                            r#"{{"id":"w{}","name":"{}","tabs":{},"session":"${}"}}"#,
                            w.id.0,
                            json_escape(&w.name),
                            tabs,
                            s.id.0
                        )]
                    } else {
                        vec![]
                    }
                })
                .collect();
            format!("[{}]", items.join(","))
        }
        OutputFormat::Text => {
            if let Some(w) = state.active_window() {
                let tabs = state.tabs(&w.id).len();
                format!("w{}: {} ({} tabs)", w.id.0, w.name, tabs)
            } else {
                String::new()
            }
        }
    }
}

fn format_tabs(state: &dyn State, format: OutputFormat) -> String {
    let win = state.active_window();
    match format {
        OutputFormat::Json => {
            let items: Vec<String> = win
                .map(|w| {
                    state
                        .tabs(&w.id)
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
                        .collect()
                })
                .unwrap_or_default();
            format!("[{}]", items.join(","))
        }
        OutputFormat::Text => win
            .map(|w| {
                state
                    .tabs(&w.id)
                    .iter()
                    .map(|t| {
                        let panes = state.panes(&t.id).len();
                        let mark = if t.active { "*" } else { " " };
                        format!("t{}: {}{} ({} panes)", t.id.0, t.name, mark, panes)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
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

fn format_layout(state: &dyn State, window: Option<WindowId>, format: OutputFormat) -> String {
    let win = window.or_else(|| state.active_window().map(|w| w.id));
    // For text format, render a tree
    let win_info = win.and_then(|_w| {
        state
            .sessions()
            .iter()
            .find(|_| true)
            .and_then(|_| state.active_window())
    });

    let _ = win;
    match format {
        OutputFormat::Json => {
            // JSON: nested layout
            if let Some(w) = win_info {
                let tabs = state.tabs(&w.id);
                let tab_items: Vec<String> = tabs
                    .iter()
                    .map(|t| {
                        let layout = state.layout(&t.id);
                        let tree = layout
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
            } else {
                "[]".to_string()
            }
        }
        OutputFormat::Text => {
            // Text tree format
            if let Some(w) = win_info {
                let mut out = format!("window w{}: {}\n", w.id.0, w.name);
                let tabs = state.tabs(&w.id);
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
                            let pane_info = state.pane(pid);
                            let size = pane_info
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
            } else {
                "(no window)".to_string()
            }
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
    let _ = pane;
    // 简化：返回 pane 信息
    if let Some(p) = pane {
        let s = fmt_str
            .replace("#{pane_id}", &format!("@{}", p.id.0))
            .replace("#{pane_active}", &p.active.to_string())
            .replace("#{pane_width}", &p.cols.to_string())
            .replace("#{pane_height}", &p.rows.to_string())
            .replace("#{pane_title}", &p.title);
        s
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
    use crate::cli::command::CliCommand;
    use crate::core::model::backend::mock::MockBackend;
    use crate::core::model::layout::{LayoutNode, SplitDir};
    use crate::core::model::state::{PaneInfo, SessionInfo, TabInfo, WindowInfo};
    use crate::core::types::{PaneId, SessionId, TabId, WindowId};

    fn mock_with_pane() -> MockBackend {
        MockBackend::with_single_pane()
    }

    #[test]
    fn format_sessions_json() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListSessions, OutputFormat::Json);
        assert!(out.contains(r#""name":"mock""#));
        assert!(out.starts_with('['));
    }

    #[test]
    fn format_sessions_text() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListSessions, OutputFormat::Text);
        assert!(out.contains("mock"));
        assert!(out.contains("attached"));
    }

    #[test]
    fn format_panes_json() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListPanes { tab: None }, OutputFormat::Json);
        assert!(out.contains(r#""id":"@1""#));
        assert!(out.contains(r#""active":true"#));
    }

    #[test]
    fn format_panes_text() {
        let b = mock_with_pane();
        let out = format_output(&b, &CliCommand::ListPanes { tab: None }, OutputFormat::Text);
        assert!(out.contains("@1"));
        assert!(out.contains("80x24"));
    }

    #[test]
    fn format_tabs_json() {
        let b = mock_with_pane();
        let out = format_output(
            &b,
            &CliCommand::ListTabs { window: None },
            OutputFormat::Json,
        );
        assert!(out.contains(r#""id":"t1""#));
    }

    #[test]
    fn format_capture_returns_output() {
        let mut b = mock_with_pane();
        b.outputs[0].1 = b"hello world\n".to_vec();
        let out = format_output(
            &b,
            &CliCommand::CapturePane {
                target: Some(PaneId(1)),
                lines: None,
            },
            OutputFormat::Json,
        );
        assert!(out.contains("hello world"));
    }

    #[test]
    fn format_capture_with_lines() {
        let mut b = mock_with_pane();
        b.outputs[0].1 = b"line1\nline2\nline3\n".to_vec();
        let out = format_output(
            &b,
            &CliCommand::CapturePane {
                target: Some(PaneId(1)),
                lines: Some(2),
            },
            OutputFormat::Json,
        );
        assert!(out.contains("line2"));
        assert!(out.contains("line3"));
        assert!(!out.contains("line1"));
    }

    #[test]
    fn format_layout_text() {
        let b = mock_with_pane();
        let out = format_output(
            &b,
            &CliCommand::ListLayout { window: None },
            OutputFormat::Text,
        );
        assert!(out.contains("window"));
        assert!(out.contains("tab"));
    }

    #[test]
    fn format_windows_text() {
        let b = mock_with_pane();
        let out = format_output(
            &b,
            &CliCommand::ListWindows { session: None },
            OutputFormat::Text,
        );
        assert!(out.contains("w1"));
    }

    #[test]
    fn format_display_message() {
        let b = mock_with_pane();
        let out = format_output(
            &b,
            &CliCommand::DisplayMessage {
                target: PaneId(1),
                format: "#{pane_id}".into(),
            },
            OutputFormat::Text,
        );
        assert_eq!(out, "@1");
    }
}
