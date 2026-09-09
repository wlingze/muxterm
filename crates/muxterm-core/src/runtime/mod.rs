//! Runtime 层：建立在 Transport 之上，理解终端语义。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §4。
//!
//! Runtime 不关心 Transport 是 local 还是 SSH；Transport 不理解 shell/tmux 语义。
//! tmux 的 `%pane` / `@window` 等真实 ID 只能在 `runtime/tmux` 内部。

pub use muxterm_runtime::capability;
pub mod contract;
pub mod daemon;
pub mod daemon_client;
pub mod herdr;
pub mod mock;
pub mod provider;
pub mod registry;
pub mod shell;
pub mod tmux;

pub use contract::{Runtime, WorktreeCreateSpec, WorktreeInfo};
pub use muxterm_runtime::RuntimeCapability;
pub use provider::RuntimeProvider;

// Re-export backend implementations. Their concrete types remain an internal
// compatibility surface until RuntimeProvider registration owns construction.
pub use daemon::DaemonRuntime;
pub use herdr::{HerdrRuntime, HerdrSession};
pub use shell::ShellRuntime;
pub use tmux::backend::TmuxRuntime;
