//! Project domain record and its configuration projection.

use anyhow::{anyhow, Result};

use crate::config::ProjectDocument;
use crate::projects::TargetConfig;
use crate::workspace::provenance::WorkspaceProvenance;
use crate::workspace::template::TemplateName;

use super::{ProjectId, Worktree, WorktreeId};

/// A configured Project with its registered Worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub target: TargetConfig,
    pub template: Option<TemplateName>,
    pub worktrees: Vec<Worktree>,
}

impl Project {
    pub fn new(
        id: impl Into<ProjectId>,
        name: impl Into<String>,
        mut target: TargetConfig,
    ) -> Self {
        let name = name.into();
        target.name.clone_from(&name);
        Self {
            id: id.into(),
            name,
            target,
            template: None,
            worktrees: Vec::new(),
        }
    }

    pub fn from_document(document: &ProjectDocument) -> Result<Self> {
        let mut target = document.to_target()?;
        target.name.clone_from(&document.name);
        target.path.clone_from(&document.path);
        Ok(Self {
            id: ProjectId::from(document.id.clone()),
            name: document.name.clone(),
            target,
            template: document
                .template
                .as_deref()
                .map(TemplateName::try_from)
                .transpose()?,
            worktrees: document
                .worktrees
                .iter()
                .map(Worktree::from_document)
                .collect(),
        })
    }

    pub fn to_document(&self) -> ProjectDocument {
        let mut document = ProjectDocument::from_target(&self.target);
        document.id = self.id.to_string();
        document.name = self.name.clone();
        document.path = self.target.path.clone();
        document.template = self.template.as_ref().map(ToString::to_string);
        document.worktrees = self.worktrees.iter().map(Worktree::to_document).collect();
        document
    }

    pub fn provenance(&self) -> WorkspaceProvenance {
        WorkspaceProvenance::project(self.id.clone())
    }

    pub fn worktree_provenance(&self, worktree_id: &WorktreeId) -> WorkspaceProvenance {
        WorkspaceProvenance::worktree(self.id.clone(), worktree_id.clone())
    }

    pub fn add_worktree(&mut self, worktree: Worktree) -> Result<()> {
        if self.worktrees.iter().any(|item| item.id == worktree.id) {
            return Err(anyhow!("重复的 worktree id: {}", worktree.id));
        }
        if self.worktrees.iter().any(|item| item.path == worktree.path) {
            return Err(anyhow!("重复的 worktree path: {}", worktree.path));
        }
        self.worktrees.push(worktree);
        Ok(())
    }

    pub fn worktree(&self, id: &WorktreeId) -> Option<&Worktree> {
        self.worktrees.iter().find(|item| &item.id == id)
    }

    pub fn worktree_mut(&mut self, id: &WorktreeId) -> Option<&mut Worktree> {
        self.worktrees.iter_mut().find(|item| &item.id == id)
    }

    pub fn remove_worktree(&mut self, id: &WorktreeId) -> Option<Worktree> {
        let index = self.worktrees.iter().position(|item| &item.id == id)?;
        Some(self.worktrees.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{TargetRuntime, TargetTransport};

    fn project() -> Project {
        Project::new(
            "project-a",
            "Project A",
            TargetConfig::new(
                "old-name",
                TargetRuntime::Shell,
                TargetTransport::Local,
                "/repo",
            ),
        )
    }

    #[test]
    fn project_round_trips_worktree_projection() {
        let mut project = project();
        project.template = Some(TemplateName::try_from("default").unwrap());
        project
            .add_worktree(Worktree::new(
                "wt-main",
                "/repo-main",
                "main",
                "/repo",
                false,
            ))
            .unwrap();

        let document = project.to_document();
        assert_eq!(document.id, "project-a");
        assert_eq!(document.template.as_deref(), Some("default"));
        assert_eq!(document.worktrees.len(), 1);

        let restored = Project::from_document(&document).unwrap();
        assert_eq!(restored.id.as_str(), "project-a");
        assert_eq!(restored.name, "Project A");
        assert_eq!(restored.worktrees[0].branch, "main");
        assert_eq!(restored.worktrees[0].open_workspace, None);
    }

    #[test]
    fn project_rejects_duplicate_worktree_identity_or_path() {
        let mut project = project();
        project
            .add_worktree(Worktree::new("wt", "/repo-wt", "main", "/repo", true))
            .unwrap();
        let duplicate_id =
            project.add_worktree(Worktree::new("wt", "/repo-other", "other", "/repo", true));
        assert!(duplicate_id.is_err());
        let duplicate_path = project.add_worktree(Worktree::new(
            "wt-other", "/repo-wt", "other", "/repo", true,
        ));
        assert!(duplicate_path.is_err());
    }
}
