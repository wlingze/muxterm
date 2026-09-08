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
    fn namespaces(&self, connection: &dyn TargetConnection) -> anyhow::Result<Vec<String>> {
        let _ = connection;
        Ok(Vec::new())
    }

    /// Discover attachable runtime-owned candidates on a target connection.
    fn discover(
        &self,
        connection: &dyn TargetConnection,
        namespace: Option<&str>,
    ) -> anyhow::Result<Vec<ExistingCandidate>>;

    /// Construct an unconnected Runtime instance.
    fn new_instance(
        &self,
        connection: Arc<dyn TargetConnection>,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>>;
}

/// Whether a transport can provide every channel required by a runtime.
///
/// Runtime and transport providers remain independent: neither side lists the
/// other side's ids. Catalog calls this relation when resolving, discovering,
/// and projecting provider information.
pub fn runtime_supports_channels(
    runtime: &dyn RuntimeProvider,
    supported_channels: &[ChannelKind],
) -> bool {
    runtime
        .channel_requirements()
        .iter()
        .all(|kind| supported_channels.contains(kind))
}
