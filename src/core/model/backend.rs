//! Runtime 契约的兼容入口。
//!
//! Runtime 所有权已经迁移到 [`crate::core::runtime::contract`]。这个模块只
//! 保留旧路径和测试 mock，供尚未迁移的调用方逐步切换。

pub use crate::core::model::state::{BackendStatus, State, StateChange};
pub use crate::core::model::task::{Task, TaskOutcome};
pub use crate::core::runtime::contract::{
    Runtime, RuntimeCapability, WorktreeCreateSpec, WorktreeInfo,
};
pub use async_trait::async_trait;

pub mod mock;
