//! Local TransportProvider：本机单例。

use std::sync::Arc;

use anyhow::Result;

use crate::core::catalog::connect::Connect;
use crate::core::catalog::transport::{TargetInfo, TransportProvider};
use crate::core::transport::ChannelKind;

/// local 传输插件。
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

    fn list_targets(&self) -> Result<Vec<TargetInfo>> {
        Ok(vec![TargetInfo::new("", "local")])
    }

    fn connect(&self, target: &str) -> Result<Arc<Connect>> {
        Ok(Connect::new("local", target))
    }
}
