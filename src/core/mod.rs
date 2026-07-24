//! 核心层：全平台共用（协议、配置、共享类型）。
//!
//! - `backend`：Backend trait 的具体实现（LocalBackend / TmuxBackend）
//! - `model`：纯模型层（trait + 类型 + TerminalModel）
//! - `tmux`：tmux -CC 协议解析 + 命令构造 + pty 辅助
//! - `terminal`：终端进程管理 + scrollback + 输入编码
//! - `config` / `types` / `ssh`：配置、共享类型、远程 SSH

pub mod backend;
pub mod config;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod model;
pub mod ssh;
pub mod terminal;
pub mod tmux;
pub mod types;
