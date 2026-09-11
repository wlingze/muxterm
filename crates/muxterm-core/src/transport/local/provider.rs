//! Local TransportProvider: the local target singleton.

use std::sync::Arc;

use crate::transport::connection::Connect;
use crate::transport::provider::{TargetInfo, TransportProvider};
use crate::transport::{ChannelKind, TargetConnection, TransportResult};

/// Local transport plugin.
pub struct LocalTransport;

impl TransportProvider for LocalTransport {
    fn id(&self) -> &'static str {
        "local"
    }

    fn name(&self) -> &'static str {
        "Local"
    }

    fn supported_channels(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec, ChannelKind::UnixSocket]
    }

    fn list_targets(&self) -> TransportResult<Vec<TargetInfo>> {
        Ok(vec![TargetInfo::new("", "local")])
    }

    fn connect(&self, target: &str) -> TransportResult<Arc<dyn TargetConnection>> {
        Ok(Connect::new("local", target))
    }
}
