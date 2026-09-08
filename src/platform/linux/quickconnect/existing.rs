//! Frontend-owned Existing rows for QuickConnect.
//!
//! Core discovery crosses this boundary as an owned FFI DTO.  The GTK panel
//! keeps only the display and attach identity it needs; it never stores the
//! Core discovery entry or Core runtime/transport enums.

use crate::platform::ffi_client::ExistingCandidate;

use super::model::{TargetConfig, TargetRuntime, TargetTransport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExistingRuntime {
    Shell,
    Tmux,
    Herdr,
}

impl ExistingRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Tmux => "tmux",
            Self::Herdr => "herdr",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "shell" => Some(Self::Shell),
            "tmux" => Some(Self::Tmux),
            "herdr" => Some(Self::Herdr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExistingTransport {
    Local,
    Ssh { name: String },
}

impl ExistingTransport {
    pub fn label(&self) -> String {
        match self {
            Self::Local => "local".to_string(),
            Self::Ssh { name } => name.clone(),
        }
    }
}

/// A QuickConnect row owned by the Linux frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingEntry {
    pub title: String,
    pub runtime: ExistingRuntime,
    pub transport: ExistingTransport,
    pub tmux_session: Option<String>,
    pub tmux_socket: Option<String>,
    pub herdr_session: Option<String>,
    pub herdr_workspace_id: Option<String>,
    pub herdr_socket: Option<String>,
}

impl ExistingEntry {
    /// Convert the owned FFI candidate without deriving identity from its title.
    pub fn from_candidate(candidate: ExistingCandidate) -> Result<Self, String> {
        let runtime = ExistingRuntime::parse(&candidate.runtime).ok_or_else(|| {
            format!(
                "unsupported Existing runtime '{}' for {}",
                candidate.runtime, candidate.name
            )
        })?;
        let transport = match candidate.transport.as_str() {
            "local" => ExistingTransport::Local,
            "ssh" => ExistingTransport::Ssh {
                name: candidate.target.clone(),
            },
            other => {
                return Err(format!(
                    "unsupported Existing transport '{other}' for {}",
                    candidate.name
                ));
            }
        };
        let tmux = runtime == ExistingRuntime::Tmux;
        let herdr = runtime == ExistingRuntime::Herdr;
        let herdr_session = herdr.then(|| {
            candidate
                .session
                .clone()
                .or(candidate.namespace.clone())
                .filter(|session| !session.is_empty())
                .unwrap_or_else(|| "default".to_string())
        });

        Ok(Self {
            title: candidate.name.clone(),
            runtime,
            transport,
            tmux_session: tmux.then(|| {
                candidate
                    .session
                    .clone()
                    .unwrap_or_else(|| candidate.name.clone())
            }),
            tmux_socket: tmux.then_some(candidate.socket.clone()).flatten(),
            herdr_session,
            herdr_workspace_id: herdr
                .then_some(candidate.workspace_id)
                .flatten()
                .filter(|id| !id.is_empty()),
            herdr_socket: herdr.then_some(candidate.socket).flatten(),
        })
    }

    /// Build a tmux row for an explicit target-side socket discovered through FFI.
    pub fn tmux(name: String, transport: ExistingTransport, socket: Option<String>) -> Self {
        Self {
            title: name.clone(),
            runtime: ExistingRuntime::Tmux,
            transport,
            tmux_session: Some(name),
            tmux_socket: socket,
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        }
    }

    pub fn subtitle(&self) -> String {
        format!("{} @ {}", self.runtime.as_str(), self.transport.label())
    }

    /// Convert a frontend row to the existing attach configuration model.
    pub fn target_config(&self) -> TargetConfig {
        let runtime = match self.runtime {
            ExistingRuntime::Shell => TargetRuntime::Shell,
            ExistingRuntime::Tmux => TargetRuntime::Tmux,
            ExistingRuntime::Herdr => TargetRuntime::Herdr,
        };
        let transport = match &self.transport {
            ExistingTransport::Local => TargetTransport::Local,
            ExistingTransport::Ssh { name } => TargetTransport::Ssh { name: name.clone() },
        };
        let path = if self.runtime == ExistingRuntime::Herdr {
            ""
        } else {
            "~"
        };
        let mut config = TargetConfig::new(self.title.clone(), runtime, transport, path);
        config.session = match self.runtime {
            ExistingRuntime::Tmux => self.tmux_session.clone(),
            ExistingRuntime::Herdr => self.herdr_session.clone(),
            ExistingRuntime::Shell => None,
        };
        config.socket = match self.runtime {
            ExistingRuntime::Tmux => self.tmux_socket.clone(),
            ExistingRuntime::Herdr => self.herdr_socket.clone(),
            ExistingRuntime::Shell => None,
        };
        config.workspace_id = self.herdr_workspace_id.clone();
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_candidate_keeps_remote_attach_identity() {
        let entry = ExistingEntry::from_candidate(ExistingCandidate {
            name: "agent-workspace".into(),
            id: String::new(),
            runtime: "herdr".into(),
            transport: "ssh".into(),
            target: "buildbox".into(),
            in_pool: false,
            windows: 0,
            attached: false,
            created: 0,
            namespace: Some("agents".into()),
            session: Some("agents".into()),
            socket: Some("/remote/.config/herdr/sessions/agents/herdr.sock".into()),
            workspace_id: Some("w7".into()),
        })
        .expect("candidate should be supported");

        assert_eq!(entry.runtime, ExistingRuntime::Herdr);
        assert_eq!(
            entry.transport,
            ExistingTransport::Ssh {
                name: "buildbox".into()
            }
        );
        let config = entry.target_config();
        assert_eq!(config.session.as_deref(), Some("agents"));
        assert_eq!(
            config.socket.as_deref(),
            Some("/remote/.config/herdr/sessions/agents/herdr.sock")
        );
        assert_eq!(config.workspace_id.as_deref(), Some("w7"));
    }

    #[test]
    fn tmux_candidate_uses_name_only_as_last_resort() {
        let entry = ExistingEntry::from_candidate(ExistingCandidate {
            name: "matrix".into(),
            id: String::new(),
            runtime: "tmux".into(),
            transport: "local".into(),
            target: "local".into(),
            in_pool: false,
            windows: 0,
            attached: false,
            created: 0,
            namespace: None,
            session: None,
            socket: Some("muxterm-test-existing".into()),
            workspace_id: None,
        })
        .expect("candidate should be supported");

        assert_eq!(entry.tmux_session.as_deref(), Some("matrix"));
        assert_eq!(entry.tmux_socket.as_deref(), Some("muxterm-test-existing"));
    }
}
