//! Backend 实现层：LocalBackend / TmuxBackend / DaemonBackend。
//!
//! 这里集中放 `Backend` trait 的具体实现，core::model 只定义 trait。
//! - [`local`]：纯本地 shell 后端（自维护 session/window/pane + pty）
//! - [`tmux`]：tmux -CC 控制模式后端（封装现有 core::tmux client）
//! - [`daemon`]：连接本地 daemon 的 client 后端（TUI × local）

pub mod daemon;
pub mod local;
pub mod tmux;

// DaemonBackend：TUI / FFI client 路径（连本地 daemon）
#[cfg(any(feature = "tui", feature = "ffi"))]
pub use daemon::DaemonBackend;
pub use local::LocalBackend;
pub use tmux::TmuxBackend;
