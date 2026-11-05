//! 前端固定聚合槽（Shells / Agents）的只读投影。
//!
//! 移植自 `chrome/AggregateWorkspaceModel.swift`。
//!
//! 聚合槽只用于 GUI 选择，**不会传给 Core**：它不拥有 Surface，也不复制
//! pane 内容，只是把已有源 Tab 换个顺序投影出来。

use std::collections::{HashMap, HashSet};

use super::sidebar::AgentItem;

/// 前端固定聚合槽的稳定身份。
pub const SHELLS_IDENTITY: &str = "muxterm.frontend.aggregate.shells";
pub const AGENTS_IDENTITY: &str = "muxterm.frontend.aggregate.agents";

/// 聚合源里的一个 tab。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateSourceTab {
    pub id: u32,
    pub title: String,
}

/// 一个真实 shell Workspace 的只读投影输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAggregateSource {
    pub workspace_id: String,
    pub transport: String,
    pub opened_order: u64,
    pub tabs: Vec<AggregateSourceTab>,
}

/// Shells 槽的一页；`source_tab_id` 仍属于对应的真实 shell Workspace。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAggregateTab {
    pub display_id: u32,
    pub workspace_id: String,
    pub source_tab_id: u32,
    pub title: String,
    pub is_local: bool,
}

/// Agents 槽的身份：workspace + 源 tab，两者缺一不可。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentAggregateKey {
    pub workspace_id: String,
    pub source_tab_id: u32,
}

/// Agents 槽的一页；只指向包含 agent 的真实源 Tab，不拥有或复制 Surface。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAggregateTab {
    pub display_id: u32,
    pub workspace_id: String,
    pub source_tab_id: u32,
    pub agent_pane_ids: Vec<u32>,
    pub title: String,
}

impl AgentAggregateTab {
    pub fn key(&self) -> AgentAggregateKey {
        AgentAggregateKey {
            workspace_id: self.workspace_id.clone(),
            source_tab_id: self.source_tab_id,
        }
    }
}

/// 聚合投影的「延后激活」门禁。
///
/// 发现目标变了不立刻切可见区，而是排到下一轮。这段延迟里世界会变：
/// 用户可能已经开始一个 pending open，或切到别的投影。
///
/// 此时无条件激活有两个后果：
/// 1. 激活聚合槽会清掉连接页；
/// 2. `connecting` 期间被改掉后，attach 结果会走隐藏插入，首帧永远画不出来。
///
/// 所以延后块必须重新确认：仍停在调度时的那条投影上才允许激活。
pub fn should_activate_deferred(
    scheduled: &str,
    current: Option<&str>,
    is_connecting: bool,
) -> bool {
    if is_connecting {
        return false;
    }
    current == Some(scheduled)
}

/// 本机页永远排在最前；机器之间沿用真实 Workspace 的固定打开顺序。
pub fn shell_tabs(sources: &[ShellAggregateSource]) -> Vec<ShellAggregateTab> {
    let mut sorted: Vec<&ShellAggregateSource> = sources.iter().collect();
    sorted.sort_by(|lhs, rhs| {
        let lhs_local = lhs.transport == "local";
        let rhs_local = rhs.transport == "local";
        if lhs_local != rhs_local {
            return rhs_local.cmp(&lhs_local);
        }
        if lhs.opened_order != rhs.opened_order {
            return lhs.opened_order.cmp(&rhs.opened_order);
        }
        lhs.workspace_id.cmp(&rhs.workspace_id)
    });

    let mut display_id: u32 = 1;
    let mut result = Vec::new();
    for source in sorted {
        let is_local = source.transport == "local";
        for tab in &source.tabs {
            result.push(ShellAggregateTab {
                display_id,
                workspace_id: source.workspace_id.clone(),
                source_tab_id: tab.id,
                title: format!("{} · {}", source.transport, tab.title),
                is_local,
            });
            display_id = display_id.wrapping_add(1);
        }
    }
    result
}

