//! Transport provider contract for target discovery and reusable connections.

use std::sync::Arc;

use super::{ChannelKind, TargetConnection, TransportResult};

/// Static information about a transport provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportInfo {
    pub id: String,
    pub name: String,
}

/// A connectable target (the local singleton or an SSH host alias).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub id: String,
    pub name: String,
}

impl TransportInfo {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

impl TargetInfo {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

/// A target-level provider that lists targets and creates reusable connections.
pub trait TransportProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;

    /// The channel kinds this target can open.
    fn supported_channels(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec]
    }

    fn list_targets(&self) -> TransportResult<Vec<TargetInfo>>;

    fn connect(&self, target: &str) -> TransportResult<Arc<dyn TargetConnection>>;

    fn info(&self) -> TransportInfo {
        TransportInfo::new(self.id(), self.name())
    }
}

/// Compatibility name for the pre-provider Catalog implementation.
pub use TransportProvider as Transport;
