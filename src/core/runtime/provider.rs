//! Runtime provider contract.
//!
//! Providers register runtime capabilities and construct opaque Runtime
//! instances from a reusable target connection. Catalog only indexes and
//! resolves providers; it does not own their concrete implementations.

use std::sync::Arc;

use crate::core::protocol::candidate::ExistingCandidate;
use crate::core::runtime::{Runtime, RuntimeCapability};
use crate::core::transport::{ChannelKind, TargetConnection};
use crate::core::workspace::spec::WorkspaceSpec;

/// Static provider information used by Catalog/FFI runtime lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub id: String,
    pub name: String,
    pub support: Vec<RuntimeCapability>,
    pub accepted_transports: Vec<String>,
}

/// A registered Runtime implementation.
pub trait RuntimeProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn support(&self) -> &'static [RuntimeCapability];

    /// Compatibility bridge for the old Catalog transport filter.
    fn accepted_transports(&self) -> &'static [&'static str];

    /// Runtime channel requirements are checked before opening a target.
    fn channel_requirements(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec]
    }

    /// Optional named namespace discovery (Herdr sessions, for example).
    fn namespaces(&self, connection: &dyn TargetConnection) -> anyhow::Result<Vec<String>> {
        let _ = connection;
        Ok(Vec::new())
    }

    /// Discover attachable runtime-owned candidates on a target connection.
    fn discover(
        &self,
        connection: &dyn TargetConnection,
        namespace: Option<&str>,
    ) -> anyhow::Result<Vec<ExistingCandidate>> {
        self.list(connection, namespace)
    }

    /// Compatibility name used by the current Catalog implementation.
    fn list(
        &self,
        _connection: &dyn TargetConnection,
        _namespace: Option<&str>,
    ) -> anyhow::Result<Vec<ExistingCandidate>> {
        Ok(Vec::new())
    }

    /// Construct an unconnected Runtime instance.
    fn new_instance(
        &self,
        connection: Arc<dyn TargetConnection>,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>> {
        self.open(connection, spec)
    }

    /// Compatibility name for the pre-provider Catalog implementation.
    fn open(
        &self,
        _connection: Arc<dyn TargetConnection>,
        _spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>> {
        Err(anyhow::anyhow!("runtime provider has no instance factory"))
    }

    fn info(&self) -> RuntimeInfo {
        RuntimeInfo {
            id: self.id().to_string(),
            name: self.name().to_string(),
            support: self.support().to_vec(),
            accepted_transports: self
                .accepted_transports()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}
