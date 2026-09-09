//! Frontend-owned Recent/Project QuickConnect store.

use std::collections::HashMap;

use super::model::{ProjectDocument, QuickConnect, TargetConfig};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct QuickConnectStore {
    pub recents: Vec<TargetConfig>,
    pub projects: Vec<TargetConfig>,
    project_ids: HashMap<String, String>,
}

impl QuickConnectStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn from_project_documents(projects: &[ProjectDocument]) -> Self {
        let mut store = Self::in_memory();
        for project in projects {
            let Ok(target) = project.to_target() else {
                continue;
            };
            store
                .project_ids
                .insert(QuickConnect::unique_id(&target), project.id.clone());
            store.projects.push(target);
        }
        store
    }

    pub fn project_documents(&self) -> Vec<ProjectDocument> {
        self.projects
            .iter()
            .map(|config| {
                let mut project = ProjectDocument::from_target(config);
                if let Some(project_id) = self.project_ids.get(&QuickConnect::unique_id(config)) {
                    project.id.clone_from(project_id);
                }
                project
            })
            .collect()
    }

    pub fn record_recent(&mut self, config: &TargetConfig) {
        let id = QuickConnect::unique_id(config);
        self.recents
            .retain(|recent| QuickConnect::unique_id(recent) != id);
        self.recents.insert(0, config.clone());
        if self.recents.len() > MAX_RECENT {
            self.recents.truncate(MAX_RECENT);
        }
    }

    pub fn replace_recents(&mut self, new_recents: &[TargetConfig]) {
        self.recents = new_recents.iter().take(MAX_RECENT).cloned().collect();
    }

    pub fn replace_all_recents(&mut self, new_recents: &[TargetConfig]) {
        self.recents = new_recents.to_vec();
    }

    pub fn project_id_for(&self, config: &TargetConfig) -> Option<String> {
        self.project_ids
            .get(&QuickConnect::unique_id(config))
            .cloned()
    }

    pub fn upsert_project(&mut self, config: &TargetConfig) -> bool {
        let id = QuickConnect::unique_id(config);
        let project_id = self
            .project_ids
            .get(&id)
            .cloned()
            .unwrap_or_else(|| ProjectDocument::from_target(config).id);
        self.project_ids.insert(id.clone(), project_id);
        if let Some(index) = self
            .projects
            .iter()
            .position(|project| QuickConnect::unique_id(project) == id)
        {
            self.projects[index] = config.clone();
            false
        } else {
            self.projects.push(config.clone());
            true
        }
    }

    pub fn remove_project(&mut self, config: &TargetConfig) {
        let id = QuickConnect::unique_id(config);
        self.projects
            .retain(|project| QuickConnect::unique_id(project) != id);
        self.project_ids.remove(&id);
    }
}
