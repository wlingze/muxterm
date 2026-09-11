//! Runtime capability contract.
//!
//! Capabilities describe what one Runtime instance can do.  They are kept
//! separate from the Runtime trait so resolver and frontend-facing support
//! views do not need to depend on a concrete runtime implementation.

/// A capability provided by a Runtime instance.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeCapability {
    /// shutdown/关窗后远端还在，能再 attach。
    PersistDetach,
    /// 连接前能列出可 open 的候选。
    Discover,
    /// `NewTab` / `SwitchTab` 有意义。
    MultiTab,
    /// `SplitPane` 有意义。
    SplitPane,
    /// Runtime 的全部 pane 共享一个 client viewport。
    SharedClientResize,
    /// 能列出当前仓库的 checkout。
    WorktreeList,
    /// 能建 checkout 并打开成新 Workspace。
    WorktreeCreate,
    /// 能打开已有 checkout。
    WorktreeOpen,
    /// 能 `git worktree remove`（不删分支）。
    WorktreeRemove,
}
