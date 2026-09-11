//! Runtime provider contract.
//!
//! Providers register runtime capabilities and construct opaque Runtime
//! instances from a reusable target connection.  Muxterm owns the registry;
//! Catalog exposes its read-only provider view and resolver.

use std::sync::Arc;

use crate::protocol::candidate::ExistingCandidate;
use crate::transport::{ChannelKind, TargetConnection};

use super::{Runtime, RuntimeCapability, RuntimeError, RuntimeResult, RuntimeSpec};

/// Static provider information used by Catalog and frontend-facing lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub id: String,
    pub name: String,
    pub support: Vec<RuntimeCapability>,
    /// Transport ids derived by Catalog from channel capabilities.
    pub accepted_transports: Vec<String>,
}

/// A registered Runtime implementation.
pub trait RuntimeProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn support(&self) -> &'static [RuntimeCapability];

    /// Runtime channel requirements are checked before opening a target.
    fn channel_requirements(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec]
    }

    /// Optional named namespace discovery (Herdr sessions, for example).
    fn namespaces(&self, connection: &dyn TargetConnection) -> RuntimeResult<Vec<String>> {
        let _ = connection;
        Ok(Vec::new())
    }

    /// Discover attachable Runtime-owned candidates on a target connection.
    fn discover(
        &self,
        connection: &dyn TargetConnection,
        namespace: Option<&str>,
    ) -> RuntimeResult<Vec<ExistingCandidate>>;

    /// Construct an unconnected Runtime instance.
    fn new_instance(
        &self,
        connection: Arc<dyn TargetConnection>,
        spec: &RuntimeSpec,
    ) -> RuntimeResult<Box<dyn Runtime>>;

    /// Create a missing attachable identity on this target.
    ///
    /// `label` is the user-visible name (Herdr workspace label, for example).
    /// `spec.path` is the create-time cwd. Successful implementations return a
    /// spec whose `path` is the created identity (Herdr workspace id).
    fn create_identity(
        &self,
        connection: &dyn TargetConnection,
        spec: &RuntimeSpec,
        label: Option<&str>,
    ) -> RuntimeResult<RuntimeSpec> {
        let _ = (connection, spec, label);
        Err(RuntimeError::Unsupported {
            operation: "CreateIdentity",
        })
    }

    /// Provider-level status snapshot (no live Workspace required).
    fn status_snapshot(
        &self,
        connection: &dyn TargetConnection,
        session: &str,
        socket: Option<&str>,
    ) -> RuntimeResult<serde_json::Value> {
        let _ = (connection, session, socket);
        Err(RuntimeError::Unsupported {
            operation: "StatusSnapshot",
        })
    }

    /// Optional session-name policy for a project worktree.
    fn worktree_session_name(&self, project_id: &str, worktree_id: &str) -> Option<String> {
        let _ = (project_id, worktree_id);
        None
    }
}

/// Whether a transport can provide every channel required by a Runtime.
pub fn runtime_supports_channels(
    runtime: &dyn RuntimeProvider,
    supported_channels: &[ChannelKind],
) -> bool {
    runtime
        .channel_requirements()
        .iter()
        .all(|kind| supported_channels.contains(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ExecProvider;

    impl RuntimeProvider for ExecProvider {
        fn id(&self) -> &'static str {
            "exec"
        }

        fn name(&self) -> &'static str {
            "exec"
        }

        fn support(&self) -> &'static [RuntimeCapability] {
            &[]
        }

        fn discover(
            &self,
            _connection: &dyn TargetConnection,
            _namespace: Option<&str>,
        ) -> RuntimeResult<Vec<ExistingCandidate>> {
            Ok(Vec::new())
        }

        fn new_instance(
            &self,
            _connection: Arc<dyn TargetConnection>,
            _spec: &RuntimeSpec,
        ) -> RuntimeResult<Box<dyn Runtime>> {
            unreachable!("contract-only test provider")
        }
    }

    #[test]
    fn channel_requirements_are_compared_without_runtime_transport_ids() {
        let provider = ExecProvider;
        assert!(runtime_supports_channels(&provider, &[ChannelKind::Exec]));
        assert!(!runtime_supports_channels(
            &provider,
            &[ChannelKind::UnixSocket]
        ));
    }
}
