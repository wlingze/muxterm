use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::config_service::action_catalog::action_catalog;
use crate::core::config_service::migration::import_legacy_projects;
use crate::core::config_service::schema::ConfigDocument;
use crate::core::config_service::storage::{
    atomic_write, preserve_toml_metadata, revision_for, ConfigRevision, ConfigSnapshot,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonPatchOperation {
    pub op: String,
    pub path: String,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftResult {
    pub transaction: String,
    pub values: Value,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigEvent {
    PreviewChanged {
        transaction: String,
        values: Value,
    },
    Committed {
        revision: ConfigRevision,
        values: Value,
    },
    Reloaded {
        revision: ConfigRevision,
        values: Value,
    },
    DiagnosticsChanged {
        diagnostics: Vec<String>,
    },
    RolledBack {
        transaction: String,
    },
}

#[derive(Debug, Clone)]
struct TransactionState {
    base_revision: ConfigRevision,
    baseline: Value,
    draft: Value,
}

pub struct SettingsService {
    path: PathBuf,
    raw: String,
    document: ConfigDocument,
    revision: ConfigRevision,
    transactions: BTreeMap<String, TransactionState>,
    next_transaction: u64,
    events: VecDeque<ConfigEvent>,
}

impl SettingsService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let raw = if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("读取配置失败: {}", path.display()))?
        } else {
            ConfigDocument::default().to_toml()?
        };
        let document = ConfigDocument::from_toml(&raw)?;
        let revision = revision_for(&raw);
        Ok(Self {
            path,
            raw,
            document,
            revision,
            transactions: BTreeMap::new(),
            next_transaction: 1,
            events: VecDeque::new(),
        })
    }

    pub fn default_user() -> Result<Self> {
        let path = crate::core::config::Config::user_config_path()
            .ok_or_else(|| anyhow!("无法确定用户配置路径"))?;
        Self::open(path)
    }

    /// Build an in-memory default service when a frontend still needs to start
    /// and the on-disk configuration is invalid. The caller must surface the
    /// original parse error separately; this fallback never overwrites it.
    pub fn in_memory_default(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let document = ConfigDocument::default();
        let raw = document
            .to_toml()
            .unwrap_or_else(|_| String::from("config_version = 1\n"));
        Self {
            path,
            revision: revision_for(&raw),
            raw,
            document,
            transactions: BTreeMap::new(),
            next_transaction: 1,
            events: VecDeque::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn document(&self) -> &ConfigDocument {
        &self.document
    }

    pub fn snapshot(&self) -> ConfigSnapshot {
        let values = serde_json::to_value(&self.document).unwrap_or(Value::Null);
        let defaults = serde_json::to_value(ConfigDocument::default()).unwrap_or(Value::Null);
        let raw = toml::from_str::<toml::Value>(&self.raw)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or_else(|| Value::Object(Map::new()));
        ConfigSnapshot {
            revision: self.revision.clone(),
            raw,
            values,
            defaults,
            schema: ConfigDocument::schema_json(),
            manifest: ConfigDocument::manifest_json(),
            action_catalog: serde_json::to_value(action_catalog())
                .unwrap_or(Value::Array(Vec::new())),
        }
    }

    pub fn begin(&mut self) -> String {
        let id = format!("config-{}", self.next_transaction);
        self.next_transaction = self.next_transaction.saturating_add(1);
        let baseline = serde_json::to_value(&self.document).unwrap_or(Value::Null);
        self.transactions.insert(
            id.clone(),
            TransactionState {
                base_revision: self.revision.clone(),
                baseline: baseline.clone(),
                draft: baseline,
            },
        );
        id
    }

    pub fn patch(
        &mut self,
        transaction: &str,
        operations: &[JsonPatchOperation],
    ) -> Result<DraftResult> {
        let state = self
            .transactions
            .get(transaction)
            .cloned()
            .ok_or_else(|| anyhow!("未知配置事务: {transaction}"))?;
        let mut draft = state.draft;
        for operation in operations {
            apply_patch_operation(&mut draft, operation)?;
        }
        let document: ConfigDocument =
            serde_json::from_value(draft.clone()).context("草稿不符合 ConfigDocument")?;
        document.validate()?;
        if let Some(current) = self.transactions.get_mut(transaction) {
            current.draft = draft.clone();
        }
        self.events.push_back(ConfigEvent::PreviewChanged {
            transaction: transaction.to_string(),
            values: draft.clone(),
        });
        Ok(DraftResult {
            transaction: transaction.to_string(),
            values: draft,
            diagnostics: Vec::new(),
        })
    }

    pub fn cancel(&mut self, transaction: &str) -> Result<()> {
        if self.transactions.remove(transaction).is_none() {
            return Err(anyhow!("未知配置事务: {transaction}"));
        }
        self.events.push_back(ConfigEvent::RolledBack {
            transaction: transaction.to_string(),
        });
        Ok(())
    }

    pub fn commit(&mut self, transaction: &str) -> Result<ConfigRevision> {
        let state = self
            .transactions
            .remove(transaction)
            .ok_or_else(|| anyhow!("未知配置事务: {transaction}"))?;
        let disk_raw = if self.path.exists() {
            fs::read_to_string(&self.path)
                .with_context(|| format!("读取配置失败: {}", self.path.display()))?
        } else {
            self.raw.clone()
        };
        let disk_document = ConfigDocument::from_toml(&disk_raw)?;
        let disk_revision = revision_for(&disk_raw);
        let candidate = if disk_revision == state.base_revision {
            state.draft
        } else {
            merge_three_way(
                &state.baseline,
                &state.draft,
                &serde_json::to_value(disk_document)?,
            )?
        };
        let document: ConfigDocument =
            serde_json::from_value(candidate).context("合并后的配置无效")?;
        document.validate()?;
        let serialized = document.to_toml()?;
        let raw = preserve_toml_metadata(&self.raw, &serialized).unwrap_or(serialized);
        atomic_write(&self.path, &raw)?;
        self.raw = raw;
        self.document = document;
        self.revision = revision_for(&self.raw);
        self.events.push_back(ConfigEvent::Committed {
            revision: self.revision.clone(),
            values: serde_json::to_value(&self.document)?,
        });
        Ok(self.revision.clone())
    }

    pub fn reload(&mut self) -> Result<ConfigRevision> {
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("读取配置失败: {}", self.path.display()))?;
        let document = ConfigDocument::from_toml(&raw)?;
        self.raw = raw;
        self.document = document;
        self.revision = revision_for(&self.raw);
        self.events.push_back(ConfigEvent::Reloaded {
            revision: self.revision.clone(),
            values: serde_json::to_value(&self.document)?,
        });
        Ok(self.revision.clone())
    }

    pub fn drain_events(&mut self) -> Vec<ConfigEvent> {
        self.events.drain(..).collect()
    }

    /// Import legacy `quickconnect.toml` projects into the unified document.
    ///
    /// The legacy file is deliberately retained so a failed or partial migration
    /// is recoverable. Calling this method is idempotent: project IDs already
    /// present in `config.toml` win over legacy entries.
    pub fn migrate_legacy_quickconnect(&mut self) -> Result<usize> {
        let Some(parent) = self.path.parent() else {
            return Ok(0);
        };
        let legacy_path = parent.join("quickconnect.toml");
        if !legacy_path.exists() {
            return Ok(0);
        }
        let raw = fs::read_to_string(&legacy_path)
            .with_context(|| format!("读取旧 QuickConnect 配置失败: {}", legacy_path.display()))?;
        let imported = import_legacy_projects(&raw)?;
        let mut added = 0;
        for project in imported {
            if self
                .document
                .projects
                .iter()
                .any(|item| item.id == project.id)
            {
                continue;
            }
            self.document.projects.push(project);
            added += 1;
        }
        if added > 0 {
            self.document.validate()?;
            let raw = self.document.to_toml()?;
            atomic_write(&self.path, &raw)?;
            self.raw = raw;
            self.revision = revision_for(&self.raw);
        }
        Ok(added)
    }

    /// Reload only when the on-disk revision differs from the current one.
    /// Frontends can call this from their platform file-monitor/debounce loop.
    /// Import legacy Linux `preferences.toml` overrides into the unified
    /// document. Theme, statusbar mode, and font size are migrated once; the
    /// legacy file is retained as a recoverable backup.
    pub fn migrate_legacy_linux_preferences(&mut self) -> Result<usize> {
        let Some(parent) = self.path.parent() else {
            return Ok(0);
        };
        let legacy_path = parent.join("preferences.toml");
        if !legacy_path.exists() {
            return Ok(0);
        }
        let raw = fs::read_to_string(&legacy_path)
            .with_context(|| format!("读取旧 preferences 配置失败: {}", legacy_path.display()))?;
        #[derive(Deserialize, Default)]
        struct LegacyPreferences {
            theme: Option<String>,
            statusbar_mode: Option<String>,
            font_size: Option<f32>,
        }
        let legacy: LegacyPreferences = toml::from_str(&raw).unwrap_or_default();
        let mut operations = Vec::new();
        if let Some(theme) = legacy.theme {
            operations.push(JsonPatchOperation {
                op: "replace".into(),
                path: "/theme/name".into(),
                value: Some(Value::String(theme)),
            });
        }
        if let Some(mode) = legacy.statusbar_mode {
            operations.push(JsonPatchOperation {
                op: "replace".into(),
                path: "/statusbar/mode".into(),
                value: Some(Value::String(mode)),
            });
        }
        if let Some(size) = legacy.font_size {
            operations.push(JsonPatchOperation {
                op: "replace".into(),
                path: "/font/size".into(),
                value: Some(Value::from(f64::from(size))),
            });
        }
        if operations.is_empty() {
            return Ok(0);
        }
        let transaction = self.begin();
        self.patch(&transaction, &operations)?;
        self.commit(&transaction)?;
        Ok(operations.len())
    }

    pub fn reload_if_changed(&mut self) -> Result<bool> {
        if !self.path.exists() {
            return Ok(false);
        }
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("读取配置失败: {}", self.path.display()))?;
        if revision_for(&raw) == self.revision {
            return Ok(false);
        }
        self.reload()?;
        Ok(true)
    }
}

