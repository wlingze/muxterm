//! Local Transport：本机单例。

use std::sync::Arc;

use anyhow::Result;

use crate::core::catalog::connect::Connect;
use crate::core::catalog::transport::{TargetInfo, Transport};

/// local 传输插件。
pub struct LocalTransport;

impl Transport for LocalTransport {
    fn id(&self) -> &'static str {
        "local"
    }

    fn name(&self) -> &'static str {
        "Local"
    }

    fn list_targets(&self) -> Result<Vec<TargetInfo>> {
        Ok(vec![TargetInfo::new("", "local")])
    }

    fn connect(&self, target: &str) -> Result<Arc<Connect>> {
        Ok(Connect::new("local", target))
    }
}
