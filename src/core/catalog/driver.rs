//! Runtime 插件（Driver）。列出候选、打开成 `Box<dyn Runtime>`。
//!
//! 不是已经 attach 的 [`crate::core::model::backend::Runtime`]。

use std::sync::Arc;

use crate::core::catalog::connect::Connect;
use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::workspace::spec::WorkspaceSpec;

/// Driver 的静态卡片信息（新建项目 / FFI `runtime_list`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub id: String,
    pub name: String,
    pub support: Vec<RuntimeCapability>,
    pub accepted_transports: Vec<String>,
}

/// 可 attach 的一格（tmux session 名或 Herdr workspace）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCandidate {
    pub runtime_id: String,
    pub transport_id: String,
    pub target: String,
    pub namespace: Option<String>,
    pub name: String,
    /// Herdr workspace_id；tmux 为空。
    pub extra: String,
}

/// Runtime 插件：在 Connect 上 list / open，自己不持有活连接池。
pub trait RuntimeDriver: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn support(&self) -> &'static [RuntimeCapability];
    fn accepted_transports(&self) -> &'static [&'static str];

    /// Herdr named session 等命名空间。tmux 可返回空。
    fn namespaces(&self, connect: &Connect) -> anyhow::Result<Vec<String>> {
        let _ = connect;
        Ok(Vec::new())
    }

    fn list(
        &self,
        connect: &Connect,
        namespace: Option<&str>,
    ) -> anyhow::Result<Vec<SessionCandidate>>;

    fn open(&self, connect: Arc<Connect>, spec: &WorkspaceSpec)
        -> anyhow::Result<Box<dyn Runtime>>;

    fn info(&self) -> RuntimeInfo {
        RuntimeInfo {
            id: self.id().to_string(),
            name: self.name().to_string(),
            support: self.support().to_vec(),
            accepted_transports: self
                .accepted_transports()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}
