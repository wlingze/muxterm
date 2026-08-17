//! Catalog 的 Transport **插件**（Local / SSH）。
//!
//! 与 `crate::core::transport::Transport`（一次 spawn 的字节流）不是同一个 trait。
//! 不要叫 TransportDriver。

use std::sync::Arc;

use crate::core::catalog::connect::Connect;

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

/// Local / SSH 插件：列出 target，给出可复用 Connect。
pub trait Transport: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;

    fn list_targets(&self) -> anyhow::Result<Vec<TargetInfo>>;

    fn connect(&self, target: &str) -> anyhow::Result<Arc<Connect>>;

    fn info(&self) -> TransportInfo {
        TransportInfo::new(self.id(), self.name())
    }
}
