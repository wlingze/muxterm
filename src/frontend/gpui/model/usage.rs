//! 面板排序用的使用频率（数据来自 Core，前端只读）。
//!
//! Core 在 workspace attach 时记账（`muxterm_workspace_usage_json`），这里只
//! 负责把快照换成排序键：
//!
//! * 活跃项固定第一（Swift `UnifiedPanelController` 也是先钉住 active）；
//! * 其余按 attach 次数降序、最近使用降序；
//! * 没有任何记录的项保持原来的目录顺序（排在已有记录之后）。

use serde::Deserialize;

/// Core 的一条 attach 记录。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UsageEntry {
    #[serde(default)]
    pub identity: String,
    /// 产品 workspace id（匹配面板里的现有行）。
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Catalog canonical workspace id（匹配保存的 Project）。
    #[serde(default)]
    pub target_workspace_id: Option<String>,
    /// Project provenance id。
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub last_used_at: i64,
}

impl UsageEntry {
    /// 从 Core 快照换成本地类型。
    pub fn collect(entries: &[crate::frontend::utils::corebridge::ClientUsageEntry]) -> Vec<Self> {
        entries
            .iter()
            .map(|entry| Self {
                identity: entry.identity.clone(),
                workspace_id: entry.workspace_id.clone(),
                target_workspace_id: entry.target_workspace_id.clone(),
                project_id: entry.project_id.clone(),
                count: entry.count,
                last_used_at: entry.last_used_at,
            })
            .collect()
    }

    /// 面板一行对应的排序键；没有记录时返回 `None`。
    pub fn rank_for(
        &self,
        workspace_id: Option<&str>,
        target_workspace_id: Option<&str>,
        project_id: Option<&str>,
    ) -> Option<(u32, i64)> {
        let matches = workspace_id.is_some_and(|id| self.workspace_id.as_deref() == Some(id))
            || target_workspace_id
                .is_some_and(|id| self.target_workspace_id.as_deref() == Some(id))
            || project_id.is_some_and(|id| self.project_id.as_deref() == Some(id));
        matches.then_some((self.count, self.last_used_at))
    }
}

/// 一组记录里取最强的排序键（次数, 最近使用）。
pub fn best_rank(
    usage: &[UsageEntry],
    workspace_id: Option<&str>,
    target_workspace_id: Option<&str>,
    project_id: Option<&str>,
) -> Option<(u32, i64)> {
    usage
        .iter()
        .filter_map(|entry| entry.rank_for(workspace_id, target_workspace_id, project_id))
        .max()
}

/// 排序比较：活跃优先，然后按 Core 的使用频率；没有记录视为 (0, 0)。
///
/// 返回 `std::cmp::Ordering`，调用方用稳定排序，保证同分项保持原顺序。
pub fn compare_rows(
    active: bool,
    left: Option<(u32, i64)>,
    right_active: bool,
    right: Option<(u32, i64)>,
) -> std::cmp::Ordering {
    right_active.cmp(&active).then(right.cmp(&left))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(identity: &str, workspace_id: &str, count: u32, last: i64) -> UsageEntry {
        UsageEntry {
            identity: identity.into(),
            workspace_id: Some(workspace_id.into()),
            target_workspace_id: None,
            project_id: None,
            count,
            last_used_at: last,
        }
    }

    /// 三种 id 任一对上就算这条记录的排名。
    #[test]
    fn rank_matches_workspace_target_or_project() {
        let mut usage = entry("a", "ws-1", 3, 10);
        usage.project_id = Some("proj".into());
        usage.target_workspace_id = Some("remote-7".into());
        assert_eq!(usage.rank_for(Some("ws-1"), None, None), Some((3, 10)));
        assert_eq!(usage.rank_for(None, Some("remote-7"), None), Some((3, 10)));
        assert_eq!(usage.rank_for(None, None, Some("proj")), Some((3, 10)));
        assert_eq!(usage.rank_for(Some("other"), None, None), None);
    }

    /// 多条记录取最强的那个（同一目标可能留下多条）。
    #[test]
    fn best_rank_picks_the_highest_count() {
        let usage = [entry("a", "ws-1", 2, 10), entry("b", "ws-1", 9, 5)];
        assert_eq!(best_rank(&usage, Some("ws-1"), None, None), Some((9, 5)));
        assert_eq!(best_rank(&usage, Some("nope"), None, None), None);
    }

    /// 活跃项永远第一；其余按次数、再按最近使用。
    #[test]
    fn compare_rows_pins_active_then_count_then_recency() {
        use std::cmp::Ordering;
        assert_eq!(
            compare_rows(true, Some((1, 1)), false, Some((99, 99))),
            Ordering::Less,
            "活跃项排最前"
        );
        assert_eq!(
            compare_rows(false, Some((5, 1)), false, Some((6, 1))),
            Ordering::Greater,
            "次数多的在前"
        );
        assert_eq!(
            compare_rows(false, Some((5, 1)), false, Some((5, 9))),
            Ordering::Greater,
            "同次数时最近使用的在前"
        );
        assert_eq!(
            compare_rows(false, None, false, Some((1, 1))),
            Ordering::Greater,
            "没记录的排在后面"
        );
    }
}
