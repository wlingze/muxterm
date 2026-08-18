//! QuickConnect 数据（Recent + Project）的持久化与列表管理（纯逻辑）。
//!
//! - Recent：最近连接过的目标（最多 20 条），去重，最近的在最前；
//!   recents 不落盘，由连接池在运行时派生。
//! - Project：用户配置的预设目标，首选落盘统一的 `config.toml`。
//!
//! 格式与 macOS Chrome/QuickConnectStore.swift 一致，便于两边共用同一份
//! 用户配置文件。

use std::path::{Path, PathBuf};

use super::model::{QuickConnect, TargetConfig, TargetRuntime, TargetTransport};

/// QuickConnect 数据存储。
#[derive(Debug, Clone, Default)]
pub struct QuickConnectStore {
    pub recents: Vec<TargetConfig>,
    pub projects: Vec<TargetConfig>,
    file_url: Option<PathBuf>,
    unified_config: bool,
}

/// 最近连接记录条数上限。
pub const MAX_RECENT: usize = 20;

/// 用户配置文件路径：`~/.config/muxterm/quickconnect.toml`。
pub fn user_quickconnect_path() -> Option<PathBuf> {
    crate::core::config::user_config_dir().map(|d| d.join("quickconnect.toml"))
}

impl QuickConnectStore {
    pub fn new(file_url: Option<PathBuf>) -> Self {
        let mut store = QuickConnectStore::default();
        if let Some(url) = file_url {
            store.load(&url);
            let missing = !url.exists();
            store.file_url = Some(url);
            // 启动时若文件不存在，写出空 projects，保证配置目录可被发现。
            if missing {
                store.persist();
            }
        }
        store
    }

    /// Create a store backed by Core's unified `config.toml` Project array.
    /// The legacy QuickConnect file is imported by SettingsService when present.
    pub fn new_unified(config_path: Option<PathBuf>) -> Self {
        let Some(path) = config_path else {
            return Self::default();
        };
        let mut store = QuickConnectStore {
            file_url: Some(path.clone()),
            unified_config: true,
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

    /// 记录一次连接：放进 recents 最前并按唯一 ID 去重。仅内存态。
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

    /// 新增或更新一个 project（按唯一 ID name+transport 匹配）。返回是否新增。
    pub fn upsert_project(&mut self, config: &TargetConfig) -> bool {
        let id = QuickConnect::unique_id(config);
        if let Some(idx) = self
            .projects
            .iter()
            .position(|p| QuickConnect::unique_id(p) == id)
        {
            self.projects[idx] = config.clone();
            self.persist();
            return false;
        }
        self.projects.push(config.clone());
        self.persist();
        true
    }

    /// 删除 project（按唯一 ID name+transport）。
    pub fn remove_project(&mut self, config: &TargetConfig) {
        let id = QuickConnect::unique_id(config);
        self.projects.retain(|p| QuickConnect::unique_id(p) != id);
        self.persist();
    }

    /// 序列化（TOML）：只写 projects，recent 不持久化。
    pub fn encode(&self) -> String {
        let mut out = String::from("# Muxterm QuickConnect 配置（TOML）\n");
        out.push_str("# 只保存 projects；recents 由连接池在运行时派生，不落盘。\n\n");
        out.push_str(&encode_section("projects", &self.projects));
        out
    }

    /// 从 TOML 解析并替换当前状态；非法/未知条目跳过。兼容旧版 recents 段。
    pub fn decode(&mut self, text: &str) {
        let mut section: Option<String> = None;
        let mut fields: std::collections::HashMap<String, String> = Default::default();
        let mut recents_buf: Vec<TargetConfig> = Vec::new();
        let mut projects_buf: Vec<TargetConfig> = Vec::new();

        fn flush(
            section: &Option<String>,
            fields: &mut std::collections::HashMap<String, String>,
            recents: &mut Vec<TargetConfig>,
            projects: &mut Vec<TargetConfig>,
        ) {
            if let Some(cfg) = config_from_fields(fields) {
                match section.as_deref() {
                    Some("recents") => recents.push(cfg),
                    Some("projects") => projects.push(cfg),
                    _ => {}
                }
            }
            fields.clear();
        }

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("[[") && line.ends_with("]]") {
                flush(&section, &mut fields, &mut recents_buf, &mut projects_buf);
                let name = line[2..line.len() - 2].trim().to_string();
                section = if name == "recents" || name == "projects" {
                    Some(name)
                } else {
                    None
                };
                continue;
            }
            if section.is_none() {
                continue;
            }
            let Some(eq) = line.find('=') else {
                continue;
            };
            let key = line[..eq].trim().to_string();
            let value = line[eq + 1..].trim().to_string();
            let Some(parsed) = parse_toml_string(&value) else {
                continue;
            };
            fields.insert(key, parsed);
        }
        flush(&section, &mut fields, &mut recents_buf, &mut projects_buf);

        if !recents_buf.is_empty() {
            self.recents = recents_buf;
        }
        self.projects = projects_buf;
    }

    // MARK: - 持久化

    fn load(&mut self, url: &Path) {
        let Ok(data) = std::fs::read_to_string(url) else {
            return;
        };
        self.decode(&data);
    }

    fn persist(&self) {
        let Some(file_url) = &self.file_url else {
            return;
        };
        if self.unified_config {
            let Ok(mut service) = crate::core::config_service::SettingsService::open(file_url)
            else {
                return;
            };
            let transaction = service.begin();
            let value = serde_json::Value::Array(
                self.projects
                    .iter()
                    .map(|project| {
                        crate::core::config_service::ProjectDocument::from_target(project)
                    })
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
            return;
        }
        if let Some(parent) = file_url.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    target = "muxterm::quickconnect",
                    path = %parent.display(),
                    "创建 QuickConnect 配置目录失败: {e}"
                );
                return;
            }
        }
        if let Err(e) = std::fs::write(file_url, self.encode()) {
            tracing::warn!(
                target = "muxterm::quickconnect",
                path = %file_url.display(),
                "写入 quickconnect.toml 失败: {e}"
            );
        }
    }
}

