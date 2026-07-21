//! Backend 实现层：LocalBackend / TmuxBackend。
//!
//! 这里集中放 `Backend` trait 的具体实现，core::model 只定义 trait。
//! - [`local`]：纯本地 shell 后端（自维护 session/window/pane + pty）
//! - [`tmux`]：tmux -CC 控制模式后端（Step 4 引入）

pub mod local;
