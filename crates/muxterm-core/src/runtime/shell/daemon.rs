//! Shell daemon IPC wire contract.
//!
//! The daemon is a shell-runtime execution form, so its request/response
//! types live beside the shell runtime rather than under the CLI frontend.
//! The snapshot fields remain temporarily compatible with the existing query
//! wire, while mutations and render/control notifications use the semantic
//! JSON event stream below.

use crate::protocol::command::CliCommand;
use crate::protocol::layout::{SplitDir, TabLayout};
use crate::protocol::state::State;
use crate::protocol::task::Task;
use crate::protocol::terminal::input::KeyEvent;

/// Output format requested by a daemon client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OutputFormat {
    Json,
    Text,
}

impl OutputFormat {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "text" | "txt" => Self::Text,
            _ => Self::Json,
        }
    }
}

/// Complete state snapshot exchanged by the legacy daemon wire.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    pub workspace_name: String,
    pub workspace_runtime: String,
    pub tabs: Vec<crate::protocol::state::TabInfo>,
    pub panes: Vec<crate::protocol::state::PaneInfo>,
    pub layouts: Vec<crate::protocol::layout::TabLayout>,
    /// pane_id.0 → cumulative output (lossy UTF-8; contains ANSI).
    pub outputs: Vec<(u32, String)>,
    pub status: crate::protocol::state::BackendStatus,
    pub active_tab: Option<u32>,
    pub active_pane: Option<u32>,
}

/// Control-lane baseline sent by the daemon when a client connects or when
/// topology changes.  It intentionally contains no cumulative pane output;
/// render data stays in the event stream as `PaneOutput`/`PaneSnapshot`/
/// `PaneFrame`/`PaneHistory` events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopologySnapshot {
    pub workspace_name: String,
    pub workspace_runtime: String,
    pub tabs: Vec<crate::protocol::state::TabInfo>,
    pub panes: Vec<crate::protocol::state::PaneInfo>,
    pub layouts: Vec<TabLayout>,
    pub active_tab: Option<u32>,
    pub active_pane: Option<u32>,
}

/// Client → shell daemon request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub command: CliCommand,
    pub format: OutputFormat,
}

/// Shell daemon → client response.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub ok: bool,
    pub output: String,
    pub error: String,
    /// Owned FFI event values produced while handling this request.
    ///
    /// The daemon wire keeps the event payload JSON-shaped so the CLI server
    /// can forward FFI DTOs without importing Core state types. Core's
    /// `DaemonRuntime` decodes the values at its runtime boundary.
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
}

impl Response {
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            output,
            error: String::new(),
            events: Vec::new(),
        }
    }

    pub fn ok_with_events(output: String, events: Vec<serde_json::Value>) -> Self {
        Self {
            ok: true,
            output,
            error: String::new(),
            events,
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: String::new(),
            error: error.into(),
            events: Vec::new(),
        }
    }
}

