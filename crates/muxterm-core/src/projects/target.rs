//! Project configuration record and QuickConnect target conversion.
//!
//! Configuration owns the serializable record shape; the Projects domain owns
//! its conversion to the runtime-facing target description.

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::config::{ProjectDocument, ProjectRuntime, ProjectTransport};

/// Runtime selected when opening a project or existing target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRuntime {
    Shell,
    Tmux,
    Herdr,
}

impl TargetRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Tmux => "tmux",
            Self::Herdr => "herdr",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "shell" => Some(Self::Shell),
            "tmux" => Some(Self::Tmux),
            "herdr" => Some(Self::Herdr),
            _ => None,
        }
    }
}

/// Transport target selected when opening a project or existing target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetTransport {
    Local,
    Ssh { name: String },
}

impl TargetTransport {
    pub fn label(&self) -> String {
        match self {
            Self::Local => "local".into(),
            Self::Ssh { name } => name.clone(),
        }
    }

    pub fn is_ssh(&self) -> bool {
        matches!(self, Self::Ssh { .. })
    }

    /// Create detached sessions through the discovery backend.
    pub fn create_backend(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Local => ("local", None),
            Self::Ssh { name } => ("ssh", Some(name.as_str())),
        }
    }

    /// Attach existing sessions through the control backend.
    pub fn attach_backend(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Local => ("tmux", None),
            Self::Ssh { name } => ("tmux-ssh", Some(name.as_str())),
        }
    }
}

/// Target identity and display metadata used by Projects and resolver inputs.
///
/// This is an interim compatibility record. The final design splits project
/// persistence, existing candidates, and WorkspaceSpec into separate types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetConfig {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    pub path: String,
    pub socket: Option<String>,
    pub session: Option<String>,
    pub workspace_id: Option<String>,
}

impl TargetConfig {
    pub fn new(
        name: impl Into<String>,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            runtime,
            transport,
            path: path.into(),
            socket: None,
            session: None,
            workspace_id: None,
        }
    }

    /// Build a tmux target for an explicitly named session.
    pub fn tmux_session(session: impl Into<String>, transport: TargetTransport) -> Self {
        let session = session.into();
        Self::new(session, TargetRuntime::Tmux, transport, "~")
    }

    /// Build a stable identity from transport, runtime, and attach fields.
    pub fn identity_key(&self) -> String {
        let (transport, target) = match &self.transport {
            TargetTransport::Local => ("local", ""),
            TargetTransport::Ssh { name } => ("ssh", name.as_str()),
        };
        let runtime = self.runtime.as_str();
        let components = match self.runtime {
            TargetRuntime::Shell => vec![
                runtime.to_string(),
                transport.to_string(),
                target.to_string(),
                if self.path.is_empty() {
                    self.name.clone()
                } else {
                    self.path.clone()
                },
            ],
            TargetRuntime::Tmux => vec![
                runtime.to_string(),
                transport.to_string(),
                target.to_string(),
                self.session
                    .clone()
                    .filter(|session| !session.is_empty())
                    .unwrap_or_else(|| self.name.clone()),
                self.socket.clone().unwrap_or_default(),
            ],
            TargetRuntime::Herdr
                if self
                    .session
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                    && self
                        .socket
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                    && self
                        .workspace_id
                        .as_deref()
                        .is_some_and(|value| !value.is_empty()) =>
            {
                vec![
                    runtime.to_string(),
                    transport.to_string(),
                    target.to_string(),
                    self.session.clone().unwrap_or_default(),
                    self.socket.clone().unwrap_or_default(),
                    self.workspace_id.clone().unwrap_or_default(),
                ]
            }
            TargetRuntime::Herdr => vec![
                "herdr-provisional".to_string(),
                transport.to_string(),
                target.to_string(),
                self.name.clone(),
                self.path.clone(),
            ],
        };
        components
            .iter()
            .map(|component| format!("{}:{component}", component.len()))
            .collect::<Vec<_>>()
            .join("|")
    }

    /// Return fields used by QuickConnect and workspace search.
    pub(crate) fn search_fields(&self) -> Vec<String> {
        let transport = match &self.transport {
            TargetTransport::Local => "local".to_string(),
            TargetTransport::Ssh { name } => format!("ssh {name}"),
        };
        vec![
            self.name.clone(),
            self.runtime.as_str().to_string(),
            transport,
            self.path.clone(),
            self.session.clone().unwrap_or_default(),
            self.socket.clone().unwrap_or_default(),
            self.workspace_id.clone().unwrap_or_default(),
        ]
    }
}

