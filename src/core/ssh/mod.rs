//! SSH 远程传输层：连接远端并跑 `tmux -CC`。
//!
//! 与本地 [`crate::core::tmux::TmuxClient`] 对称：产出同一套
//! [`crate::core::tmux::TmuxEvent`] / [`crate::core::tmux::command::TmuxCommand`]。

pub mod client;

#[allow(unused_imports)] // 供平台层与后续模块选用
pub use client::{CommandStream, RemoteTmuxClient, SshAuth, SshConfig, SshError, SshSession};
