//! Frontend-owned Recent/Project QuickConnect store.

use super::model::{ProjectDocument, QuickConnect, RecentWorkspaceDescriptor, TargetConfig};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct QuickConnectStore {
    recents: Vec<RecentWorkspaceDescriptor>,
    projects: Vec<ProjectDocument>,
}

impl QuickConnectStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn from_project_documents(projects: &[ProjectDocument]) -> Self {
        Self {
            recents: Vec::new(),
            projects: projects.to_vec(),
        }
    }

    pub fn recent_targets(&self) -> Vec<TargetConfig> {
        self.recents
            .iter()
            .map(RecentWorkspaceDescriptor::to_target_config)
            .collect()
    }

    pub fn project_documents(&self) -> Vec<ProjectDocument> {
        self.projects.clone()
    }

    /// Return editable target projections only for valid persisted projects.
    ///
    /// The store keeps the original documents so an invalid/unknown project is
    /// not silently deleted when another project is edited or saved.
    pub fn project_targets(&self) -> Vec<TargetConfig> {
        self.projects
            .iter()
            .filter_map(|project| project.to_target().ok())
            .collect()
    }

    pub fn record_recent(&mut self, config: &TargetConfig) {
        let recent = RecentWorkspaceDescriptor::from_target(config);
        let id = recent.identity_key();
        self.recents.retain(|recent| recent.identity_key() != id);
        self.recents.insert(0, recent);
        if self.recents.len() > MAX_RECENT {
            self.recents.truncate(MAX_RECENT);
        }
    }

    pub fn replace_recents(&mut self, new_recents: &[RecentWorkspaceDescriptor]) {
        self.recents = new_recents.iter().take(MAX_RECENT).cloned().collect();
    }

    pub fn replace_all_recents(&mut self, new_recents: &[RecentWorkspaceDescriptor]) {
        self.recents = new_recents.to_vec();
    }

    pub fn project_id_for(&self, config: &TargetConfig) -> Option<String> {
        let id = QuickConnect::unique_id(config);
        self.projects.iter().find_map(|project| {
            let target = project.to_target().ok()?;
            (QuickConnect::unique_id(&target) == id).then(|| project.id.clone())
        })
    }

    pub fn upsert_project(&mut self, config: &TargetConfig) -> bool {
        let id = QuickConnect::unique_id(config);
        let mut document = ProjectDocument::from_target(config);
        if let Some(index) = self.projects.iter().position(|project| {
            project
                .to_target()
                .ok()
                .is_some_and(|target| QuickConnect::unique_id(&target) == id)
        }) {
            let previous = &self.projects[index];
            document.id.clone_from(&previous.id);
            document.template.clone_from(&previous.template);
            document.worktrees.clone_from(&previous.worktrees);
            document.command.clone_from(&previous.command);
            document.env.clone_from(&previous.env);
            document
                .runtime
                .options
                .clone_from(&previous.runtime.options);
            document
                .transport
                .options
                .clone_from(&previous.transport.options);
            self.projects[index] = document;
            false
        } else {
            self.projects.push(document);
            true
        }
    }

    pub fn remove_project(&mut self, config: &TargetConfig) {
        let id = QuickConnect::unique_id(config);
        self.projects.retain(|project| {
            project
                .to_target()
                .ok()
                .is_none_or(|target| QuickConnect::unique_id(&target) != id)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::quickconnect::model::{TargetRuntime, TargetTransport};

    fn config(name: &str) -> TargetConfig {
        TargetConfig::new(name, TargetRuntime::Tmux, TargetTransport::Local, "~/work")
    }

    #[test]
    fn store_round_trips_documents_without_rebuilding_them() {
        let mut document = ProjectDocument::from_target(&config("project"));
        document.template = Some("review".into());
        document.command = vec!["cargo".into(), "test".into()];
        document
            .worktrees
            .push(crate::linux::quickconnect::model::WorktreeDocument {
                id: "feature".into(),
                path: "~/work-feature".into(),
                branch: "feature".into(),
                repo_root: "~/work".into(),
                linked: true,
            });

        let store = QuickConnectStore::from_project_documents(&[document.clone()]);

        assert_eq!(store.project_documents(), vec![document]);
        assert_eq!(store.project_targets(), vec![config("project")]);
    }

    #[test]
    fn editing_a_project_preserves_persisted_metadata() {
        let mut original = config("project");
        original.session = Some("stable-session".into());
        let mut document = ProjectDocument::from_target(&original);
        document.id = "stable-project-id".into();
        document.template = Some("review".into());
        document.command = vec!["cargo".into(), "test".into()];
        let mut store = QuickConnectStore::from_project_documents(&[document]);

        let mut edited = config("renamed");
        edited.session = Some("stable-session".into());
        edited.path = "~/renamed".into();
        assert!(!store.upsert_project(&edited));

        let saved_projects = store.project_documents();
        let [saved] = saved_projects.as_slice() else {
            panic!("one project should remain");
        };
        assert_eq!(saved.id, "stable-project-id");
        assert_eq!(saved.name, "renamed");
        assert_eq!(saved.path, "~/renamed");
        assert_eq!(saved.template.as_deref(), Some("review"));
        assert_eq!(saved.command, vec!["cargo", "test"]);
    }

    #[test]
    fn invalid_project_document_survives_store_round_trip_but_is_not_a_row() {
        let invalid = ProjectDocument {
            id: "broken".into(),
            name: "broken".into(),
            path: "~/broken".into(),
            runtime: crate::linux::quickconnect::model::ProjectRuntime {
                id: "unknown-runtime".into(),
                options: Default::default(),
                session: None,
                socket: None,
                workspace_id: None,
            },
            transport: crate::linux::quickconnect::model::ProjectTransport {
                id: "local".into(),
                target: String::new(),
                options: Default::default(),
            },
            template: None,
            worktrees: Vec::new(),
            command: Vec::new(),
            env: Default::default(),
        };
        let store = QuickConnectStore::from_project_documents(std::slice::from_ref(&invalid));

        assert_eq!(store.project_documents(), vec![invalid]);
        assert!(store.project_targets().is_empty());
    }
}
