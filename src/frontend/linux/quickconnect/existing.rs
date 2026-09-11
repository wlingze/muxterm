//! Frontend-owned Existing rows for QuickConnect.
//!
//! Core discovery crosses this boundary as an owned FFI DTO.  The GTK panel
//! keeps only the display and attach identity it needs; it never stores the
//! Core discovery entry or Core runtime/transport enums.

use crate::frontend::ffi_client::{
    ClientCandidateRef, ClientExistingCandidateRef, ClientOpenIntent, ClientOpenRequest,
    ExistingCandidate,
};

use super::model::QuickConnectSearchTarget;

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

    /// Convert the row's typed identity into the product-level attach request.
    ///
    /// Existing rows must go through Catalog resolution. In particular, the
    /// display title is not used as a session/workspace lookup key.
    pub fn open_request(&self) -> ClientOpenRequest {
        let (transport_id, target) = match &self.transport {
            ExistingTransport::Local => ("local", "local".to_string()),
            ExistingTransport::Ssh { name } => ("ssh", name.clone()),
        };
        let (session, socket, workspace_id) = match self.runtime {
            ExistingRuntime::Shell => (None, None, None),
            ExistingRuntime::Tmux => (self.tmux_session.clone(), self.tmux_socket.clone(), None),
            ExistingRuntime::Herdr => (
                self.herdr_session.clone(),
                self.herdr_socket.clone(),
                self.herdr_workspace_id.clone(),
            ),
        };

        ClientOpenRequest {
            candidate: ClientCandidateRef::Existing {
                identity: ClientExistingCandidateRef {
                    runtime_id: self.runtime.as_str().to_string(),
                    transport_id: transport_id.to_string(),
                    target,
                    session,
                    socket,
                    workspace_id,
                },
            },
            intent: ClientOpenIntent::AttachOnly,
            template: None,
            activate: true,
        }
    }

    /// Return the stable attach identity used for sorting and deduplication.
    pub fn identity_key(&self) -> String {
        let (transport, target) = match &self.transport {
            ExistingTransport::Local => ("local", ""),
            ExistingTransport::Ssh { name } => ("ssh", name.as_str()),
        };
        let session = match self.runtime {
            ExistingRuntime::Tmux => self.tmux_session.as_deref(),
            ExistingRuntime::Herdr => self.herdr_session.as_deref(),
            ExistingRuntime::Shell => None,
        };
        let socket = match self.runtime {
            ExistingRuntime::Tmux => self.tmux_socket.as_deref(),
            ExistingRuntime::Herdr => self.herdr_socket.as_deref(),
            ExistingRuntime::Shell => None,
        };
        let workspace_id = (self.runtime == ExistingRuntime::Herdr)
            .then_some(self.herdr_workspace_id.as_deref())
            .flatten();
        let components = match self.runtime {
            ExistingRuntime::Shell => vec![
                self.runtime.as_str().to_string(),
                transport.to_string(),
                target.to_string(),
                self.title.clone(),
            ],
            ExistingRuntime::Tmux => vec![
                self.runtime.as_str().to_string(),
                transport.to_string(),
                target.to_string(),
                session
                    .filter(|value| !value.is_empty())
                    .unwrap_or(self.title.as_str())
                    .to_string(),
                socket.unwrap_or_default().to_string(),
            ],
            ExistingRuntime::Herdr
                if session.is_some_and(|value| !value.is_empty())
                    && socket.is_some_and(|value| !value.is_empty())
                    && workspace_id.is_some_and(|value| !value.is_empty()) =>
            {
                vec![
                    self.runtime.as_str().to_string(),
                    transport.to_string(),
                    target.to_string(),
                    session.unwrap_or_default().to_string(),
                    socket.unwrap_or_default().to_string(),
                    workspace_id.unwrap_or_default().to_string(),
                ]
            }
            ExistingRuntime::Herdr => vec![
                "herdr-provisional".to_string(),
                transport.to_string(),
                target.to_string(),
                self.title.clone(),
                String::new(),
            ],
        };
        components
            .iter()
            .map(|component| format!("{}:{component}", component.len()))
            .collect::<Vec<_>>()
            .join("|")
    }
}

impl QuickConnectSearchTarget for ExistingEntry {
    fn runtime_name(&self) -> &str {
        self.runtime.as_str()
    }

    fn is_local_transport(&self) -> bool {
        matches!(self.transport, ExistingTransport::Local)
    }

    fn ssh_alias(&self) -> Option<&str> {
        match &self.transport {
            ExistingTransport::Ssh { name } => Some(name),
            ExistingTransport::Local => None,
        }
    }

    fn search_fields(&self) -> Vec<String> {
        vec![
            self.title.clone(),
            self.runtime.as_str().to_string(),
            self.transport.label(),
            self.tmux_session.clone().unwrap_or_default(),
            self.tmux_socket.clone().unwrap_or_default(),
            self.herdr_session.clone().unwrap_or_default(),
            self.herdr_socket.clone().unwrap_or_default(),
            self.herdr_workspace_id.clone().unwrap_or_default(),
        ]
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
        assert_eq!(entry.herdr_session.as_deref(), Some("agents"));
        assert_eq!(
            entry.herdr_socket.as_deref(),
            Some("/remote/.config/herdr/sessions/agents/herdr.sock")
        );
        assert_eq!(entry.herdr_workspace_id.as_deref(), Some("w7"));
        let mut renamed = entry.clone();
        renamed.title = "display-only-name".into();
        assert_eq!(entry.identity_key(), renamed.identity_key());

        let request = entry.open_request();
        assert_eq!(request.intent, ClientOpenIntent::AttachOnly);
        assert!(request.activate);
        let ClientCandidateRef::Existing { identity } = request.candidate else {
            panic!("existing row must produce an Existing candidate reference");
        };
        assert_eq!(identity.runtime_id, "herdr");
        assert_eq!(identity.transport_id, "ssh");
        assert_eq!(identity.target, "buildbox");
        assert_eq!(identity.session.as_deref(), Some("agents"));
        assert_eq!(
            identity.socket.as_deref(),
            Some("/remote/.config/herdr/sessions/agents/herdr.sock")
        );
        assert_eq!(identity.workspace_id.as_deref(), Some("w7"));
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
