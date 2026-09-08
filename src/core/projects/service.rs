//! Project operations that bridge persisted records to Catalog open requests.

use anyhow::{anyhow, Result};

use crate::core::catalog::{Catalog, ResolveIntent};
use crate::core::workspace::id::WorkspaceId;
use crate::core::workspace::template::TemplateName;

use super::{Project, ProjectId, ProjectStore};

/// Projects facade. It owns records, while Catalog owns connections and live
/// Workspace instances.
#[derive(Debug, Clone, Default)]
pub struct ProjectsService {
    store: ProjectStore,
}

impl ProjectsService {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn new(store: ProjectStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &ProjectStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut ProjectStore {
        &mut self.store
    }

    pub fn list_projects(&self) -> &[Project] {
        self.store.projects()
    }

    pub fn get_project(&self, id: &ProjectId) -> Option<&Project> {
        self.store.get(id)
    }

    pub fn create_project(&mut self, project: Project) -> Result<()> {
        if self.store.get(&project.id).is_some() {
            return Err(anyhow!("project 已存在: {}", project.id));
        }
        self.store.upsert(project)?;
        Ok(())
    }

    pub fn update_project(&mut self, project: Project) -> Result<()> {
        if self.store.get(&project.id).is_none() {
            return Err(anyhow!("project 不存在: {}", project.id));
        }
        self.store.upsert(project)?;
        Ok(())
    }

    pub fn remove_project(&mut self, id: &ProjectId) -> Result<Option<Project>> {
        self.store.remove(id)
    }

    /// Open a Project through the single Catalog resolver path.
    pub async fn open_project(
        &self,
        catalog: &mut Catalog,
        id: &ProjectId,
        intent: ResolveIntent,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        let project = self
            .get_project(id)
            .ok_or_else(|| anyhow!("project 不存在: {id}"))?;
        let mut resolved = catalog.resolve_target(&project.target, intent)?;
        resolved.spec.provenance = Some(project.provenance());
        resolved.spec.template = template_override.or_else(|| project.template.clone());
        let workspace_id = resolved.workspace_id();
        catalog.open_resolved(resolved).await?;
        Ok(workspace_id)
    }

    /// Open a registered Worktree through the same Catalog path as a Project.
    pub async fn open_worktree(
        &mut self,
        catalog: &mut Catalog,
        project_id: &ProjectId,
        worktree_id: &super::WorktreeId,
        intent: ResolveIntent,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        let project = self
            .get_project(project_id)
            .ok_or_else(|| anyhow!("project 不存在: {project_id}"))?
            .clone();
        let worktree = project
            .worktree(worktree_id)
            .ok_or_else(|| anyhow!("worktree 不存在: {worktree_id}"))?
            .clone();

        let mut target = project.target.clone();
        target.name = if worktree.branch.trim().is_empty() {
            worktree.id.to_string()
        } else {
            worktree.branch.clone()
        };
        target.path = worktree.path;
        target.workspace_id = None;

        let mut resolved = catalog.resolve_target(&target, intent)?;
        resolved.spec.provenance = Some(project.worktree_provenance(worktree_id));
        resolved.spec.template = template_override.or(project.template.clone());
        let workspace_id = resolved.workspace_id();
        catalog.open_resolved(resolved).await?;

        if let Some(record) = self
            .store
            .get_mut(project_id)
            .and_then(|item| item.worktree_mut(worktree_id))
        {
            record.open_workspace = Some(workspace_id.clone());
        }
        Ok(workspace_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::projects::Project;
    use crate::core::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};

    fn project(id: &str) -> Project {
        Project::new(
            id,
            id,
            TargetConfig::new(id, TargetRuntime::Shell, TargetTransport::Local, "/repo"),
        )
    }

    #[test]
    fn service_owns_project_lifecycle_without_live_workspace_state() {
        let mut service = ProjectsService::in_memory();
        service.create_project(project("a")).unwrap();
        assert!(service.create_project(project("a")).is_err());

        let mut updated = project("a");
        updated.name = "A updated".into();
        service.update_project(updated).unwrap();
        assert_eq!(service.get_project(&"a".into()).unwrap().name, "A updated");

        assert!(service.remove_project(&"a".into()).unwrap().is_some());
        assert!(service.list_projects().is_empty());
    }
}