fn apply_patch_operation(document: &mut Value, operation: &JsonPatchOperation) -> Result<()> {
    let tokens = pointer_tokens(&operation.path)?;
    if tokens.is_empty() {
        return match operation.op.as_str() {
            "replace" | "add" => {
                *document = operation
                    .value
                    .clone()
                    .ok_or_else(|| anyhow!("patch 缺少 value"))?;
                Ok(())
            }
            "remove" => Err(anyhow!("不能 remove 根节点")),
            other => Err(anyhow!("不支持的 patch 操作: {other}")),
        };
    }
    let (parent_tokens, leaf) = tokens.split_at(tokens.len() - 1);
    let parent = pointer_mut(document, parent_tokens)?;
    match parent {
        Value::Object(map) => match operation.op.as_str() {
            "add" | "replace" => {
                map.insert(
                    leaf[0].clone(),
                    operation
                        .value
                        .clone()
                        .ok_or_else(|| anyhow!("patch 缺少 value"))?,
                );
                Ok(())
            }
            "remove" => map
                .remove(&leaf[0])
                .map(|_| ())
                .ok_or_else(|| anyhow!("patch 路径不存在: {}", operation.path)),
            other => Err(anyhow!("不支持的 patch 操作: {other}")),
        },
        Value::Array(items) => {
            let index = if leaf[0] == "-" {
                items.len()
            } else {
                leaf[0].parse::<usize>().context("数组 patch 下标无效")?
            };
            match operation.op.as_str() {
                "add" => {
                    let value = operation
                        .value
                        .clone()
                        .ok_or_else(|| anyhow!("patch 缺少 value"))?;
                    if index > items.len() {
                        return Err(anyhow!("数组 patch 下标越界"));
                    }
                    items.insert(index, value);
                    Ok(())
                }
                "replace" => {
                    if index >= items.len() {
                        return Err(anyhow!("数组 patch 下标越界"));
                    }
                    items[index] = operation
                        .value
                        .clone()
                        .ok_or_else(|| anyhow!("patch 缺少 value"))?;
                    Ok(())
                }
                "remove" => {
                    if index >= items.len() {
                        return Err(anyhow!("数组 patch 下标越界"));
                    }
                    items.remove(index);
                    Ok(())
                }
                other => Err(anyhow!("不支持的 patch 操作: {other}")),
            }
        }
        _ => Err(anyhow!(
            "patch 父路径不是 object 或 array: {}",
            operation.path
        )),
    }
}

