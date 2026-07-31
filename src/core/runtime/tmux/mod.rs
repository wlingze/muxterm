//! TmuxRuntime：tmux 控制模式运行时。
//!
//! 包含：backend、client、command、protocol、pty、ssh_client
//!
//! 内部含 adapter（协议解析 + 命令映射 + ID 映射）。
//! tmux 的 %pane/@window 等真实 ID 只在此模块内部。

pub mod backend;
pub mod client;
pub mod command;
pub mod protocol;
pub mod pty;
pub mod ssh_client;

pub use backend::TmuxBackend;
