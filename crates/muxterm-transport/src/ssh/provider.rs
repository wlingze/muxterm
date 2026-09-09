//! SSH transport provider backed by the user's system SSH configuration.

use std::sync::Arc;

use crate::connection::Connect;
use crate::provider::{TargetInfo, TransportProvider};
use crate::{ChannelKind, TargetConnection, TransportResult};

/// SSH transport plugin.
pub struct SshTransport;

impl TransportProvider for SshTransport {
    fn id(&self) -> &'static str {
        "ssh"
    }

    fn name(&self) -> &'static str {
        "SSH"
    }

    fn supported_channels(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec, ChannelKind::UnixSocket]
    }

    fn list_targets(&self) -> TransportResult<Vec<TargetInfo>> {
        Ok(crate::ssh::config::list_ssh_hosts(None)
            .unwrap_or_default()
            .into_iter()
            .map(|host| TargetInfo::new(&host.alias, &host.alias))
            .collect())
    }

    fn connect(&self, target: &str) -> TransportResult<Arc<dyn TargetConnection>> {
        Ok(Connect::new("ssh", target))
    }
}