/// Map a daemon wire command to a shell-runtime Task.
///
/// Query commands intentionally return `None`; the daemon host formats those
/// queries from the current state, while mutations stay in the Core Task
/// contract.
pub fn cli_command_to_task(cmd: &CliCommand, state: &dyn State) -> Option<Task> {
    use CliCommand::*;

    match cmd {
        Config { .. } => None,

        NewWorkspace { .. } => None,
        CloseWorkspace { .. } => Some(Task::Shutdown),
        AttachWorkspace { .. } => None,
        Detach { .. } => Some(Task::Detach),
        RenameWorkspace { new_name } => Some(Task::RenameWorkspace {
            name: new_name.clone(),
        }),

        NewTab { name } => Some(Task::NewTab {
            name: name.clone(),
            command: None,
            workdir: None,
        }),
        KillTab { target } => {
            let tid = target.or_else(|| state.active_tab().map(|t| t.id))?;
            Some(Task::CloseTab { target: tid })
        }
        SelectTab { target } => Some(Task::SwitchTab { target: *target }),
        RenameTab { new_name } => {
            let tid = state.active_tab()?.id;
            Some(Task::RenameTab {
                target: tid,
                name: new_name.clone(),
            })
        }

        SplitPane {
            horizontal, target, ..
        } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id));
            let dir = if *horizontal {
                SplitDir::Horizontal
            } else {
                SplitDir::Vertical
            };
            Some(Task::SplitPane {
                target: pid,
                dir,
                command: None,
                workdir: None,
            })
        }
        KillPane { target } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id))?;
            Some(Task::ClosePane { target: pid })
        }
        SelectPane { target } => Some(Task::SwitchPane { target: *target }),
        ResizePane {
            target,
            width,
            height,
        } => match (width, height) {
            (Some(cols), Some(rows)) => Some(Task::ResizePane {
                target: *target,
                cols: *cols,
                rows: *rows,
            }),
            (Some(size), None) => Some(Task::ResizePaneAxis {
                target: *target,
                dir: SplitDir::Horizontal,
                size: *size,
            }),
            (None, Some(size)) => Some(Task::ResizePaneAxis {
                target: *target,
                dir: SplitDir::Vertical,
                size: *size,
            }),
            (None, None) => None,
        },
        ResizeClient { width, height } => Some(Task::ResizeClient {
            cols: *width,
            rows: *height,
        }),

        SendKeys { target, text } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id))?;
            let keys = text.chars().map(KeyEvent::Char).collect();
            Some(Task::SendKeys { target: pid, keys })
        }
        WriteRaw { target, data } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id))?;
            Some(Task::WriteRaw {
                target: pid,
                data: data.clone(),
            })
        }
        CapturePane { .. } => None,

        ListWorkspaces | ListTabs | ListPanes { .. } | ListLayout | PollEvents | DumpState => None,
        DisplayMessage { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::layout::SplitDir;
    use crate::protocol::task::Task;
    use crate::runtime::mock::MockRuntime;
    use crate::types::PaneId;
    use crate::workspace::terminal_model::TerminalModel;

    fn make_model() -> TerminalModel {
        TerminalModel::new(Box::new(MockRuntime::with_single_pane()))
    }

    #[test]
    fn split_pane_horizontal_maps_to_task() {
        let model = make_model();
        let task = cli_command_to_task(
            &CliCommand::SplitPane {
                horizontal: true,
                target: Some(PaneId(1)),
                size: None,
            },
            model.state(),
        );
        assert!(matches!(
            task,
            Some(Task::SplitPane {
                dir: SplitDir::Horizontal,
                ..
            })
        ));
    }

    #[test]
    fn list_workspaces_returns_none() {
        let model = make_model();
        let task = cli_command_to_task(&CliCommand::ListWorkspaces, model.state());
        assert!(task.is_none());
    }

    #[test]
    fn detach_maps_to_explicit_core_task() {
        let model = make_model();
        let task = cli_command_to_task(&CliCommand::Detach { target: None }, model.state());
        assert_eq!(task, Some(Task::Detach));
    }

    #[test]
    fn resize_pane_single_axis_maps_to_axis_task() {
        let model = make_model();
        let horizontal = cli_command_to_task(
            &CliCommand::ResizePane {
                target: PaneId(1),
                width: Some(60),
                height: None,
            },
            model.state(),
        );
        assert!(matches!(
            horizontal,
            Some(Task::ResizePaneAxis {
                dir: SplitDir::Horizontal,
                size: 60,
                ..
            })
        ));

        let vertical = cli_command_to_task(
            &CliCommand::ResizePane {
                target: PaneId(1),
                width: None,
                height: Some(18),
            },
            model.state(),
        );
        assert!(matches!(
            vertical,
            Some(Task::ResizePaneAxis {
                dir: SplitDir::Vertical,
                size: 18,
                ..
            })
        ));
    }

    #[test]
    fn resize_client_maps_to_task() {
        let model = make_model();
        let task = cli_command_to_task(
            &CliCommand::ResizeClient {
                width: 120,
                height: 36,
            },
            model.state(),
        );
        assert_eq!(
            task,
            Some(Task::ResizeClient {
                cols: 120,
                rows: 36,
            })
        );
    }

    #[test]
    fn write_raw_maps_to_task_with_bytes() {
        let model = make_model();
        let data = b"\x1b]10;rgb:0000/0000/0000\x1b\\".to_vec();
        let task = cli_command_to_task(
            &CliCommand::WriteRaw {
                target: Some(PaneId(1)),
                data: data.clone(),
            },
            model.state(),
        );
        assert_eq!(
            task,
            Some(Task::WriteRaw {
                target: PaneId(1),
                data,
            })
        );
    }
}
