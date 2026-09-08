//! Runtime 插件（Driver）。列出候选、打开成 `Box<dyn Runtime>`。
//!
//! 不是已经 attach 的 [`crate::core::runtime::Runtime`]。

use std::sync::Arc;

use crate::core::catalog::connect::Connect;
use crate::core::runtime::{Runtime, RuntimeCapability};
use crate::core::transport::ChannelKind;
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
///
/// W6 §11.1：session / target-side socket / workspace_id 是 typed 字段，
/// 由 Core 直接转换；platform 不得从 `extra` 猜身份。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionCandidate {
    pub runtime_id: String,
    pub transport_id: String,
    pub target: String,
    pub namespace: Option<String>,
    pub name: String,
    /// Herdr workspace_id；tmux 为空。
    pub extra: String,
    /// Herdr named session 名（typed；namespace 的别名）。
    pub session: Option<String>,
    /// Herdr target-side socket 绝对路径（SSH = 远端路径）。
    pub socket: Option<String>,
    /// Herdr workspace id（typed；extra 的别名）。
    pub workspace_id: Option<String>,
}

/// Runtime provider：在 TargetConnection 上 discover / instantiate，自己不持有
/// 活连接池。
pub trait RuntimeProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn support(&self) -> &'static [RuntimeCapability];
    fn accepted_transports(&self) -> &'static [&'static str];

    /// Runtime 需要的通道类型；Catalog 用它和 TransportProvider 的能力求交。
    fn channel_requirements(&self) -> &'static [ChannelKind] {
        &[ChannelKind::Exec]
    }

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

    /// 构造尚未 connect 的 Runtime instance。
    ///
    /// `open` 是旧命名的兼容入口；新调用方应使用 `new_instance`。
    fn new_instance(
        &self,
        connect: Arc<Connect>,
        spec: &WorkspaceSpec,
    ) -> anyhow::Result<Box<dyn Runtime>> {
        self.open(connect, spec)
    }

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

/// 旧命名兼容别名；新代码统一使用 [`RuntimeProvider`]。
pub use RuntimeProvider as RuntimeDriver;
