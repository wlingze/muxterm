//! QuickConnect 数据（Recent + Project）的列表管理（纯逻辑）。
//!
//! - Recent：手动记录的最近连接最多 20 条，去重，最近的在最前；
//!   recents 不落盘，由连接池在运行时派生。
//! - Project：用户配置的预设目标，保存在统一 `config.toml` 的
//!   `[[projects]]`。本模块不解析或序列化 TOML；增删改通过
//!   `SettingsService` 事务写 Core 文档。

use std::path::PathBuf;

use super::model::{QuickConnect, TargetConfig};

/// QuickConnect 数据存储。
#[derive(Debug, Clone, Default)]
pub struct QuickConnectStore {
    pub recents: Vec<TargetConfig>,
    pub projects: Vec<TargetConfig>,
    /// 统一 config.toml 路径（`new_unified` 设置）；None 表示纯内存。
    config_path: Option<PathBuf>,
}

/// 最近连接记录条数上限。
pub const MAX_RECENT: usize = 20;

impl QuickConnectStore {
    /// 纯内存 store（测试 / 不落盘的调用方）。
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Create a store backed by Core's unified `config.toml` Project array.
    /// The legacy QuickConnect file is imported by SettingsService when present.
    pub fn new_unified(config_path: Option<PathBuf>) -> Self {
        let Some(path) = config_path else {
            return Self::default();
        };
        let mut store = QuickConnectStore {
            config_path: Some(path.clone()),
            ..Default::default()
        };
        match crate::core::config_service::SettingsService::open(&path) {
            Ok(mut service) => {
                if let Err(error) = service.migrate_legacy_quickconnect() {
                    tracing::warn!(
                        target = "muxterm::config",
                        "QuickConnect 迁移未完成: {error}"
                    );
                }
                store.projects = service
                    .document()
                    .projects
                    .iter()
                    .filter_map(|project| project.to_target().ok())
                    .collect();
            }
            Err(error) => tracing::warn!(
                target = "muxterm::config",
                "读取统一 Project 配置失败: {error}"
            ),
        }
        store
    }

    /// 记录一次连接：放进 recents 最前并按 attach identity 去重。仅内存态。
    pub fn record_recent(&mut self, config: &TargetConfig) {
        let id = QuickConnect::unique_id(config);
        self.recents.retain(|r| QuickConnect::unique_id(r) != id);
        self.recents.insert(0, config.clone());
        if self.recents.len() > MAX_RECENT {
            self.recents.truncate(MAX_RECENT);
        }
    }

    /// 用连接池派生的 recents 替换内存态（不触发落盘）。
    pub fn replace_recents(&mut self, new_recents: &[TargetConfig]) {
        self.recents = new_recents.iter().take(MAX_RECENT).cloned().collect();
    }

    /// 用连接池的完整 Workspace 快照替换内存态（不触发落盘）。
    ///
    /// 连接池的容量是软提醒阈值，可能合法地超过 20；快速面板搜索必须
    /// 能命中第 21 个及之后的 Workspace，因此这个入口不套用手动 Recent
    /// 历史的 `MAX_RECENT` 限制。空面板仍由 UI 只展示前几条。
    pub fn replace_all_recents(&mut self, new_recents: &[TargetConfig]) {
        self.recents = new_recents.to_vec();
    }

    /// 新增或更新一个 project（按 attach identity 匹配）。返回是否新增。
    pub fn upsert_project(&mut self, config: &TargetConfig) -> bool {
        let id = QuickConnect::unique_id(config);
        let added = if let Some(idx) = self
            .projects
            .iter()
            .position(|p| QuickConnect::unique_id(p) == id)
        {
            self.projects[idx] = config.clone();
            false
        } else {
            self.projects.push(config.clone());
            true
        };
        self.persist();
        added
    }

    /// 删除 project（按 attach identity）。
    pub fn remove_project(&mut self, config: &TargetConfig) {
        let id = QuickConnect::unique_id(config);
        self.projects.retain(|p| QuickConnect::unique_id(p) != id);
        self.persist();
    }

