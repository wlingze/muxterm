//! QuickConnect 数据（Recent + Project）的持久化与列表管理（纯逻辑）。
//!
//! - Recent：最近连接过的目标（最多 20 条），去重，最近的在最前；
//!   recents 不落盘，由连接池在运行时派生。
//! - Project：用户配置的预设目标，落盘 `~/.config/muxterm/quickconnect.toml`。
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
}

/// 最近连接记录条数上限。
pub const MAX_RECENT: usize = 20;

/// 用户配置文件路径：`~/.config/muxterm/quickconnect.toml`。
pub fn user_quickconnect_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("muxterm").join("quickconnect.toml"))
}

impl QuickConnectStore {
    pub fn new(file_url: Option<PathBuf>) -> Self {
        let mut store = QuickConnectStore::default();
        if let Some(url) = file_url {
            store.load(&url);
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
        if let Some(parent) = file_url.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(file_url, self.encode());
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
    Some(TargetConfig::new(name, runtime, transport, path))
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
}
