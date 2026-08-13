//! QuickConnect 纯逻辑模型（与 macOS Chrome 层行为一致，无 GTK 依赖）。
//!
//! 目标配置 / 展示 / 派生逻辑，供 QuickConnect 面板与单测共用。

use std::path::Path;

/// 快速连接目标的运行时（shell / tmux）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRuntime {
    Shell,
    Tmux,
}

impl TargetRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetRuntime::Shell => "shell",
            TargetRuntime::Tmux => "tmux",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "shell" => Some(TargetRuntime::Shell),
            "tmux" => Some(TargetRuntime::Tmux),
            _ => None,
        }
    }
}

/// 连接传输（ssh 需要名字；local 不需要）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetTransport {
    Local,
    Ssh { name: String },
}

impl TargetTransport {
    pub fn label(&self) -> String {
        match self {
            TargetTransport::Local => "local".into(),
            TargetTransport::Ssh { name } => name.clone(),
        }
    }

    pub fn is_ssh(&self) -> bool {
        matches!(self, TargetTransport::Ssh { .. })
    }
}

/// 一个可快速连接的目标（Recent / Project 共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetConfig {
    pub name: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    pub path: String,
}

impl TargetConfig {
    pub fn new(
        name: impl Into<String>,
        runtime: TargetRuntime,
        transport: TargetTransport,
        path: impl Into<String>,
    ) -> Self {
        TargetConfig {
            name: name.into(),
            runtime,
            transport,
            path: path.into(),
        }
    }
}

/// 快速连接条目上的小标记：目标同时是 Recent 和/或 Project。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickBadge {
    Recent,
    Project,
}

impl QuickBadge {
    pub fn label(self) -> &'static str {
        match self {
            QuickBadge::Recent => "Recent",
            QuickBadge::Project => "Project",
        }
    }
}

/// 面板中的一行：目标 + 应显示的标记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnectEntry {
    pub config: TargetConfig,
    pub badges: Vec<QuickBadge>,
}

impl QuickConnectEntry {
    pub fn new(config: TargetConfig, badges: Vec<QuickBadge>) -> Self {
        QuickConnectEntry { config, badges }
    }
}

/// 快速连接目标的展示与派生逻辑（纯函数，便于单测）。
pub enum QuickConnect {}

impl QuickConnect {
    /// 从 path 派生默认 name：取路径最后一段目录名（最小目录）。
    /// 根目录 / 空路径回退 "workspace"。
    pub fn default_name(for_path: &str) -> String {
        let trimmed = for_path.trim();
        if trimmed.is_empty() {
            return "workspace".into();
        }
        let last = Path::new(trimmed)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if last.is_empty() || last == "/" {
            return "workspace".into();
        }
        last
    }

    /// 面板副标题：`runtime @ transport`。ssh transport 显示为名字。
    pub fn subtitle(config: &TargetConfig) -> String {
        format!("{} @ {}", config.runtime.as_str(), config.transport.label())
    }

    /// 该目标是否需要 tmux 按 name attach（tmux 且 name 非空）。
    pub fn should_attach(existing_name: Option<&str>, config: &TargetConfig) -> bool {
        config.runtime == TargetRuntime::Tmux && !existing_name.unwrap_or("").trim().is_empty()
    }

    /// 展示文本（搜索用）：name + 副标题 + path。
    pub fn search_text(config: &TargetConfig) -> String {
        format!("{} {} {}", config.name, Self::subtitle(config), config.path).to_lowercase()
    }

    /// 目标唯一 ID：`name@transport`。
    pub fn unique_id(config: &TargetConfig) -> String {
        format!("{}@{}", config.name, config.transport.label())
    }

    /// 计算目标应显示的标记：Recent 在前、Project 在后。
    pub fn badges(
        config: &TargetConfig,
        recents: &[TargetConfig],
        projects: &[TargetConfig],
    ) -> Vec<QuickBadge> {
        let id = Self::unique_id(config);
        let mut result = Vec::new();
        if recents.iter().any(|r| Self::unique_id(r) == id) {
            result.push(QuickBadge::Recent);
        }
        if projects.iter().any(|p| Self::unique_id(p) == id) {
            result.push(QuickBadge::Project);
        }
        result
    }

    /// 面板条目：先展示最近的前 `recent_limit` 条（最新在前），
    /// 再补 Project 中未出现的目标。按唯一 ID 去重。
    pub fn entries(
        recents: &[TargetConfig],
        projects: &[TargetConfig],
        recent_limit: usize,
    ) -> Vec<QuickConnectEntry> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for config in recents.iter().take(recent_limit) {
            let id = Self::unique_id(config);
            if !seen.insert(id) {
                continue;
            }
            result.push(QuickConnectEntry::new(
                config.clone(),
                Self::badges(config, recents, projects),
            ));
        }
        for config in projects {
            let id = Self::unique_id(config);
            if !seen.insert(id) {
                continue;
            }
            result.push(QuickConnectEntry::new(
                config.clone(),
                Self::badges(config, recents, projects),
            ));
        }
        result
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
    fn default_name_from_path() {
        assert_eq!(QuickConnect::default_name("/a/b/c"), "c");
        assert_eq!(
            QuickConnect::default_name("~/Developer/self/muxterm"),
            "muxterm"
        );
        assert_eq!(QuickConnect::default_name(""), "workspace");
        assert_eq!(QuickConnect::default_name("/"), "workspace");
    }

    #[test]
    fn unique_id_distinguishes_transport() {
        let a = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/x");
        let b = cfg(
            "srv",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/x",
        );
        assert_ne!(QuickConnect::unique_id(&a), QuickConnect::unique_id(&b));
        assert_eq!(QuickConnect::unique_id(&a), "srv@local");
    }

    #[test]
    fn badges_both_recent_and_project() {
        let a = cfg("srv", TargetRuntime::Tmux, TargetTransport::Local, "~/x");
        let badges = QuickConnect::badges(&a, std::slice::from_ref(&a), std::slice::from_ref(&a));
        assert_eq!(badges, vec![QuickBadge::Recent, QuickBadge::Project]);
    }

    #[test]
    fn entries_dedupe_and_order() {
        let r1 = cfg("r1", TargetRuntime::Shell, TargetTransport::Local, "~/r1");
        let r2 = cfg("r2", TargetRuntime::Shell, TargetTransport::Local, "~/r2");
        let p1 = cfg("p1", TargetRuntime::Tmux, TargetTransport::Local, "~/p1");
        let dup = r1.clone();
        let entries = QuickConnect::entries(&[r1, r2], &[dup, p1], 5);
        let names: Vec<_> = entries.iter().map(|e| e.config.name.as_str()).collect();
        assert_eq!(names, vec!["r1", "r2", "p1"]);
        assert_eq!(
            entries[0].badges,
            vec![QuickBadge::Recent, QuickBadge::Project]
        );
    }
}
