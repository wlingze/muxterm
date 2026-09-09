//! 把活动 Workspace 连接到所属 Project/Worktree 的 provenance。

use crate::projects::{ProjectId, WorktreeId};

/// 从 Projects 领域打开的 Workspace 所携带的归属元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceProvenance {
    pub project_id: Option<ProjectId>,
    pub worktree_id: Option<WorktreeId>,
}

impl WorkspaceProvenance {
    pub fn new(project_id: Option<ProjectId>, worktree_id: Option<WorktreeId>) -> Self {
        Self {
            project_id,
            worktree_id,
        }
    }

    pub fn project(project_id: impl Into<ProjectId>) -> Self {
        Self::new(Some(project_id.into()), None)
    }

    pub fn worktree(project_id: impl Into<ProjectId>, worktree_id: impl Into<WorktreeId>) -> Self {
        Self::new(Some(project_id.into()), Some(worktree_id.into()))
    }
}
