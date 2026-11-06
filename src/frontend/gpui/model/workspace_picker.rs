//! Swift `UnifiedPanelController` 的 Open 导航与 Project 持久化规则。
//! 只维护产品输入和请求 generation，不读文件、不直接访问 Core。

use super::directory::DirectorySuggestionController;
use super::quick_connect::{QuickConnect, TargetConfig};
use super::target::{TargetRuntime, TargetTransport};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDraft {
    pub transport: TargetTransport,
    pub path: String,
    pub runtime: TargetRuntime,
    pub name: String,
}

impl CreateDraft {
    pub fn new(transport: TargetTransport, path: &str) -> Self {
        let path = super::directory::DirectoryPathModel::resolved_path(path);
        Self {
            name: QuickConnect::default_name(&path),
            path,
            transport,
            runtime: TargetRuntime::Herdr,
        }
    }

    pub fn config(&self) -> Result<TargetConfig, &'static str> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err("Enter a workspace name");
        }
        if !matches!(self.runtime, TargetRuntime::Herdr | TargetRuntime::Tmux) {
            return Err("Choose Herdr or tmux");
        }
        // 本地路径必须先展开 `~`：Core 的 list_dir 只认字面 "~"，
        // tmux/herdr 的 -c 也不会展开 `~/x`，不展开就会「找不到路径」。
        let path = match self.transport {
            super::target::TargetTransport::Local => super::directory::expand_home(&self.path),
            super::target::TargetTransport::Ssh { .. } => self.path.clone(),
        };
        Ok(TargetConfig::new(
            name,
            self.runtime,
            self.transport.clone(),
            &path,
        ))
    }
}

