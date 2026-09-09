//! Compatibility path for the Runtime instance contract.

pub use muxterm_runtime::{
    Runtime, RuntimeCapability, RuntimeSpec, WorktreeCreateSpec, WorktreeInfo,
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
