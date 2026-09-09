//! Project-owned git checkout records.

use crate::config::WorktreeDocument;
use muxterm_protocol::WorkspaceId;

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

/// Build argv for `git worktree add` without invoking a shell.
pub fn git_worktree_add_argv(
    repo_root: &str,
    path: &str,
    branch: Option<&str>,
    base: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![
        "git".to_string(),
        "-C".to_string(),
        repo_root.to_string(),
        "worktree".to_string(),
        "add".to_string(),
    ];
    if let Some(branch) = branch.filter(|branch| !branch.trim().is_empty()) {
        argv.push("-b".into());
        argv.push(branch.into());
    }
    argv.push(path.into());
    if let Some(base) = base.filter(|base| !base.trim().is_empty()) {
        argv.push(base.into());
    }
    argv
}

#[cfg(test)]
mod command_tests {
    use super::git_worktree_add_argv;

    #[test]
    fn git_worktree_command_is_shell_free_and_preserves_paths() {
        assert_eq!(
            git_worktree_add_argv(
                "/repo root",
                "/checkout path",
                Some("feature/x"),
                Some("main")
            ),
            vec![
                "git",
                "-C",
                "/repo root",
                "worktree",
                "add",
                "-b",
                "feature/x",
                "/checkout path",
                "main"
            ]
        );
        assert_eq!(
            git_worktree_add_argv("/repo", "/checkout", None, None),
            vec!["git", "-C", "/repo", "worktree", "add", "/checkout"]
        );
    }
}