fn pointer_tokens(path: &str) -> Result<Vec<String>> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    if !path.starts_with('/') {
        return Err(anyhow!("JSON Pointer 必须以 / 开头: {path}"));
    }
    path[1..]
        .split('/')
        .map(|token| Ok(token.replace("~1", "/").replace("~0", "~")))
        .collect()
}

fn pointer_mut<'a>(value: &'a mut Value, tokens: &[String]) -> Result<&'a mut Value> {
    let mut current = value;
    for token in tokens {
        current = match current {
            Value::Object(map) => map
                .get_mut(token)
                .ok_or_else(|| anyhow!("patch 路径不存在: {token}"))?,
            Value::Array(items) => items
                .get_mut(token.parse::<usize>().context("数组 patch 下标无效")?)
                .ok_or_else(|| anyhow!("patch 数组下标越界: {token}"))?,
            _ => return Err(anyhow!("patch 路径经过了标量")),
        };
    }
    Ok(current)
}

fn merge_three_way(base: &Value, mine: &Value, disk: &Value) -> Result<Value> {
    if mine == base {
        return Ok(disk.clone());
    }
    if disk == base || mine == disk {
        return Ok(mine.clone());
    }
    match (base, mine, disk) {
        (Value::Object(base), Value::Object(mine), Value::Object(disk)) => {
            let mut merged = Map::new();
            let keys: BTreeSet<String> = base
                .keys()
                .chain(mine.keys())
                .chain(disk.keys())
                .cloned()
                .collect();
            for key in keys {
                let b = base.get(&key).unwrap_or(&Value::Null);
                let m = mine.get(&key).unwrap_or(&Value::Null);
                let d = disk.get(&key).unwrap_or(&Value::Null);
                let value =
                    merge_three_way(b, m, d).with_context(|| format!("配置字段冲突: /{key}"))?;
                if !value.is_null() || mine.contains_key(&key) || disk.contains_key(&key) {
                    merged.insert(key, value);
                }
            }
            Ok(Value::Object(merged))
        }
        (Value::Array(base), Value::Array(mine), Value::Array(disk)) => {
            for key in ["id", "action"] {
                if is_keyed_array(base, key)
                    && is_keyed_array(mine, key)
                    && is_keyed_array(disk, key)
                {
                    return merge_keyed_array(base, mine, disk, key);
                }
            }
            Err(anyhow!("配置存在并发修改冲突"))
        }
        _ => Err(anyhow!("配置存在并发修改冲突")),
    }
}

