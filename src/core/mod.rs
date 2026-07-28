//! 核心层：全平台共用（协议、配置、共享类型）。
//!
//! 主链：Frontend → Core Protocol → Runtime → Transport
//! Config 横切；Discovery 连接前查询。
//!
//! - `protocol`：Core Protocol 层（Session/Window/Tab/Pane、Task、StateChange、Capability）
//! - `runtime`：Runtime 层（ShellRuntime、TmuxRuntime、Backend trait facade、RuntimeMode）
//! - `transport`：Transport 层（LocalProcessTransport、SshProcessTransport、Transport trait）
//! - `discovery`：Discovery 连接前查询（SSH hosts、tmux sessions、目录）
//! - `backend`：Backend 实现（LocalBackend / TmuxBackend / DaemonBackend，迁移中）
//! - `model`：纯模型层（trait + 类型 + TerminalModel，迁移到 protocol/runtime 中）
//! - `tmux`：tmux -CC 协议解析 + 命令构造 + pty 辅助
//! - `terminal`：终端进程管理 + scrollback + 输入编码
//! - `config` / `types` / `ssh`：配置、共享类型、远程 SSH

pub mod backend;
pub mod buffer_cap;
pub mod config;
pub mod discovery;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod model;
pub mod protocol;
pub mod runtime;
pub mod ssh;
pub mod terminal;
pub mod tmux;
pub mod transport;
pub mod types;
