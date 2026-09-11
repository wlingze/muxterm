//! Stable line-oriented daemon IPC values.
//!
//! The socket client/server implementations live in the runtime and CLI
//! adapters.  This module owns only the values that cross that boundary, so
//! Core and the CLI cannot accidentally grow two incompatible wire contracts.

use crate::protocol::command::CliCommand;
use crate::protocol::layout::TabLayout;
use crate::protocol::state::{PaneInfo, TabInfo};

/// Return the default Unix socket path shared by the daemon host and clients.
///
/// The path is part of the daemon endpoint contract, while the socket I/O
/// implementations remain in the runtime and CLI adapters.
pub fn default_socket_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    let safe: String = name
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    dir.join(format!("muxterm-{safe}.sock"))
}

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

/// Control-lane baseline sent by the daemon when a client connects or when
/// topology changes.  It intentionally contains no cumulative pane output;
/// render data stays in the event stream as pane output/frame/history events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TopologySnapshot {
    pub workspace_name: String,
    pub workspace_runtime: String,
    pub tabs: Vec<TabInfo>,
    pub panes: Vec<PaneInfo>,
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
    /// Owned JSON event values produced while handling this request.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_the_shared_command_contract() {
        let request = Request {
            command: CliCommand::SplitPane {
                horizontal: true,
                target: Some(crate::protocol::PaneId(1)),
                size: None,
            },
            format: OutputFormat::Json,
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded.format, OutputFormat::Json));
        assert!(matches!(
            decoded.command,
            CliCommand::SplitPane {
                horizontal: true,
                target: Some(crate::protocol::PaneId(1)),
                ..
            }
        ));
    }

    #[test]
    fn response_preserves_event_values() {
        let response = Response::ok_with_events(
            "events".into(),
            vec![serde_json::json!({
                "kind": "pane_output",
                "pane_id": 1,
                "data": [0, 255, 27]
            })],
        );
        let json = serde_json::to_string(&response).unwrap();
        let decoded: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.output, "events");
        assert_eq!(decoded.events, response.events);
    }
}