fn is_keyed_array(items: &[Value], key: &str) -> bool {
    let mut seen = BTreeSet::new();
    items.iter().all(|item| {
        item.get(key)
            .and_then(Value::as_str)
            .map(|value| seen.insert(value.to_string()))
            .unwrap_or(false)
    })
}

fn merge_keyed_array(base: &[Value], mine: &[Value], disk: &[Value], key: &str) -> Result<Value> {
    let base_index = keyed_index(base, key);
    let mine_index = keyed_index(mine, key);
    let disk_index = keyed_index(disk, key);
    let ids: BTreeSet<String> = base_index
        .keys()
        .chain(mine_index.keys())
        .chain(disk_index.keys())
        .cloned()
        .collect();
    let mut merged = Vec::new();
    for id in ids {
        let value = merge_optional(
            base_index.get(&id).copied(),
            mine_index.get(&id).copied(),
            disk_index.get(&id).copied(),
        )?;
        if let Some(value) = value {
            merged.push(value);
        }
    }
    Ok(Value::Array(merged))
}

fn keyed_index<'a>(items: &'a [Value], key: &str) -> BTreeMap<String, &'a Value> {
    items
        .iter()
        .filter_map(|item| {
            item.get(key)
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), item))
        })
        .collect()
}

fn merge_optional(
    base: Option<&Value>,
    mine: Option<&Value>,
    disk: Option<&Value>,
) -> Result<Option<Value>> {
    match (base, mine, disk) {
        (None, None, None) => Ok(None),
        (None, Some(value), None) | (None, None, Some(value)) => Ok(Some(value.clone())),
        (None, Some(mine), Some(disk)) => Ok(Some(merge_three_way(&Value::Null, mine, disk)?)),
        (Some(base), Some(mine), Some(disk)) => Ok(Some(merge_three_way(base, mine, disk)?)),
        (Some(base), None, None) => {
            // Both sides deleted the same item.
            let _ = base;
            Ok(None)
        }
        (Some(base), None, Some(disk)) if disk == base => Ok(None),
        (Some(base), Some(mine), None) if mine == base => Ok(None),
        (Some(_), None, Some(_)) | (Some(_), Some(_), None) => Err(anyhow!("配置存在并发修改冲突")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "muxterm-config-service-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn transaction_preview_cancel_and_commit() {
        let path = temp_path("transaction.toml");
        let _ = fs::remove_file(&path);
        let mut service = SettingsService::open(&path).unwrap();
        let transaction = service.begin();
        service
            .patch(
                &transaction,
                &[JsonPatchOperation {
                    op: "replace".into(),
                    path: "/font/size".into(),
                    value: Some(Value::from(15.0)),
                }],
            )
            .unwrap();
        assert_eq!(service.document().config.font.size, 13.0);
        service.commit(&transaction).unwrap();
        assert_eq!(service.document().config.font.size, 15.0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn three_way_merge_keeps_disjoint_object_changes() {
        let base = serde_json::json!({"a": 1, "b": 1});
        let mine = serde_json::json!({"a": 2, "b": 1});
        let disk = serde_json::json!({"a": 1, "b": 3});
        assert_eq!(
            merge_three_way(&base, &mine, &disk).unwrap(),
            serde_json::json!({"a": 2, "b": 3})
        );
    }

    #[test]
    fn keyed_project_merge_keeps_disjoint_field_edits() {
        let base = serde_json::json!({
            "projects": [
                {"id": "a", "name": "A", "path": "~/a", "runtime": {"id": "tmux"}, "transport": {"id": "local"}}
            ]
        });
        let mine = serde_json::json!({
            "projects": [
                {"id": "a", "name": "A2", "path": "~/a", "runtime": {"id": "tmux"}, "transport": {"id": "local"}}
            ]
        });
        let disk = serde_json::json!({
            "projects": [
                {"id": "a", "name": "A", "path": "~/b", "runtime": {"id": "tmux"}, "transport": {"id": "local"}}
            ]
        });
        let merged = merge_three_way(&base, &mine, &disk).unwrap();
        assert_eq!(merged["projects"][0]["name"], "A2");
        assert_eq!(merged["projects"][0]["path"], "~/b");
    }

    #[test]
    fn keyed_project_merge_keeps_concurrent_additions() {
        let base = serde_json::json!({"projects": []});
        let mine = serde_json::json!({
            "projects": [
                {"id": "mine", "name": "Mine", "path": "~/m", "runtime": {"id": "tmux"}, "transport": {"id": "local"}}
            ]
        });
        let disk = serde_json::json!({
            "projects": [
                {"id": "disk", "name": "Disk", "path": "~/d", "runtime": {"id": "tmux"}, "transport": {"id": "local"}}
            ]
        });
        let merged = merge_three_way(&base, &mine, &disk).unwrap();
        assert_eq!(merged["projects"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn snapshot_distinguishes_sparse_raw_and_resolved_values() {
        let path = temp_path("sparse.toml");
        let _ = fs::remove_file(&path);
        fs::write(&path, "[font]\nsize = 15\n").unwrap();
        let service = SettingsService::open(&path).unwrap();
        let snapshot = service.snapshot();
        assert!(snapshot.raw["font"]["family"].is_null());
        assert_eq!(snapshot.values["font"]["family"], "JetBrains Mono");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn migrate_legacy_linux_preferences_merges_overrides() {
        let dir = std::env::temp_dir().join(format!(
            "muxterm-config-service-legacy-merge-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let legacy = dir.join("preferences.toml");
        fs::write(
            &legacy,
            "theme = \"black\"\nstatusbar_mode = \"theme\"\nfont_size = 15.0\n",
        )
        .unwrap();
        let mut service = SettingsService::open(&path).unwrap();
        let migrated = service.migrate_legacy_linux_preferences().unwrap();
        assert_eq!(migrated, 3);
        assert_eq!(service.document().config.theme.name, "black");
        assert_eq!(service.document().config.statusbar.mode, "theme");
        assert_eq!(service.document().config.font.size, 15.0);
        // 旧文件保留为可恢复备份，不删除。
        assert!(legacy.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_legacy_linux_preferences_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "muxterm-config-service-legacy-idem-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let legacy = dir.join("preferences.toml");
        fs::write(&legacy, "font_size = 14.0\n").unwrap();
        let mut service = SettingsService::open(&path).unwrap();
        assert_eq!(service.migrate_legacy_linux_preferences().unwrap(), 1);
        assert_eq!(service.migrate_legacy_linux_preferences().unwrap(), 1);
        assert_eq!(service.document().config.font.size, 14.0);
        let _ = fs::remove_dir_all(&dir);
    }
}