fn encode_section(name: &str, items: &[TargetConfig]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!("[[{name}]]\n"));
        out.push_str(&format!("name = {}\n", toml_string(&item.name)));
        out.push_str(&format!(
            "runtime = {}\n",
            toml_string(item.runtime.as_str())
        ));
        match &item.transport {
            TargetTransport::Local => out.push_str("transport = \"local\"\n"),
            TargetTransport::Ssh { name } => {
                out.push_str("transport = \"ssh\"\n");
                out.push_str(&format!("transport_name = {}\n", toml_string(name)));
            }
        }
        out.push_str(&format!("path = {}\n", toml_string(&item.path)));
        if let Some(socket) = &item.socket {
            out.push_str(&format!("socket = {}\n", toml_string(socket)));
        }
        if let Some(session) = &item.session {
            out.push_str(&format!("session = {}\n", toml_string(session)));
        }
        if let Some(workspace_id) = &item.workspace_id {
            out.push_str(&format!("workspace_id = {}\n", toml_string(workspace_id)));
        }
        out.push('\n');
    }
    out
}

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn parse_toml_string(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.starts_with('"') || !raw.ends_with('"') {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            _ => return None,
        }
    }
    Some(out)
}

fn config_from_fields(fields: &std::collections::HashMap<String, String>) -> Option<TargetConfig> {
    let name = fields.get("name")?.to_string();
    let runtime = TargetRuntime::from_str(fields.get("runtime")?)?;
    let transport_raw = fields.get("transport")?;
    let transport = match transport_raw.as_str() {
        "local" => TargetTransport::Local,
        "ssh" => TargetTransport::Ssh {
            name: fields.get("transport_name")?.to_string(),
        },
        _ => return None,
    };
    let path = fields.get("path")?.to_string();
    let mut config = TargetConfig::new(name, runtime, transport, path);
    // 旧 TOML 缺新字段仍可读（None）；保存后自动迁移补全（W6 §11.1）。
    config.socket = fields.get("socket").cloned();
    config.session = fields.get("session").cloned().filter(|s| !s.is_empty());
    config.workspace_id = fields
        .get("workspace_id")
        .cloned()
        .filter(|s| !s.is_empty());
    // 一次迁移：旧格式把 Herdr workspace id 塞在 path 段（如 `w1`），
    // 且没有独立 workspace_id 字段 → 移入 workspace_id，path 保持原样
    // 不覆盖（path 是项目目录语义，禁止把 wN 当目录回填）。
    if config.workspace_id.is_none() && config.runtime == TargetRuntime::Herdr {
        if let Some(legacy) = legacy_workspace_id_from_path(&config.path) {
            config.workspace_id = Some(legacy);
        }
    }
    Some(config)
}