/// Stage 改变时递增 generation；同一 host 的旧请求也不能覆盖新一轮导航。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PickerStage {
    #[default]
    Root,
    Hosts,
    Sessions(TargetTransport),
    Path(TargetTransport),
    Confirm(CreateDraft),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerRequest {
    pub generation: u64,
    pub stage: PickerStage,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspacePicker {
    pub stage: PickerStage,
    pub path: DirectorySuggestionController,
    pub projects: ProjectList,
    pub aliases: Vec<String>,
    pub sessions: Vec<super::existing::ExistingRuntime>,
    pub loading: bool,
    pub error: Option<String>,
    generation: u64,
}

impl WorkspacePicker {
    pub fn navigate(&mut self, stage: PickerStage) -> PickerRequest {
        self.generation = self.generation.wrapping_add(1);
        self.stage = stage;
        self.error = None;
        self.loading = false;
        self.request()
    }
    pub fn request(&self) -> PickerRequest {
        PickerRequest {
            generation: self.generation,
            stage: self.stage.clone(),
        }
    }
    pub fn accepts(&self, request: &PickerRequest) -> bool {
        self.request() == *request
    }
    pub fn enter_sessions(&mut self, transport: TargetTransport) -> PickerRequest {
        self.sessions.clear();
        self.navigate(PickerStage::Sessions(transport))
    }
    pub fn back(&mut self) {
        let stage = match &self.stage {
            PickerStage::Root | PickerStage::Hosts => PickerStage::Root,
            PickerStage::Sessions(_) => PickerStage::Hosts,
            PickerStage::Path(host) => PickerStage::Sessions(host.clone()),
            PickerStage::Confirm(draft) => PickerStage::Path(draft.transport.clone()),
        };
        self.navigate(stage);
    }
}

/// 保留整个 ProjectDocument，只改选中项，不能把 template/env/worktrees 等字段丢掉。
#[derive(Debug, Clone, Default)]
pub struct ProjectList(Vec<Value>);

impl ProjectList {
    pub fn new(value: &Value) -> Self {
        Self(value.as_array().cloned().unwrap_or_default())
    }
    pub fn value(&self) -> Value {
        Value::Array(self.0.clone())
    }
    pub fn configs(&self) -> Vec<TargetConfig> {
        self.0.iter().filter_map(config_from_project).collect()
    }
    pub fn contains(&self, config: &TargetConfig) -> bool {
        self.configs()
            .iter()
            .any(|saved| same_target(saved, config))
    }
    pub fn remove(&mut self, config: &TargetConfig) {
        self.0.retain(|value| {
            config_from_project(value).is_none_or(|saved| !same_target(&saved, config))
        });
    }
    pub fn upsert(&mut self, config: &TargetConfig) {
        let position = self.0.iter().position(|value| {
            config_from_project(value).is_some_and(|saved| same_target(&saved, config))
        });
        let mut value = position
            .map(|index| self.0[index].clone())
            .unwrap_or_else(|| json!({}));
        let id = value["id"].as_str().map(str::to_string).unwrap_or_else(|| {
            let base = project_slug(&config.name);
            let used: std::collections::HashSet<_> = self
                .0
                .iter()
                .filter_map(|item| item["id"].as_str())
                .map(str::to_lowercase)
                .collect();
            let mut id = config.persisted_id.clone().unwrap_or_else(|| base.clone());
            if used.contains(&id.to_lowercase()) {
                id = format!("{base}-{}", config.runtime.label().to_lowercase());
            }
            let mut suffix = 2;
            while used.contains(&id.to_lowercase()) {
                id = format!("{base}-{suffix}");
                suffix += 1;
            }
            id
        });
        value["id"] = json!(id);
        value["name"] = json!(config.name);
        value["path"] = json!(config.path);
        let mut runtime = value
            .get("runtime")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        runtime["id"] = json!(config.runtime.label().to_lowercase());
        for (key, field) in [
            ("session", &config.session),
            ("socket", &config.socket),
            ("workspace_id", &config.workspace_id),
        ] {
            if let Some(field) = field {
                runtime[key] = json!(field);
            }
        }
        value["runtime"] = runtime;
        value["transport"] = match &config.transport {
            TargetTransport::Local => json!({"id":"local"}),
            TargetTransport::Ssh { name } => json!({"id":"ssh", "target":name}),
        };
        if let Some(position) = position {
            self.0[position] = value;
        } else {
            self.0.push(value);
        }
    }
}

pub fn same_target(left: &TargetConfig, right: &TargetConfig) -> bool {
    if left.persisted_id.is_some() && left.persisted_id == right.persisted_id {
        return true;
    }
    QuickConnect::unique_id(left) == QuickConnect::unique_id(right)
}

pub fn config_from_project(value: &Value) -> Option<TargetConfig> {
    let runtime = &value["runtime"];
    let transport = &value["transport"];
    Some(TargetConfig {
        name: value["name"].as_str()?.to_string(),
        path: value["path"].as_str().unwrap_or_default().to_string(),
        runtime: TargetRuntime::parse(runtime["id"].as_str()?)?,
        transport: if transport["id"] == "ssh" {
            TargetTransport::Ssh {
                name: transport["target"].as_str().unwrap_or_default().to_string(),
            }
        } else {
            TargetTransport::Local
        },
        session: runtime["session"].as_str().map(str::to_string),
        socket: runtime["socket"].as_str().map(str::to_string),
        workspace_id: runtime["workspace_id"].as_str().map(str::to_string),
        persisted_id: value["id"].as_str().map(str::to_string),
    })
}

fn project_slug(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "project".into()
    } else {
        slug.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::target::{TargetRuntime, TargetTransport};
    use serde_json::json;

    /// 对照 `UnifiedPanelLauncherE2ETests.testOpenNewPathCompletesDirectoriesAndConfirms`，
    /// 最后增加用户要求的 runtime/name 确认；到确认前不能创建。
    #[test]
    fn path_confirmation_defaults_to_herdr_and_the_last_directory_name() {
        let draft = CreateDraft::new(
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/Developer/muxterm/",
        );
        assert_eq!(draft.runtime, TargetRuntime::Herdr);
        assert_eq!(draft.name, "muxterm");
        let config = draft.config().unwrap();
        assert_eq!(config.path, "~/Developer/muxterm");
        assert_eq!(config.session, None, "Herdr named session 不能由展示名反推");
    }

    /// 本地路径必须落成绝对路径：Core 只展开字面 `~`，`~/x` 会「找不到路径」。
    /// SSH 保留 `~`，远端 shell 自己展开。
    #[test]
    fn local_confirmation_expands_home_but_ssh_keeps_the_tilde() {
        let home = crate::frontend::gpui::model::directory::home_directory().expect("HOME");
        let mut draft = CreateDraft::new(TargetTransport::Local, "~/Developer/muxterm");
        draft.runtime = TargetRuntime::Tmux;
        assert_eq!(
            draft.config().unwrap().path,
            format!("{home}/Developer/muxterm")
        );

        let remote = CreateDraft::new(
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/dev/app",
        );
        assert_eq!(remote.config().unwrap().path, "~/dev/app");
    }

    #[test]
    fn confirmation_allows_tmux_and_an_edited_name_but_rejects_empty_names() {
        let mut draft = CreateDraft::new(TargetTransport::Local, "/srv/work");
        draft.runtime = TargetRuntime::Tmux;
        draft.name = "custom-name".into();
        assert_eq!(draft.config().unwrap().name, "custom-name");
        draft.name = "  ".into();
        assert!(draft.config().is_err());
    }

    /// 对照 `QuickConnectStore.projectJSON` / `testSavedSessionRowShowsProjectBadge`。
    #[test]
    fn saving_a_discovered_herdr_keeps_the_complete_identity() {
        let config = TargetConfig {
            name: "muxterm".into(),
            runtime: TargetRuntime::Herdr,
            transport: TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            path: "~/dev/muxterm".into(),
            session: Some("default".into()),
            socket: Some("/run/herdr.sock".into()),
            workspace_id: Some("w2".into()),
            persisted_id: None,
        };
        let mut projects = ProjectList::new(&json!([]));
        projects.upsert(&config);
        let roundtrip = projects.configs().remove(0);
        assert_eq!(roundtrip.workspace_id.as_deref(), Some("w2"));
        assert_eq!(roundtrip.session, config.session);
        assert_eq!(roundtrip.socket, config.socket);
        assert_eq!(roundtrip.transport, config.transport);
        assert_eq!(roundtrip.persisted_id.as_deref(), Some("muxterm"));
        projects.upsert(&config);
        assert_eq!(projects.configs().len(), 1);
    }

    #[test]
    fn saving_does_not_erase_other_project_fields_or_collide_ids() {
        let original = json!([{"id":"demo", "name":"old", "path":"/old", "runtime":{"id":"tmux","session":"old"}, "transport":{"id":"local"}, "template":"work", "env":{"X":"1"}}]);
        let mut projects = ProjectList::new(&original);
        let config = TargetConfig::new("demo", TargetRuntime::Tmux, TargetTransport::Local, "/new");
        projects.upsert(&config);
        assert_eq!(projects.value()[0], original[0]);
        assert_eq!(
            projects.configs()[1].persisted_id.as_deref(),
            Some("demo-tmux")
        );
    }

    /// 对照 `testLateDiscoveryCannotReplaceWorkspaceRootAfterBack`。
    #[test]
    fn navigation_invalidates_pending_requests_even_when_returning_to_the_same_host() {
        let mut picker = WorkspacePicker::default();
        let host = TargetTransport::Local;
        let old = picker.enter_sessions(host.clone());
        picker.back();
        let current = picker.enter_sessions(host);
        assert!(!picker.accepts(&old));
        assert!(picker.accepts(&current));
    }
}
