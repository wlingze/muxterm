//! Project operations that bridge persisted records to Catalog open requests.

use anyhow::{anyhow, Result};

use crate::catalog::OpenRequest;
use crate::catalog::{Catalog, ResolveIntent};
use crate::executable::expand_config_value;
use crate::protocol::candidate::CandidateRef;
use crate::quickconnect::model::{TargetRuntime, TargetTransport};
use crate::runtime::WorktreeCreateSpec;
use crate::transport::registry::ConnectionRegistry;
use crate::transport::ChannelRequest;
use crate::workspace::pool::WorkspacePool;
use crate::workspace::template::TemplateName;
use muxterm_protocol::WorkspaceId;

use super::{git_worktree_add_argv, Project, ProjectId, ProjectStore, Worktree, WorktreeId};

/// Projects facade. It owns records, while Muxterm owns reusable connections
/// and live Workspace instances in a pool.
#[derive(Debug, Clone, Default)]
pub struct ProjectsService {
    store: ProjectStore,
}

fn allocate_worktree_id(project: &Project, spec: &WorktreeCreateSpec) -> WorktreeId {
    let seed = spec
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .or_else(|| (!spec.branch.trim().is_empty()).then_some(spec.branch.as_str()))
        .unwrap_or(spec.path.as_str());
    let base = WorktreeId::new(seed);
    if project.worktree(&base).is_none() {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = WorktreeId::new(format!("{seed}-{suffix}"));
        if project.worktree(&candidate).is_none() {
            return candidate;
        }
        suffix += 1;
    }
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

    /// Create a Worktree with the generic git strategy used by shell/tmux.
    ///
    /// The command is submitted to the target connection as argv; no platform
    /// frontend constructs `git worktree` commands and no Herdr path falls back
    /// to this strategy.
    pub fn create_generic_worktree(
        &mut self,
        catalog: &Catalog,
        connections: &mut ConnectionRegistry,
        project_id: &ProjectId,
        spec: &WorktreeCreateSpec,
    ) -> Result<WorktreeId> {
        let project = self
            .get_project(project_id)
            .ok_or_else(|| anyhow!("project 不存在: {project_id}"))?
            .clone();
        if project.target.runtime == TargetRuntime::Herdr {
            return Err(anyhow!("Herdr project 必须使用 native worktree strategy"));
        }

        let (transport_id, target) = match &project.target.transport {
            TargetTransport::Local => ("local", ""),
            TargetTransport::Ssh { name } => ("ssh", name.as_str()),
        };
        let connection = catalog.connect(connections, transport_id, target)?;
        let local = matches!(project.target.transport, TargetTransport::Local);
        let repo_root = if local {
            expand_config_value(&project.target.path)
        } else {
            project.target.path.clone()
        };
        let worktree_path = if local {
            expand_config_value(&spec.path)
        } else {
            spec.path.clone()
        };
        let base = spec.base.as_deref().map(|base| {
            if local {
                expand_config_value(base)
            } else {
                base.to_string()
            }
        });
        let argv = git_worktree_add_argv(
            &repo_root,
            &worktree_path,
            (!spec.branch.trim().is_empty()).then_some(spec.branch.as_str()),
            base.as_deref(),
        );
        let output = connection.exec_command(ChannelRequest::Exec {
            argv,
            cwd: None,
            env: Vec::new(),
            pty: None,
        })?;
        if output.status != 0 {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("git worktree add 失败: {}", detail.trim()));
        }

        let worktree_id = allocate_worktree_id(&project, spec);
        let worktree = Worktree::new(
            worktree_id.clone(),
            worktree_path,
            spec.branch.clone(),
            repo_root,
            true,
        );
        let mut updated = project;
        updated.add_worktree(worktree)?;
        self.store.upsert(updated)?;
        Ok(worktree_id)
    }

    /// Generic create followed by the normal Catalog open path. Completion is
    /// defined by the new Workspace entering the pool.
    pub async fn create_generic_worktree_and_open(
        &mut self,
        catalog: &Catalog,
        connections: &mut ConnectionRegistry,
        pool: &mut WorkspacePool,
        project_id: &ProjectId,
        spec: &WorktreeCreateSpec,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        let worktree_id = self.create_generic_worktree(catalog, connections, project_id, spec)?;
        self.open_worktree(
            catalog,
            connections,
            pool,
            project_id,
            &worktree_id,
            ResolveIntent::CreateIfMissing,
            template_override,
        )
        .await
    }

    /// Native Runtime worktree strategy (currently Herdr): the Runtime owns
    /// the checkout creation, while Projects owns the resulting provenance.
    pub async fn create_native_worktree_and_open(
        &mut self,
        catalog: &Catalog,
        connections: &mut ConnectionRegistry,
        pool: &mut WorkspacePool,
        project_id: &ProjectId,
        spec: &WorktreeCreateSpec,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        let project = self
            .get_project(project_id)
            .ok_or_else(|| anyhow!("project 不存在: {project_id}"))?
            .clone();
        if project.target.runtime != TargetRuntime::Herdr {
            return Err(anyhow!(
                "native worktree strategy 只适用于支持 WorktreeCreate 的 Runtime"
            ));
        }
        let source = self
            .open_project(
                catalog,
                connections,
                pool,
                project_id,
                ResolveIntent::AttachOnly,
                None,
            )
            .await?;
        let worktree_id = allocate_worktree_id(&project, spec);
        let template = template_override.or(project.template.clone());
        let workspace_id = catalog
            .create_native_worktree_with_pool(
                connections,
                pool,
                &source,
                spec,
                Some(project.worktree_provenance(&worktree_id)),
                template,
            )
            .await?;

        let mut updated = project;
        updated.add_worktree(Worktree::new(
            worktree_id,
            spec.path.clone(),
            spec.branch.clone(),
            updated.target.path.clone(),
            true,
        ))?;
        self.store.upsert(updated)?;
        Ok(workspace_id)
    }

    /// Select native vs generic strategy, then return only after the new
    /// Workspace has been inserted into the caller-owned product pool.
    pub async fn create_worktree(
        &mut self,
        catalog: &Catalog,
        connections: &mut ConnectionRegistry,
        pool: &mut WorkspacePool,
        project_id: &ProjectId,
        spec: &WorktreeCreateSpec,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        let runtime = self
            .get_project(project_id)
            .ok_or_else(|| anyhow!("project 不存在: {project_id}"))?
            .target
            .runtime;
        if runtime == TargetRuntime::Herdr {
            self.create_native_worktree_and_open(
                catalog,
                connections,
                pool,
                project_id,
                spec,
                template_override,
            )
            .await
        } else {
            self.create_generic_worktree_and_open(
                catalog,
                connections,
                pool,
                project_id,
                spec,
                template_override,
            )
            .await
        }
    }

    /// Open a Project through the single Catalog resolver path and caller-owned
    /// product pool.
    pub async fn open_project(
        &self,
        catalog: &Catalog,
        connections: &mut ConnectionRegistry,
        pool: &mut WorkspacePool,
        id: &ProjectId,
        intent: ResolveIntent,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        if self.get_project(id).is_none() {
            return Err(anyhow!("project 不存在: {id}"));
        }
        let request = OpenRequest {
            candidate: CandidateRef::Project {
                project_id: id.to_string(),
            },
            intent,
            template: template_override,
            activate: true,
        };
        let resolved = catalog.resolve_open_request(connections, &request, self.list_projects())?;
        let workspace_id = resolved.workspace_id();
        catalog.open_resolved(connections, pool, resolved).await?;
        Ok(workspace_id)
    }

    /// Open a registered Worktree through the same Catalog path as a Project.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_worktree(
        &mut self,
        catalog: &Catalog,
        connections: &mut ConnectionRegistry,
        pool: &mut WorkspacePool,
        project_id: &ProjectId,
        worktree_id: &super::WorktreeId,
        intent: ResolveIntent,
        template_override: Option<TemplateName>,
    ) -> Result<WorkspaceId> {
        if self
            .get_project(project_id)
            .and_then(|project| project.worktree(worktree_id))
            .is_none()
        {
            return Err(anyhow!("worktree 不存在: {project_id}/{worktree_id}"));
        }
        let request = OpenRequest {
            candidate: CandidateRef::Worktree {
                project_id: project_id.to_string(),
                worktree_id: worktree_id.to_string(),
            },
            intent,
            template: template_override,
            activate: true,
        };
        let resolved = catalog.resolve_open_request(connections, &request, self.list_projects())?;
        let workspace_id = resolved.workspace_id();
        catalog.open_resolved(connections, pool, resolved).await?;

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
    use crate::projects::Project;
    use crate::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};

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
