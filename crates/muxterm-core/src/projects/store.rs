//! Persistent Project collection backed by Core SettingsService transactions.

use anyhow::{anyhow, Result};
use std::path::PathBuf;

use super::Project;
use crate::config::{JsonPatchOperation, SettingsService};

/// Core-owned Project persistence. It does not own live Runtime instances.
#[derive(Debug, Clone, Default)]
pub struct ProjectStore {
    projects: Vec<Project>,
    config_path: Option<PathBuf>,
}

impl ProjectStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Build the project projection from the already-loaded unified settings.
    ///
    /// Keeping the source `SettingsService` shared with the handle avoids a
    /// second config parse and makes migrations performed during startup
    /// visible to the Projects domain immediately.
    pub fn from_settings(settings: &SettingsService) -> Result<Self> {
        let projects = settings
            .document()
            .projects
            .iter()
            .map(Project::from_document)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            projects,
            config_path: Some(settings.path().to_path_buf()),
        })
    }

    pub fn new_unified(config_path: Option<PathBuf>) -> Result<Self> {
        let Some(path) = config_path else {
            return Ok(Self::in_memory());
        };
        let mut settings = SettingsService::open(&path)?;
        settings.migrate_legacy_quickconnect()?;
        Self::from_settings(&settings)
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn get(&self, id: &super::ProjectId) -> Option<&Project> {
        self.projects.iter().find(|project| &project.id == id)
    }

    pub fn get_mut(&mut self, id: &super::ProjectId) -> Option<&mut Project> {
        self.projects.iter_mut().find(|project| &project.id == id)
    }

    pub fn upsert(&mut self, project: Project) -> Result<bool> {
        validate_project(&project)?;
        let mut next = self.projects.clone();
        let added = if let Some(index) = next.iter().position(|item| item.id == project.id) {
            next[index] = project;
            false
        } else {
            next.push(project);
            true
        };
        self.persist_projects(&next)?;
        self.projects = next;
        Ok(added)
    }

    pub fn remove(&mut self, id: &super::ProjectId) -> Result<Option<Project>> {
        let Some(index) = self.projects.iter().position(|project| &project.id == id) else {
            return Ok(None);
        };
        let mut next = self.projects.clone();
        let removed = next.remove(index);
        self.persist_projects(&next)?;
        self.projects = next;
        Ok(Some(removed))
    }

    fn persist_projects(&self, projects: &[Project]) -> Result<()> {
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        let mut settings = SettingsService::open(path)?;
        let transaction = settings.begin();
        let value = serde_json::Value::Array(
            projects
                .iter()
                .map(|project| serde_json::to_value(project.to_document()))
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
        let operation = JsonPatchOperation {
            op: "replace".into(),
            path: "/projects".into(),
            value: Some(value),
        };
        settings.patch(&transaction, &[operation])?;
        settings.commit(&transaction)?;
        Ok(())
    }
}

fn validate_project(project: &Project) -> Result<()> {
    if project.id.as_str().trim().is_empty() || project.name.trim().is_empty() {
        return Err(anyhow!("project id 和 name 不能为空"));
    }
    if project.target.path.trim().is_empty() {
        return Err(anyhow!("project {} 的 path 不能为空", project.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigDocument;
    use crate::config::SettingsService;
    use crate::projects::Project;
    use crate::quickconnect::model::{TargetConfig, TargetRuntime, TargetTransport};
    use std::fs;

    fn project(id: &str, path: &str) -> Project {
        Project::new(
            id,
            id,
            TargetConfig::new(id, TargetRuntime::Shell, TargetTransport::Local, path),
        )
    }

    #[test]
    fn in_memory_store_upserts_and_removes_typed_projects() {
        let mut store = ProjectStore::in_memory();
        assert!(store.upsert(project("a", "/a")).unwrap());
        assert!(!store.upsert(project("a", "/b")).unwrap());
        assert_eq!(store.projects().len(), 1);
        assert_eq!(store.get(&"a".into()).unwrap().target.path, "/b");

        let removed = store.remove(&"a".into()).unwrap().unwrap();
        assert_eq!(removed.id.as_str(), "a");
        assert!(store.projects().is_empty());
    }

    #[test]
    fn store_rejects_invalid_project_before_persisting() {
        let mut store = ProjectStore::in_memory();
        let error = store
            .upsert(project("", "/invalid"))
            .expect_err("empty project id must be rejected");
        assert!(error.to_string().contains("不能为空"));
        assert!(store.projects().is_empty());
    }

    #[test]
    fn store_projects_from_the_loaded_settings_document() {
        let path =
            std::env::temp_dir().join(format!("muxterm-project-store-{}.toml", std::process::id()));
        let mut document = ConfigDocument::default();
        document
            .projects
            .push(project("loaded", "/loaded").to_document());
        fs::write(&path, document.to_toml().unwrap()).unwrap();

        let settings = SettingsService::open(&path).unwrap();
        let store = ProjectStore::from_settings(&settings).unwrap();

        assert_eq!(store.projects().len(), 1);
        assert_eq!(store.projects()[0].id.as_str(), "loaded");
        assert_eq!(store.projects()[0].target.path, "/loaded");
        fs::remove_file(path).unwrap();
    }
}
