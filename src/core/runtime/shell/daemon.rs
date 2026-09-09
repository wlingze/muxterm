//! Shell daemon IPC wire contract.
//!
//! The daemon is a shell-runtime execution form, so its request/response
//! types live beside the shell runtime rather than under the CLI frontend.
//! The snapshot fields remain temporarily compatible with the existing wire;
//! the event-stream replacement is a later migration step.

use crate::core::protocol::command::CliCommand;
use crate::core::protocol::layout::SplitDir;
use crate::core::protocol::state::State;
use crate::core::protocol::state::StateChange;
use crate::core::protocol::task::Task;
use crate::core::protocol::terminal::input::KeyEvent;

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
    pub tabs: Vec<crate::core::protocol::state::TabInfo>,
    pub panes: Vec<crate::core::protocol::state::PaneInfo>,
    pub layouts: Vec<crate::core::protocol::layout::TabLayout>,
    /// pane_id.0 → cumulative output (lossy UTF-8; contains ANSI).
    pub outputs: Vec<(u32, String)>,
    pub status: crate::core::protocol::state::BackendStatus,
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
    /// Runtime events produced while handling this request.
    #[serde(default)]
    pub events: Vec<StateChange>,
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

    pub fn ok_with_events(output: String, events: Vec<StateChange>) -> Self {
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

        ListWorkspaces | ListTabs | ListPanes { .. } | ListLayout | DumpState => None,
        DisplayMessage { .. } => None,
    }
}
