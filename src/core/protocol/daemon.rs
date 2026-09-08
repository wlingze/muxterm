//! Daemon IPC wire DTOs shared by the runtime and CLI adapter.

use crate::core::protocol::command::CliCommand;

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

/// Complete state snapshot exchanged by the daemon wire.
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

/// Client → daemon request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub command: CliCommand,
    pub format: OutputFormat,
}

/// Daemon → client response.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub ok: bool,
    pub output: String,
    pub error: String,
}

impl Response {
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            output,
            error: String::new(),
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: String::new(),
            error: error.into(),
        }
    }
}
