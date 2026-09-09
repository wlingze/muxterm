//! Runtime instance contract shared by Core and runtime implementations.
//!
//! This crate owns the behavior of one attached Runtime instance.  Runtime
//! implementations live in Core for now, but they depend on this contract
//! instead of defining the trait inside the Core module tree.

use async_trait::async_trait;
use muxterm_protocol::state::{BackendStatus, State, StateChange};
use muxterm_protocol::task::{Task, TaskOutcome};
use muxterm_protocol::WorkspaceId;

use crate::{RuntimeCapability, RuntimeError, RuntimeResult};

/// Runtime-facing fields needed to construct or reopen one instance.
///
/// Product-only fields such as provenance and templates stay in Core's
/// `WorkspaceSpec`; Core converts explicitly at the provider boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSpec {
    pub transport: String,
    pub alias: Option<String>,
    pub session: String,
    pub runtime: String,
    pub path: String,
    pub socket: Option<String>,
    pub create: bool,
    pub scrollback_lines: u32,
}

/// A git checkout known to a Runtime-native worktree API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub repo_root: String,
    /// Workspace opened for this checkout, when the Runtime reports one.
    pub open_workspace: Option<WorkspaceId>,
    /// Whether this is a linked worktree (`false` means the main checkout).
    pub linked: bool,
}

/// Product-neutral request for a Runtime-native worktree operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreateSpec {
    pub branch: String,
    pub path: String,
    pub base: Option<String>,
    pub label: Option<String>,
}

/// Behavior of one connected Runtime instance.
#[async_trait]
pub trait Runtime: State + Send {
    /// Type-erased access retained for Core diagnostics and contract tests.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Establish the Runtime connection.
    async fn connect(&mut self) -> RuntimeResult<()>;

    /// Execute one product task synchronously.
    fn execute(&mut self, task: &Task) -> RuntimeResult<TaskOutcome>;

    /// Non-blocking FIFO drain of pending state changes.
    fn take_events(&mut self) -> Vec<StateChange>;

    /// Convenience alias for the state status.
    fn runtime_status(&self) -> BackendStatus {
        self.status()
    }

    /// Capabilities actually provided by this Runtime instance.
    fn support(&self) -> &'static [RuntimeCapability] {
        &[]
    }

    /// List checkouts through a Runtime-native worktree API.
    fn list_worktrees(&self) -> RuntimeResult<Vec<WorktreeInfo>> {
        Err(RuntimeError::Unsupported {
            operation: "WorktreeList",
        })
    }

    /// Create a checkout and return the Runtime-facing workspace spec.
    fn create_worktree_spec(&self, _spec: &WorktreeCreateSpec) -> RuntimeResult<RuntimeSpec> {
        Err(RuntimeError::Unsupported {
            operation: "WorktreeCreate",
        })
    }

    /// Open an existing checkout and return the Runtime-facing workspace spec.
    fn open_worktree_spec(&self, _path: &str) -> RuntimeResult<RuntimeSpec> {
        Err(RuntimeError::Unsupported {
            operation: "WorktreeOpen",
        })
    }

    /// Whether the Runtime has an active status-bar subscription.
    fn status_subscriptions_active(&self) -> bool {
        false
    }

    /// Notify the Runtime that its Workspace is foreground/background.
    fn set_foreground(&mut self, _foreground: bool) {}

    /// Current `(down, up)` byte counters.
    fn traffic_bytes(&self) -> (u64, u64) {
        (0, 0)
    }

    /// Shut down the Runtime and release its resources.
    async fn shutdown(&mut self) -> RuntimeResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_spec_keeps_runtime_boundary_fields_only() {
        let spec = RuntimeSpec {
            transport: "ssh".into(),
            alias: Some("dev".into()),
            session: "demo".into(),
            runtime: "tmux".into(),
            path: "/srv/project".into(),
            socket: Some("muxterm-test-runtime".into()),
            create: true,
            scrollback_lines: 512,
        };
        assert_eq!(spec.transport, "ssh");
        assert_eq!(spec.alias.as_deref(), Some("dev"));
        assert!(spec.create);
        assert_eq!(spec.scrollback_lines, 512);
    }
}
