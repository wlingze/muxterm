//! Backend 实现层：LocalBackend / TmuxBackend / DaemonBackend。
//!
//! 这里集中放 `Backend` trait 的具体实现，core::model 只定义 trait。
//! - [`local`]：纯本地 shell 后端（自维护 session/window/pane + pty）
//! - [`tmux`]：tmux -CC 控制模式后端（封装现有 core::tmux client）
//! - [`daemon`]：连接本地 daemon 的 client 后端（TUI × local）

pub mod daemon;
pub mod local;
pub mod tmux;

// DaemonBackend 仅 TUI 路径直接 `use`；无 tui feature 时 bin 会报 unused_imports
#[cfg(feature = "tui")]
pub use daemon::DaemonBackend;
pub use local::LocalBackend;
pub use tmux::TmuxBackend;