/// 一个真实源 Tab 只投影一页，即使它同时包含 shell 与多个 agent pane。
/// 次序只由 Workspace / Tab 身份决定，状态变化不会让页面跳位。
pub fn agent_tabs(agents: &[AgentItem], workspace_order: &[String]) -> Vec<AgentAggregateTab> {
    let rank: HashMap<&str, usize> = workspace_order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();

    // 只投影有 tab_id 的条目：没有 tab 身份的 agent 进不了聚合页。
    let mut ordered: Vec<&AgentItem> = agents
        .iter()
        .filter(|agent| agent.tab_id.is_some())
        .collect();
    ordered.sort_by(|lhs, rhs| {
        let lhs_rank = rank.get(lhs.workspace_id.as_str()).copied();
        let rhs_rank = rank.get(rhs.workspace_id.as_str()).copied();
        let lhs_rank_key = lhs_rank.unwrap_or(usize::MAX);
        let rhs_rank_key = rhs_rank.unwrap_or(usize::MAX);
        if lhs_rank_key != rhs_rank_key {
            return lhs_rank_key.cmp(&rhs_rank_key);
        }
        let lhs_tab = lhs.tab_number.unwrap_or(u32::MAX);
        let rhs_tab = rhs.tab_number.unwrap_or(u32::MAX);
        if lhs_tab != rhs_tab {
            return lhs_tab.cmp(&rhs_tab);
        }
        let lhs_tab_id = lhs.tab_id.unwrap_or(u32::MAX);
        let rhs_tab_id = rhs.tab_id.unwrap_or(u32::MAX);
        if lhs_tab_id != rhs_tab_id {
            return lhs_tab_id.cmp(&rhs_tab_id);
        }
        if lhs.pane_id != rhs.pane_id {
            return lhs.pane_id.cmp(&rhs.pane_id);
        }
        lhs.agent_name
            .to_lowercase()
            .cmp(&rhs.agent_name.to_lowercase())
    });

    let mut ordered_keys: Vec<AgentAggregateKey> = Vec::new();
    let mut by_tab: HashMap<AgentAggregateKey, Vec<&AgentItem>> = HashMap::new();
    for agent in ordered {
        let Some(source_tab_id) = agent.tab_id else {
            continue;
        };
        let key = AgentAggregateKey {
            workspace_id: agent.workspace_id.clone(),
            source_tab_id,
        };
        if !by_tab.contains_key(&key) {
            ordered_keys.push(key.clone());
        }
        by_tab.entry(key).or_default().push(agent);
    }

    ordered_keys
        .iter()
        .enumerate()
        .filter_map(|(offset, key)| {
            let tab_agents = by_tab.get(key)?;
            let first = tab_agents.first()?;
            let mut components = vec![first.title.clone()];
            let names = unique_nonempty(tab_agents.iter().map(|a| Some(a.agent_name.as_str())));
            if !names.is_empty() {
                components.push(names.join(", "));
            }
            let session_titles =
                unique_nonempty(tab_agents.iter().map(|a| a.session_title.as_deref()))
                    .into_iter()
                    .filter(|title| {
                        !components
                            .iter()
                            .any(|existing| existing.to_lowercase() == title.to_lowercase())
                    })
                    .collect::<Vec<_>>();
            if !session_titles.is_empty() {
                components.push(session_titles.join(" / "));
            }
            Some(AgentAggregateTab {
                display_id: (offset + 1) as u32,
                workspace_id: key.workspace_id.clone(),
                source_tab_id: key.source_tab_id,
                agent_pane_ids: tab_agents.iter().map(|a| a.pane_id).collect(),
                title: components.join(" · "),
            })
        })
        .collect()
}

