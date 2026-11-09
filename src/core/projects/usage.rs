//! 工作区使用频率：attach 计数，归 Core 所有。
//!
//! 面板（Quick Panel）要按「你回得最多的 workspace」排序。这个排序数据
//! 必须由 Core 记账，不能各前端各存一份：
//!
//! * Core 在 workspace 激活（attach）时记一次；
//! * 所有前端通过 FFI 读同一份快照（`muxterm_workspace_usage_json`）；
//! * 保留天数读 `[quick_panel].attach_history_days`（默认 30，1..=365）。
//!
//! 身份键复用 Catalog 的 `target_identity_key`：与 Quick Panel 去重是同一套
//! 身份，不会出现「面板一行、历史一行」。记录里同时存 workspace id 与
//! project id，前端不需要自己重算身份键。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 一次 attach 的累计记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEntry {
    /// `target_identity_key`：与 Quick Panel 去重同源。
    pub identity: String,
    /// 产品 workspace id（`WorkspacePool` 的 key，用来匹配面板里的现有行）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Catalog canonical workspace id（Herdr/远端目标 id，用来匹配保存的 Project）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_workspace_id: Option<String>,
    /// Project / worktree 的来源 id（`spec.provenance.project_id`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// attach 次数。
    pub count: u32,
    /// 最近一次 attach（unix 秒）。
    pub last_used_at: i64,
}

/// 使用历史：按 identity 去重，最新的在前。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachHistory {
    #[serde(default)]
    entries: Vec<UsageEntry>,
}

/// 保留上限：防止历史无限增长（每次 attach 都会落盘）。
pub const MAX_ENTRIES: usize = 200;

