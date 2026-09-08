//! Catalog 的 Transport **插件**（Local / SSH）。
//!
//! 与 `crate::core::transport::Transport`（一次 spawn 的字节流）不是同一个 trait。
//! 不要叫 TransportDriver。

use std::sync::Arc;

use crate::core::catalog::connect::Connect;
use crate::core::transport::ChannelKind;

/// Transport 插件的静态信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportInfo {
    pub id: String,
    pub name: String,
}

/// 一个可连接的目标（Local 单例或 SSH Host alias）。
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

/// Local / SSH provider：列出 target，给出可复用 TargetConnection。
pub trait TransportProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;

    /// 此 target 能打开的通道类型。
    fn supported_channels(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec]
    }

    fn list_targets(&self) -> anyhow::Result<Vec<TargetInfo>>;

    fn connect(&self, target: &str) -> anyhow::Result<Arc<Connect>>;

    fn info(&self) -> TransportInfo {
        TransportInfo::new(self.id(), self.name())
    }
}

/// 旧命名兼容别名；新代码统一使用 [`TransportProvider`]。
pub use TransportProvider as Transport;
