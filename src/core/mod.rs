//! 核心层：全平台共用。
//!
//! 主链：Frontend → Core Protocol → Runtime → Transport
//! Config 横切；Discovery 连接前查询。
//!
//! - `model`：纯领域模型（Session/Window/Tab/Pane、Task、StateChange、Layout、Backend trait）
//! - `runtime`：Runtime 层（shell/、tmux/、daemon，实现 Backend trait）
//! - `transport`：Transport 层（local、ssh，字节流 + PTY）
//! - `protocol`：Core Protocol facade（Capability）
//! - `discovery`：连接前无状态查询
//! - `tmux`：tmux 协议解析 + 命令构造 + client（被 runtime/tmux 复用）
//! - `terminal`：终端输入编码 + 进程管理 + scrollback
//! - `config` / `types` / `ssh` / `buffer_cap`

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
