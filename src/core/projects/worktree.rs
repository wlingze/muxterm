//! Project-owned git checkout records.

use crate::core::config_service::WorktreeDocument;
use crate::core::workspace::id::WorkspaceId;

use super::WorktreeId;

/// A git checkout registered under a Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub id: WorktreeId,
    pub path: String,
    pub branch: String,
    pub repo_root: String,
    pub linked: bool,
    /// Runtime state; deliberately not persisted in the Project document.
    pub open_workspace: Option<WorkspaceId>,
}

impl Worktree {
    pub fn new(
        id: impl Into<WorktreeId>,
        path: impl Into<String>,
        branch: impl Into<String>,
        repo_root: impl Into<String>,
        linked: bool,
    ) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            branch: branch.into(),
            repo_root: repo_root.into(),
            linked,
            open_workspace: None,
        }
    }

    pub(crate) fn from_document(document: &WorktreeDocument) -> Self {
        Self::new(
            document.id.clone(),
            document.path.clone(),
            document.branch.clone(),
            document.repo_root.clone(),
            document.linked,
        )
    }

    pub(crate) fn to_document(&self) -> WorktreeDocument {
        WorktreeDocument {
            id: self.id.to_string(),
            path: self.path.clone(),
            branch: self.branch.clone(),
            repo_root: self.repo_root.clone(),
            linked: self.linked,
        }
    }
}
