//! Ssh TransportProvider：读 ~/.ssh/config 的 Host，给出可复用 Connect。

use std::sync::Arc;

use anyhow::Result;

use crate::core::catalog::connect::Connect;
use crate::core::catalog::transport::{TargetInfo, TransportProvider};
use crate::core::transport::ChannelKind;

/// ssh 传输插件。
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

    fn list_targets(&self) -> Result<Vec<TargetInfo>> {
        Ok(crate::core::discovery::list_ssh_hosts(None)
            .unwrap_or_default()
            .into_iter()
            .map(|h| TargetInfo::new(&h.alias, &h.alias))
            .collect())
    }

    fn connect(&self, target: &str) -> Result<Arc<Connect>> {
        Ok(Connect::new("ssh", target))
    }
}
