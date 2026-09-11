//! Runtime 层：建立在 Transport 之上，理解终端语义。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §4。
//!
//! Runtime 不关心 Transport 是 local 还是 SSH；Transport 不理解 shell/tmux 语义。
//! tmux 的 `%pane` / `@window` 等真实 ID 只能在 `runtime/tmux` 内部。

pub mod batch;
pub mod capability;
pub mod contract;
pub mod error;
#[cfg(feature = "test-harness")]
pub mod herdr;
#[cfg(not(feature = "test-harness"))]
mod herdr;
#[cfg(feature = "test-harness")]
pub mod mock;
#[cfg(not(feature = "test-harness"))]
mod mock;
pub mod provider;
pub mod registry;
#[cfg(feature = "test-harness")]
pub mod shell;
#[cfg(not(feature = "test-harness"))]
mod shell;
#[cfg(feature = "test-harness")]
pub mod tmux;
#[cfg(not(feature = "test-harness"))]
mod tmux;

pub use batch::{ControlEvent, RenderEvent, RuntimeBatch, RuntimeSignal};
pub use capability::RuntimeCapability;
pub use contract::{Runtime, RuntimeSpec, WorktreeCreateSpec, WorktreeInfo};
pub use error::{RuntimeError, RuntimeResult};
#[cfg(any(test, feature = "test-harness"))]
pub(crate) use mock::MockRuntime;
pub use provider::{runtime_supports_channels, RuntimeInfo, RuntimeProvider};
pub use registry::RuntimeRegistry;

/// Legacy C constructor helper. Lives here so FFI does not name TmuxDriver.
pub(crate) fn legacy_ssh_alias_and_tmux_socket(
    socket: Option<&str>,
    alias: Option<&str>,
) -> Option<(String, Option<String>)> {
    tmux::backend::TmuxRuntime::ssh_alias_and_tmux_socket(socket, alias)
}

/// Host-side daemon adapter. Daemon is a shell form, not a fourth provider.
pub(crate) fn new_daemon_runtime(path: std::path::PathBuf, name: String) -> Box<dyn Runtime> {
    Box::new(shell::daemon_runtime::DaemonRuntime::new(path, name))
}

/// One Herdr workspace discovered on a named session socket.
pub(crate) struct HerdrWorkspaceProbe {
    pub workspace_id: String,
    pub label: String,
}

/// Probe a Herdr named session without exposing `HerdrSession`.
pub(crate) fn list_herdr_workspaces_at(
    session_name: &str,
    socket: &std::path::Path,
) -> Option<Vec<HerdrWorkspaceProbe>> {
    let session = herdr::session::HerdrSession::new(session_name, socket);
    if session.ping().is_err() {
        return None;
    }
    let list = session.workspace_list().ok()?;
    Some(
        list.into_iter()
            .map(|ws| HerdrWorkspaceProbe {
                workspace_id: ws.workspace_id,
                label: ws.label,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::herdr::runtime::HerdrRuntime;
    use super::shell::ShellRuntime;
    use super::tmux::backend::TmuxRuntime;
    use super::*;

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