impl AttachHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[UsageEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 记一次 attach：计数 +1，重置时间，挪到最前。
    pub fn record(
        &mut self,
        identity: &str,
        workspace_id: Option<&str>,
        target_workspace_id: Option<&str>,
        project_id: Option<&str>,
        now: i64,
    ) -> &UsageEntry {
        let previous = self
            .entries
            .iter()
            .position(|entry| entry.identity == identity);
        let count = previous
            .map(|index| self.entries[index].count)
            .unwrap_or(0)
            .saturating_add(1);
        if let Some(index) = previous {
            self.entries.remove(index);
        }
        self.entries.insert(
            0,
            UsageEntry {
                identity: identity.to_string(),
                workspace_id: workspace_id.map(str::to_string),
                target_workspace_id: target_workspace_id.map(str::to_string),
                project_id: project_id.map(str::to_string),
                count,
                last_used_at: now,
            },
        );
        self.entries.truncate(MAX_ENTRIES);
        &self.entries[0]
    }

    /// 建索引前先取旧计数（`record` 需要它累加）。
    pub fn count_of(&self, identity: &str) -> u32 {
        self.entries
            .iter()
            .find(|entry| entry.identity == identity)
            .map(|entry| entry.count)
            .unwrap_or(0)
    }

    /// 丢弃超过 `retain_days` 天没用的记录；返回被丢弃的条数。
    pub fn prune(&mut self, retain_days: u32, now: i64) -> usize {
        let days = retain_days.clamp(1, 365) as i64;
        let cutoff = now - days * 24 * 60 * 60;
        let before = self.entries.len();
        self.entries
            .retain(|entry| entry.last_used_at >= cutoff || entry.last_used_at <= 0);
        before - self.entries.len()
    }

    /// 排序后的快照：次数降序、最近使用降序、identity 升序保证稳定。
    pub fn ranked(&self, limit: usize) -> Vec<&UsageEntry> {
        let mut entries: Vec<&UsageEntry> = self.entries.iter().collect();
        entries.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then(right.last_used_at.cmp(&left.last_used_at))
                .then(left.identity.cmp(&right.identity))
        });
        entries.truncate(limit.min(entries.len()));
        entries
    }

    /// 按 identity / workspace id / project id 查排名（次数, 最近使用）。
    pub fn rank_by_identity(&self, identity: &str) -> Option<(u32, i64)> {
        self.entries
            .iter()
            .find(|entry| entry.identity == identity)
            .map(|entry| (entry.count, entry.last_used_at))
    }

    pub fn rank_by_workspace(&self, workspace_id: &str) -> Option<(u32, i64)> {
        self.entries
            .iter()
            .filter(|entry| entry.workspace_id.as_deref() == Some(workspace_id))
            .map(|entry| (entry.count, entry.last_used_at))
            .max()
    }

    pub fn rank_by_target_workspace(&self, target_workspace_id: &str) -> Option<(u32, i64)> {
        self.entries
            .iter()
            .filter(|entry| entry.target_workspace_id.as_deref() == Some(target_workspace_id))
            .map(|entry| (entry.count, entry.last_used_at))
            .max()
    }

    pub fn rank_by_project(&self, project_id: &str) -> Option<(u32, i64)> {
        self.entries
            .iter()
            .filter(|entry| entry.project_id.as_deref() == Some(project_id))
            .map(|entry| (entry.count, entry.last_used_at))
            .max()
    }

    /// FFI 快照：`{"entries":[...]}`，顺序即面板排序。
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "entries": self.ranked(MAX_ENTRIES),
        })
    }

    /// 读历史：文件缺失或损坏都当空历史，绝不让启动失败。
    pub fn load(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// 写历史：先写临时文件再 rename，避免半截 JSON。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, format!("{raw}\n"))?;
        std::fs::rename(&temp, path)
    }

    /// 默认路径：与 `config.toml` 同目录的 `attach-history.json`。
    pub fn default_path() -> Option<PathBuf> {
        crate::config::Config::user_config_path()
            .and_then(|path| path.parent().map(|dir| dir.join("attach-history.json")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(identity: &str, count: u32, last: i64) -> UsageEntry {
        UsageEntry {
            identity: identity.to_string(),
            workspace_id: Some(format!("ws-{identity}")),
            target_workspace_id: None,
            project_id: None,
            count,
            last_used_at: last,
        }
    }

    /// 同一 identity 只累加，不新增行；最新一次排最前。
    #[test]
    fn recording_the_same_identity_accumulates() {
        let mut history = AttachHistory::new();
        history.entries = vec![entry("a", 3, 100)];
        history.record("a", Some("ws-a"), Some("target-a"), None, 200);
        assert_eq!(history.entries().len(), 1);
        assert_eq!(history.entries()[0].count, 4, "计数接着旧值加");
        assert_eq!(history.entries()[0].last_used_at, 200);
        assert_eq!(history.count_of("a"), 4);
    }

    /// 排名：次数优先，其次最近使用，最后 identity 保证稳定。
    #[test]
    fn ranking_prefers_count_then_recency() {
        let mut history = AttachHistory::new();
        history.entries = vec![
            entry("old-often", 5, 10),
            entry("new-often", 5, 90),
            entry("rare", 1, 999),
        ];
        let ranked: Vec<&str> = history
            .ranked(10)
            .into_iter()
            .map(|entry| entry.identity.as_str())
            .collect();
        assert_eq!(ranked, vec!["new-often", "old-often", "rare"]);
    }

    /// `attach_history_days` 之外（且有时间戳）的记录被清掉。
    #[test]
    fn pruning_drops_entries_older_than_the_window() {
        let now = 40 * 24 * 60 * 60;
        let mut history = AttachHistory::new();
        history.entries = vec![
            entry("recent", 1, now - 60),
            entry("stale", 2, now - 31 * 24 * 60 * 60),
        ];
        let removed = history.prune(30, now);
        assert_eq!(removed, 1);
        assert_eq!(history.entries()[0].identity, "recent");
    }

    /// 保留天数越界按 1..=365 收口；0 天不能把历史清空成一个都不剩。
    #[test]
    fn retain_days_are_clamped() {
        let now = 10 * 24 * 60 * 60;
        let mut history = AttachHistory::new();
        history.entries = vec![entry("today", 1, now)];
        assert_eq!(history.prune(0, now), 0, "0 天按 1 天算");
        assert_eq!(history.entries().len(), 1);
    }

    /// 落盘再读回来完全一致；损坏文件当空历史。
    #[test]
    fn history_round_trips_and_tolerates_corruption() {
        let dir = std::env::temp_dir().join(format!(
            "muxterm-usage-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = dir.join("attach-history.json");
        let mut history = AttachHistory::new();
        history.entries = vec![entry("a", 2, 5)];
        history.save(&path).expect("save");
        assert_eq!(AttachHistory::load(&path), history);

        std::fs::write(&path, "not json").expect("write garbage");
        assert_eq!(AttachHistory::load(&path), AttachHistory::new());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 前端拿到的是排序后的 JSON 快照。
    #[test]
    fn json_exposes_the_ranked_snapshot() {
        let mut history = AttachHistory::new();
        history.entries = vec![entry("a", 1, 10), entry("b", 7, 20)];
        let json = history.json();
        let entries = json["entries"].as_array().expect("entries");
        assert_eq!(entries[0]["identity"], "b");
        assert_eq!(entries[0]["count"], 7);
    }
}