/// 旧 TOML 的 path 恰为 Herdr workspace id（`w<数字>`）的一次性迁移；
/// 不是目录路径时不迁移（避免把真实目录误当 workspace id）。
fn legacy_workspace_id_from_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    let digits = trimmed.strip_prefix('w')?;
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        name: &str,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: &str,
    ) -> TargetConfig {
        TargetConfig::new(name, runtime, transport, path)
    }

    #[test]
    fn encode_decode_roundtrip() {
        let mut store = QuickConnectStore::new(None);
        store.upsert_project(&cfg(
            "muxterm",
            TargetRuntime::Shell,
            TargetTransport::Local,
            "~/Developer/self/muxterm",
        ));
        store.upsert_project(&cfg(
            "server",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/work",
        ));
        let text = store.encode();
        let mut back = QuickConnectStore::new(None);
        back.decode(&text);
        assert_eq!(back.projects, store.projects);
        assert!(text.contains("transport_name = \"ryzen\""));
    }

    /// W6 §11.1：session / socket / workspace_id 必须 TOML round-trip，
    /// 且 path（项目目录）与 workspace_id（`wN`）互不覆盖。
    #[test]
    fn herdr_identity_fields_roundtrip_and_do_not_overwrite() {
        let mut store = QuickConnectStore::new(None);
        let mut project = cfg(
            "proj",
            TargetRuntime::Herdr,
            TargetTransport::Local,
            "/srv/project",
        );
        project.session = Some("dev".into());
        project.socket = Some("/tmp/herdr-dev.sock".into());
        project.workspace_id = Some("w3".into());
        store.upsert_project(&project);

        let text = store.encode();
        assert!(text.contains("workspace_id = \"w3\""), "{text}");
        assert!(text.contains("session = \"dev\""), "{text}");
        assert!(text.contains("socket = \"/tmp/herdr-dev.sock\""), "{text}");

        let mut back = QuickConnectStore::new(None);
        back.decode(&text);
        assert_eq!(back.projects.len(), 1);
        let loaded = &back.projects[0];
        assert_eq!(
            loaded.path, "/srv/project",
            "path 是项目目录，不能被 workspace_id 覆盖"
        );
        assert_eq!(loaded.workspace_id.as_deref(), Some("w3"));
        assert_eq!(loaded.session.as_deref(), Some("dev"));
        assert_eq!(loaded.socket.as_deref(), Some("/tmp/herdr-dev.sock"));
    }

    /// W6 §11.1：旧 TOML 没有 session/socket/workspace_id 字段仍可读；
    /// 旧格式把 Herdr workspace id 塞在 path（如 `w1`）→ 一次性迁移到
    /// workspace_id，path 保持原样（不能把 wN 当目录回填）。
    #[test]
    fn legacy_toml_missing_identity_fields_migrates_workspace_id_from_path() {
        let mut store = QuickConnectStore::new(None);
        store.decode(
            r#"
[[projects]]
name = "legacy"
runtime = "herdr"
transport = "local"
path = "w2"
"#,
        );
        assert_eq!(store.projects.len(), 1);
        let legacy = &store.projects[0];
        assert_eq!(
            legacy.workspace_id.as_deref(),
            Some("w2"),
            "旧 path=w2 必须迁移到 workspace_id"
        );
        assert_eq!(
            legacy.path, "w2",
            "迁移不改 path（旧值保留，保存后才有新格式）"
        );
        assert_eq!(legacy.session, None);
        assert_eq!(legacy.socket, None);

        // 目录路径不是 workspace id，不迁移。
        let mut dir = QuickConnectStore::new(None);
        dir.decode(
            r#"
[[projects]]
name = "dir"
runtime = "herdr"
transport = "local"
path = "/srv/project"
"#,
        );
        assert_eq!(dir.projects[0].workspace_id, None);
        assert_eq!(dir.projects[0].path, "/srv/project");
    }

    /// W6 §11.1：identity key 由 transport target / runtime / session /
    /// target-side socket / workspace_id 构成；name 与 path 变更不改变身份。
    #[test]
    fn identity_key_ignores_name_and_path() {
        let mut a = cfg("alpha", TargetRuntime::Herdr, TargetTransport::Local, "/a");
        a.session = Some("dev".into());
        a.socket = Some("/tmp/herdr-dev.sock".into());
        a.workspace_id = Some("w3".into());
        let mut b = a.clone();
        b.name = "beta".into();
        b.path = "/b".into();
        assert_eq!(a.identity_key(), b.identity_key());

        // 同名同 path 但不同 socket/session/workspace_id → 不同身份。
        let mut c = a.clone();
        c.socket = Some("/other.sock".into());
        assert_ne!(a.identity_key(), c.identity_key());
        let mut d = a.clone();
        d.workspace_id = Some("w9".into());
        assert_ne!(a.identity_key(), d.identity_key());
        let mut e = a.clone();
        e.session = Some("prod".into());
        assert_ne!(a.identity_key(), e.identity_key());
    }

    #[test]
    fn decode_ignores_unknown_sections_and_bad_entries() {
        let mut store = QuickConnectStore::new(None);
        store.decode(
            r#"
[[other]]
name = "x"
[[projects]]
name = "ok"
runtime = "tmux"
transport = "local"
path = "~/p"
[[projects]]
name = "bad"
runtime = "weird"
"#,
        );
        assert_eq!(store.projects.len(), 1);
        assert_eq!(store.projects[0].name, "ok");
    }

    #[test]
    fn record_recent_dedupes_and_limits() {
        let mut store = QuickConnectStore::new(None);
        for i in 0..25 {
            store.record_recent(&cfg(
                &format!("p{i}"),
                TargetRuntime::Shell,
                TargetTransport::Local,
                "~/x",
            ));
        }
        assert_eq!(store.recents.len(), MAX_RECENT);
        assert_eq!(store.recents[0].name, "p24");
        store.record_recent(&cfg(
            "p24",
            TargetRuntime::Shell,
            TargetTransport::Local,
            "~/x",
        ));
        assert_eq!(store.recents.len(), MAX_RECENT);
        assert_eq!(store.recents[0].name, "p24");
    }

    #[test]
    fn upsert_updates_existing() {
        let mut store = QuickConnectStore::new(None);
        let a = cfg("srv", TargetRuntime::Shell, TargetTransport::Local, "~/a");
        assert!(store.upsert_project(&a));
        let b = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/b");
        assert!(!store.upsert_project(&b));
        assert_eq!(store.projects.len(), 1);
        assert_eq!(store.projects[0].path, "~/b");
    }

    fn temp_qc_path() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "muxterm-qc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        dir.join("quickconnect.toml")
    }

    /// 回归：`new()` 必须记住落盘路径，否则 upsert 静默不写文件。
    #[test]
    fn persist_creates_config_dir_and_writes_projects() {
        let path = temp_qc_path();
        let parent = path.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(&parent);
        let mut store = QuickConnectStore::new(Some(path.clone()));
        assert!(
            path.exists(),
            "启动时应创建空的 quickconnect.toml: {}",
            path.display()
        );
        store.upsert_project(&cfg(
            "muxterm",
            TargetRuntime::Shell,
            TargetTransport::Local,
            "~/Developer/self/muxterm",
        ));
        let text = std::fs::read_to_string(&path).expect("应能读回 quickconnect.toml");
        assert!(text.contains("name = \"muxterm\""));
        assert!(text.contains("runtime = \"shell\""));
        let back = QuickConnectStore::new(Some(path.clone()));
        assert_eq!(back.projects.len(), 1);
        assert_eq!(back.projects[0].name, "muxterm");
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn user_quickconnect_path_is_under_user_config_dir() {
        let dir = crate::core::config::user_config_dir();
        let qc = user_quickconnect_path();
        match (dir, qc) {
            (Some(d), Some(p)) => assert_eq!(p, d.join("quickconnect.toml")),
            (None, None) => {}
            other => panic!("config dir 与 quickconnect 路径应同时有或同时无: {other:?}"),
        }
    }
}