/// 去空、去重（大小写不敏感），保留首次出现的写法。
fn unique_nonempty<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in values {
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        if seen.insert(value.to_lowercase()) {
            result.push(value.to_string());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::gpui::model::sidebar::Indicator;

    fn agent(
        workspace: &str,
        tab_id: u32,
        pane_id: u32,
        title: &str,
        agent_name: &str,
        session: Option<&str>,
        tab_number: u32,
    ) -> AgentItem {
        AgentItem {
            workspace_id: workspace.to_string(),
            tab_id: Some(tab_id),
            pane_id,
            title: title.to_string(),
            detail: String::new(),
            indicator: Indicator::Working,
            agent_name: agent_name.to_string(),
            session_title: session.map(str::to_string),
            tab_number: Some(tab_number),
        }
    }

    /// 对照 `testShellTabsAlwaysPutEveryLocalTabBeforeRemoteMachines`。
    #[test]
    fn shell_tabs_always_put_every_local_tab_before_remote_machines() {
        let tabs = shell_tabs(&[
            ShellAggregateSource {
                workspace_id: "ssh@@ryzen".to_string(),
                transport: "ryzen".to_string(),
                opened_order: 1,
                tabs: vec![AggregateSourceTab {
                    id: 8,
                    title: "remote".to_string(),
                }],
            },
            ShellAggregateSource {
                workspace_id: "local@@shell".to_string(),
                transport: "local".to_string(),
                opened_order: 9,
                tabs: vec![
                    AggregateSourceTab {
                        id: 2,
                        title: "notes".to_string(),
                    },
                    AggregateSourceTab {
                        id: 4,
                        title: "scratch".to_string(),
                    },
                ],
            },
            ShellAggregateSource {
                workspace_id: "ssh@@build".to_string(),
                transport: "build".to_string(),
                opened_order: 2,
                tabs: vec![AggregateSourceTab {
                    id: 3,
                    title: "build".to_string(),
                }],
            },
        ]);

        assert_eq!(
            tabs.iter()
                .map(|t| t.workspace_id.as_str())
                .collect::<Vec<_>>(),
            vec!["local@@shell", "local@@shell", "ssh@@ryzen", "ssh@@build"]
        );
        assert_eq!(
            tabs.iter().map(|t| t.source_tab_id).collect::<Vec<_>>(),
            vec![2, 4, 8, 3]
        );
        assert_eq!(
            tabs.iter().map(|t| t.display_id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(tabs[0].title, "local · notes");
        assert!(tabs[0].is_local);
        assert!(!tabs[2].is_local);
    }

    /// 对照 `testAgentTabsUseStableWorkspaceAndTopologyOrderInsteadOfStatusOrder`。
    #[test]
    fn agent_tabs_use_stable_workspace_and_topology_order_instead_of_status_order() {
        let agents = vec![
            agent(
                "second",
                22,
                9,
                "server · ryzen",
                "Codex",
                Some("Fix build"),
                2,
            ),
            agent(
                "first",
                11,
                5,
                "muxterm · local",
                "Grok",
                Some("Review UI"),
                1,
            ),
            agent(
                "first",
                11,
                6,
                "muxterm · local",
                "Claude",
                Some("Pair review"),
                1,
            ),
        ];

        let tabs = agent_tabs(&agents, &["first".to_string(), "second".to_string()]);

        assert_eq!(
            tabs.iter()
                .map(|t| t.workspace_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(
            tabs.iter().map(|t| t.source_tab_id).collect::<Vec<_>>(),
            vec![11, 22]
        );
        assert_eq!(
            tabs.iter()
                .map(|t| t.agent_pane_ids.clone())
                .collect::<Vec<_>>(),
            vec![vec![5, 6], vec![9]]
        );
        assert_eq!(
            tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec![
                "muxterm · local · Grok, Claude · Review UI / Pair review",
                "server · ryzen · Codex · Fix build",
            ]
        );
        assert_eq!(
            tabs.iter().map(|t| t.display_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    /// 对照 `testAgentTabIdentityIncludesWorkspaceBecauseTabIdsRepeat`。
    #[test]
    fn agent_tab_identity_includes_workspace_because_tab_ids_repeat() {
        let first = AgentAggregateKey {
            workspace_id: "one".to_string(),
            source_tab_id: 1,
        };
        let second = AgentAggregateKey {
            workspace_id: "two".to_string(),
            source_tab_id: 1,
        };
        let another_tab = AgentAggregateKey {
            workspace_id: "one".to_string(),
            source_tab_id: 2,
        };
        assert_ne!(first, second);
        assert_ne!(first, another_tab);
    }

    /// 对照 `testDeferredAggregateActivationRequiresTheSameProjection`。
    /// 否则会清掉连接页，并把随后完成的 attach 挤成隐藏 Workspace。
    #[test]
    fn deferred_aggregate_activation_requires_the_same_projection() {
        let scheduled = "agent-attach-agent-no-resize";
        assert!(should_activate_deferred(scheduled, Some(scheduled), false));
        assert!(
            !should_activate_deferred(scheduled, Some(scheduled), true),
            ".connecting 期间不得抢可见区，否则 attach 会走隐藏插入"
        );
        assert!(
            !should_activate_deferred(scheduled, Some("another-workspace"), false),
            "用户已切走就不得再激活"
        );
        assert!(
            !should_activate_deferred(scheduled, None, false),
            "已不在聚合投影上就不得激活"
        );
    }

    /// 对照 `testDefaultAggregateShortcutsAndConfigActions`。
    #[test]
    fn default_aggregate_shortcuts_and_config_actions() {
        use crate::frontend::gpui::model::shortcuts::{
            action, action_from_name, builtin_action, KeyAction, KeyChord,
        };
        use std::collections::HashSet;

        let shells = KeyChord::new(true, false, false, true, "s");
        let agents = KeyChord::new(true, false, false, true, "a");
        assert_eq!(builtin_action(&shells), Some(KeyAction::OpenShells));
        assert_eq!(builtin_action(&agents), Some(KeyAction::OpenAgents));
        // 对照 `AppDelegate.buildMenu` 的 `shellsItem`（keyEquivalent "k"）：
        // Swift 的 Cmd-K 就是 Shells，不是空闲键。
        assert_eq!(
            builtin_action(&KeyChord::command("k")),
            Some(KeyAction::OpenShells)
        );
        assert_eq!(action_from_name("open_shells"), Some(KeyAction::OpenShells));
        assert_eq!(action_from_name("open_agents"), Some(KeyAction::OpenAgents));
        // 自定义段也走同一条解析路径。
        assert_eq!(
            action(&agents, None, &HashSet::new()),
            Some(KeyAction::OpenAgents)
        );
    }

    /// 没有 tab 身份的 agent 进不了聚合页，否则会造出无法回跳的一页。
    #[test]
    fn agents_without_tab_identity_are_not_projected() {
        let mut orphan = agent("first", 11, 5, "muxterm · local", "Grok", None, 1);
        orphan.tab_id = None;
        assert!(agent_tabs(&[orphan], &["first".to_string()]).is_empty());
    }

    /// 会话标题与 tab 标题重复时不再追加，避免标题出现两遍。
    #[test]
    fn session_title_matching_tab_title_is_not_repeated() {
        let tabs = agent_tabs(
            &[agent(
                "first",
                11,
                5,
                "muxterm · local",
                "Grok",
                Some("MUXTERM · LOCAL"),
                1,
            )],
            &["first".to_string()],
        );
        assert_eq!(tabs[0].title, "muxterm · local · Grok");
    }
}