impl ProjectDocument {
    /// Convert a QuickConnect target into the serializable Project contract.
    pub fn from_target(config: &TargetConfig) -> Self {
        let (transport_id, target) = match &config.transport {
            TargetTransport::Local => ("local".to_string(), String::new()),
            TargetTransport::Ssh { name } => ("ssh".to_string(), name.clone()),
        };
        Self {
            id: format!("{}@{}", config.name, transport_id),
            name: config.name.clone(),
            path: config.path.clone(),
            runtime: ProjectRuntime {
                id: config.runtime.as_str().to_string(),
                options: BTreeMap::new(),
                session: config.session.clone(),
                socket: config.socket.clone(),
                workspace_id: config.workspace_id.clone(),
            },
            transport: ProjectTransport {
                id: transport_id,
                target,
                options: BTreeMap::new(),
            },
            template: None,
            worktrees: Vec::new(),
            command: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    /// Convert the portable Project contract back into a QuickConnect target.
    pub fn to_target(&self) -> Result<TargetConfig> {
        let runtime = TargetRuntime::from_str(&self.runtime.id)
            .ok_or_else(|| anyhow!("不支持的 project runtime: {}", self.runtime.id))?;
        let transport = match self.transport.id.to_ascii_lowercase().as_str() {
            "local" => TargetTransport::Local,
            "ssh" => {
                let alias = if self.transport.target.trim().is_empty() {
                    self.transport
                        .options
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                } else {
                    self.transport.target.as_str()
                };
                if alias.trim().is_empty() {
                    return Err(anyhow!("project {} 的 SSH transport 缺少 target", self.id));
                }
                TargetTransport::Ssh {
                    name: alias.to_string(),
                }
            }
            other => return Err(anyhow!("不支持的 project transport: {other}")),
        };
        let mut target = TargetConfig::new(&self.name, runtime, transport, &self.path);
        target.session = self.runtime.session.clone().or_else(|| {
            self.runtime
                .options
                .get("session")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        target.socket = self.runtime.socket.clone().or_else(|| {
            self.runtime
                .options
                .get("socket")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        target.workspace_id = self.runtime.workspace_id.clone().or_else(|| {
            self.runtime
                .options
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_document_json_round_trip_preserves_target_identity() {
        let mut target = TargetConfig::new(
            "agents",
            TargetRuntime::Herdr,
            TargetTransport::Ssh {
                name: "buildbox".into(),
            },
            "/work/muxterm",
        );
        target.session = Some("agents".into());
        target.socket = Some("/remote/.config/herdr/sessions/agents/herdr.sock".into());
        target.workspace_id = Some("w7".into());

        let document = ProjectDocument::from_target(&target);
        let json = serde_json::to_value(&document).expect("ProjectDocument 应可序列化");
        let restored_document: ProjectDocument =
            serde_json::from_value(json).expect("ProjectDocument 应可反序列化");
        let restored_target = restored_document
            .to_target()
            .expect("ProjectDocument 应可还原 target");

        assert_eq!(restored_target, target);
    }

    #[test]
    fn project_document_reads_legacy_runtime_and_transport_options() {
        let document = ProjectDocument {
            id: "agents@ssh".into(),
            name: "agents".into(),
            path: "/work/muxterm".into(),
            runtime: ProjectRuntime {
                id: "herdr".into(),
                options: BTreeMap::from([
                    ("session".into(), Value::String("agents".into())),
                    ("socket".into(), Value::String("/tmp/herdr.sock".into())),
                    ("workspace_id".into(), Value::String("w7".into())),
                ]),
                session: None,
                socket: None,
                workspace_id: None,
            },
            transport: ProjectTransport {
                id: "ssh".into(),
                target: String::new(),
                options: BTreeMap::from([("name".into(), Value::String("buildbox".into()))]),
            },
            template: None,
            worktrees: Vec::new(),
            command: Vec::new(),
            env: BTreeMap::new(),
        };

        let target = document.to_target().expect("legacy options 应可还原");
        assert_eq!(target.runtime, TargetRuntime::Herdr);
        assert_eq!(
            target.transport,
            TargetTransport::Ssh {
                name: "buildbox".into()
            }
        );
        assert_eq!(target.session.as_deref(), Some("agents"));
        assert_eq!(target.socket.as_deref(), Some("/tmp/herdr.sock"));
        assert_eq!(target.workspace_id.as_deref(), Some("w7"));
    }
}