    /// 通过 Core SettingsService 事务把 projects 写回统一 config.toml。
    /// 失败只记日志；内存列表保留，不覆盖用户文件。
    fn persist(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let Ok(mut service) = crate::core::config_service::SettingsService::open(path) else {
            return;
        };
        let transaction = service.begin();
        let value = serde_json::Value::Array(
            self.projects
                .iter()
                .map(crate::core::config_service::ProjectDocument::from_target)
                .filter_map(|project| serde_json::to_value(project).ok())
                .collect(),
        );
        let operation = crate::core::config_service::JsonPatchOperation {
            op: "replace".into(),
            path: "/projects".into(),
            value: Some(value),
        };
        if let Err(error) = service
            .patch(&transaction, &[operation])
            .and_then(|_| service.commit(&transaction).map(|_| ()))
        {
            tracing::warn!(
                target = "muxterm::config",
                "写入统一 Project 配置失败: {error}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::quickconnect::model::{TargetRuntime, TargetTransport};

    fn cfg(
        name: &str,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: &str,
    ) -> TargetConfig {
        TargetConfig::new(name, runtime, transport, path)
    }

    #[test]
    fn record_recent_dedupes_and_limits() {
        let mut store = QuickConnectStore::in_memory();
        for i in 0..25 {
            store.record_recent(&cfg(
                &format!("p{i}"),
                TargetRuntime::Shell,
                TargetTransport::Local,
                &format!("~/x/{i}"),
            ));
        }
        assert_eq!(store.recents.len(), MAX_RECENT);
        assert_eq!(store.recents[0].name, "p24");
        store.record_recent(&cfg(
            "p24",
            TargetRuntime::Shell,
            TargetTransport::Local,
            "~/x/24",
        ));
        assert_eq!(store.recents.len(), MAX_RECENT);
        assert_eq!(store.recents[0].name, "p24");
    }

    #[test]
    fn replace_all_recents_keeps_workspaces_beyond_history_limit() {
        let mut store = QuickConnectStore::in_memory();
        let all: Vec<TargetConfig> = (0..(MAX_RECENT + 5))
            .map(|i| {
                cfg(
                    &format!("p{i}"),
                    TargetRuntime::Shell,
                    TargetTransport::Local,
                    &format!("~/x/{i}"),
                )
            })
            .collect();

        store.replace_all_recents(&all);

        assert_eq!(store.recents.len(), MAX_RECENT + 5);
        assert_eq!(
            store.recents.last().map(|config| config.name.as_str()),
            Some("p24")
        );
    }

    #[test]
    fn upsert_updates_existing() {
        let mut store = QuickConnectStore::in_memory();
        let a = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/a");
        assert!(store.upsert_project(&a));
        let b = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/b");
        assert!(!store.upsert_project(&b));
        assert_eq!(store.projects.len(), 1);
        assert_eq!(store.projects[0].path, "~/b");
    }

    #[test]
    fn unified_upsert_writes_config_toml_projects() {
        let dir = std::env::temp_dir().join(format!("muxterm-qc-unified-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "config_version = 1\n").unwrap();
        let mut store = QuickConnectStore::new_unified(Some(path.clone()));
        store.upsert_project(&cfg(
            "muxterm",
            TargetRuntime::Shell,
            TargetTransport::Local,
            "~/Developer/self/muxterm",
        ));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("name = \"muxterm\""), "{raw}");
        let service = crate::core::config_service::SettingsService::open(&path).unwrap();
        assert_eq!(service.document().projects.len(), 1);
        assert_eq!(service.document().projects[0].name, "muxterm");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_project_persists_through_core() {
        let dir = std::env::temp_dir().join(format!("muxterm-qc-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "config_version = 1\n").unwrap();
        let mut store = QuickConnectStore::new_unified(Some(path.clone()));
        let project = cfg("p", TargetRuntime::Tmux, TargetTransport::Local, "~/p");
        store.upsert_project(&project);
        assert_eq!(store.projects.len(), 1);
        store.remove_project(&project);
        assert!(store.projects.is_empty());
        let service = crate::core::config_service::SettingsService::open(&path).unwrap();
        assert!(service.document().projects.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
