//! Reusable target-level connection identity.

use std::sync::Arc;

use super::{ByteChannel, ChannelRequest, TargetConnection};

/// A reusable target connection (local no-op / SSH control context / test mock).
#[derive(Debug)]
pub struct Connect {
    transport_id: String,
    target: String,
}

impl Connect {
    /// Create a connection identity; the registry owns the cached `Arc`.
    pub fn new(transport_id: impl Into<String>, target: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            transport_id: transport_id.into(),
            target: target.into(),
        })
    }

    pub fn transport_id(&self) -> &str {
        &self.transport_id
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

impl TargetConnection for Connect {
    fn transport_id(&self) -> &str {
        self.transport_id()
    }

    fn target(&self) -> &str {
        self.target()
    }

    fn open_channel(&self, _request: ChannelRequest) -> anyhow::Result<Box<dyn ByteChannel>> {
        Err(anyhow::anyhow!(
            "target connection '{}' has no channel adapter yet",
            self.target
        ))
    }

    fn probe(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
