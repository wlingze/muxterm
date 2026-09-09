//! Stable product protocol primitives shared by Core and frontends.
//!
//! This crate intentionally has no runtime, transport, workspace, or UI
//! dependency. It is the first extracted workspace crate; richer event and
//! task DTOs can move here without pulling the Core implementation graph in.

use std::path::Path;
use std::str::FromStr;

pub mod candidate;
pub mod color;
pub mod input;
pub mod layout;
pub mod state;

pub use color::Rgb;

/// Product-level pane identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PaneId(pub u32);

impl PaneId {
    pub fn as_str(self) -> String {
        format!("@{}", self.0)
    }

    /// Parse the product forms accepted at a tmux protocol boundary.
    pub fn parse(value: &str) -> Result<Self, String> {
        let number = value
            .strip_prefix('@')
            .or_else(|| value.strip_prefix('%'))
            .unwrap_or(value);
        u32::from_str(number)
            .map(Self)
            .map_err(|_| format!("pane id 非数字: {value}"))
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// Product-level tab identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TabId(pub u32);

impl TabId {
    pub fn as_str(self) -> String {
        format!("t{}", self.0)
    }

    /// Parse a tmux window identity at the runtime protocol boundary.
    pub fn parse(value: &str) -> Result<Self, String> {
        let number = value
            .strip_prefix('@')
            .ok_or_else(|| format!("window id 缺少 @ 前缀: {value}"))?;
        u32::from_str(number)
            .map(Self)
            .map_err(|_| format!("window id 非数字: {value}"))
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// Stable product workspace identity: `transport/alias/session/runtime/path`.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct WorkspaceId {
    pub transport: String,
    pub alias: Option<String>,
    pub session: String,
    pub runtime: String,
    pub path: String,
}

impl WorkspaceId {
    pub fn new(
        transport: &str,
        alias: Option<&str>,
        session: &str,
        runtime: &str,
        path: &str,
    ) -> Self {
        Self {
            transport: transport.to_string(),
            alias: alias.map(ToOwned::to_owned),
            session: session.to_string(),
            runtime: runtime.to_string(),
            path: path.to_string(),
        }
    }

    /// Replica/attention key that preserves path identity for shared sessions.
    pub fn replica_id(&self) -> String {
        let name = if self.session.is_empty() {
            default_workspace_name(&self.path)
        } else {
            self.session.clone()
        };
        let transport = if self.transport == "ssh" {
            self.alias.clone().unwrap_or_else(|| "ssh".into())
        } else {
            "local".into()
        };
        if !self.session.is_empty() && !self.path.is_empty() {
            format!("{name}:{path}@{transport}", path = self.path)
        } else {
            format!("{name}@{transport}")
        }
    }

    pub fn as_str(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.transport,
            self.alias.as_deref().unwrap_or_default(),
            self.session,
            self.runtime,
            self.path
        )
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

fn default_workspace_name(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "workspace".into();
    }
    let last = Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if last.is_empty() || last == "/" {
        "workspace".into()
    } else {
        last.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_display_and_parse() {
        let pane = PaneId::parse("%42").unwrap();
        assert_eq!(pane, PaneId(42));
        assert_eq!(pane.to_string(), "@42");
        assert_eq!(TabId::parse("@7").unwrap().as_str(), "t7");
    }

    #[test]
    fn workspace_replica_preserves_path() {
        let first = WorkspaceId::new("local", None, "shared", "shell", "one");
        let second = WorkspaceId::new("local", None, "shared", "shell", "two");
        assert_ne!(first.replica_id(), second.replica_id());
    }
}
