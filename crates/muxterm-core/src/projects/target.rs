//! Project configuration record and QuickConnect target conversion.
//!
//! Configuration owns the serializable record shape; the Projects domain owns
//! its conversion to the runtime-facing target description.

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::config_service::{ProjectDocument, ProjectRuntime, ProjectTransport};
use crate::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};

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
