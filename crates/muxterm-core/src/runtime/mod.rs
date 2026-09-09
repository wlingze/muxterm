//! Runtime 层：建立在 Transport 之上，理解终端语义。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §4。
//!
//! Runtime 不关心 Transport 是 local 还是 SSH；Transport 不理解 shell/tmux 语义。
//! tmux 的 `%pane` / `@window` 等真实 ID 只能在 `runtime/tmux` 内部。

pub use muxterm_runtime::capability;
pub mod herdr;
pub mod mock;
pub mod registry;
pub mod shell;
pub mod tmux;

pub use muxterm_runtime::{
    runtime_supports_channels, Runtime, RuntimeCapability, RuntimeError, RuntimeInfo,
    RuntimeProvider, RuntimeResult, WorktreeCreateSpec, WorktreeInfo,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::herdr::HerdrRuntime;
    use crate::runtime::shell::ShellRuntime;
    use crate::runtime::tmux::backend::TmuxRuntime;

    const WORKTREE_CAPS: [RuntimeCapability; 4] = [
        RuntimeCapability::WorktreeList,
        RuntimeCapability::WorktreeCreate,
        RuntimeCapability::WorktreeOpen,
        RuntimeCapability::WorktreeRemove,
    ];

    #[test]
    fn tmux_runtime_support_has_no_worktree() {
        let rt = TmuxRuntime::new(None);
        let caps = rt.support();
        assert!(caps.contains(&RuntimeCapability::PersistDetach));
        assert!(caps.contains(&RuntimeCapability::Discover));
        assert!(caps.contains(&RuntimeCapability::MultiTab));
        assert!(caps.contains(&RuntimeCapability::SplitPane));
        assert!(caps.contains(&RuntimeCapability::SharedClientResize));
        for capability in WORKTREE_CAPS {
            assert!(!caps.contains(&capability), "tmux 不应支持 {capability:?}");
        }
    }

    #[test]
    fn shell_runtime_support_has_no_worktree() {
        let rt = ShellRuntime::new("$SHELL", "");
        let caps = rt.support();
        assert!(caps.contains(&RuntimeCapability::MultiTab));
        assert!(caps.contains(&RuntimeCapability::SplitPane));
        assert!(!caps.contains(&RuntimeCapability::SharedClientResize));
        assert!(!caps.contains(&RuntimeCapability::PersistDetach));
        assert!(!caps.contains(&RuntimeCapability::Discover));
        for capability in WORKTREE_CAPS {
            assert!(!caps.contains(&capability), "shell 不应支持 {capability:?}");
        }
    }

    #[test]
    fn core_runtime_implementations_use_the_extracted_trait() {
        let _ = std::any::TypeId::of::<HerdrRuntime>();
        fn assert_runtime<T: Runtime>() {}
        assert_runtime::<TmuxRuntime>();
        assert_runtime::<ShellRuntime>();
        assert_runtime::<HerdrRuntime>();
    }
}
