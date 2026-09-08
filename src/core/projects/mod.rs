//! Projects 领域与 Workspace provenance 共用的稳定标识。

mod project;
mod service;
mod store;
mod worktree;

pub use project::Project;
pub use service::ProjectsService;
pub use store::ProjectStore;
pub use worktree::{git_worktree_add_argv, Worktree};

/// 配置项 Project 的稳定标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ProjectId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ProjectId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Project 下 Worktree 的稳定标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorktreeId(String);

impl WorktreeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for WorktreeId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for WorktreeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for WorktreeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_typed_and_display_stably() {
        let project = ProjectId::from("project-a");
        let worktree = WorktreeId::from(String::from("worktree-1"));

        assert_eq!(project.as_str(), "project-a");
        assert_eq!(project.to_string(), "project-a");
        assert_eq!(worktree.as_str(), "worktree-1");
        assert_eq!(worktree.to_string(), "worktree-1");
    }
}
