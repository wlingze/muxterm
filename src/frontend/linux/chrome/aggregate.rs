//! 固定聚合视图只保存源 Tab 身份，显示时复用源 Scene 与 Surface。

use std::collections::HashSet;

use crate::frontend::linux::view_store::ViewStore;
use crate::frontend::linux::workspace_sidebar::AgentSidebarItem;
use crate::frontend::utils::corebridge::ClientActivitySnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Shells,
    Agents,
}

impl AggregateKind {
    pub fn label(self) -> String {
        use crate::frontend::utils::i18n::{self, Key};
        match self {
            Self::Shells => format!("S · {}", i18n::tr(Key::AggregateShells)),
            Self::Agents => format!("A · {}", i18n::tr(Key::AggregateAgents)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceTab {
    pub workspace: String,
    pub tab: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateTab {
    pub source: SourceTab,
    pub title: String,
}

#[derive(Default)]
pub struct AggregateSelection {
    pub kind: Option<AggregateKind>,
    pub shell_last: Option<SourceTab>,
    pub agent_last: Option<SourceTab>,
    pub hidden_agents: HashSet<SourceTab>,
}

impl AggregateSelection {
    pub fn remember(&mut self, source: SourceTab) {
        match self.kind {
            Some(AggregateKind::Shells) => self.shell_last = Some(source),
            Some(AggregateKind::Agents) => self.agent_last = Some(source),
            None => {}
        }
    }

    pub fn selected<'a>(&self, tabs: &'a [AggregateTab]) -> Option<&'a AggregateTab> {
        let last = match self.kind {
            Some(AggregateKind::Shells) => &self.shell_last,
            _ => &self.agent_last,
        };
        tabs.iter()
            .find(|tab| Some(&tab.source) == last.as_ref())
            .or_else(|| tabs.first())
    }
}

pub fn project(
    kind: AggregateKind,
    store: &ViewStore,
    activity: &ClientActivitySnapshot,
    hidden: &HashSet<SourceTab>,
) -> Vec<AggregateTab> {
    let agents = AgentSidebarItem::from_views(store, activity);
    let mut sources: Vec<_> = store.workspaces().collect();
    if kind == AggregateKind::Shells {
        sources.sort_by_key(|(_, view)| {
            view.workspace
                .as_ref()
                .is_none_or(|workspace| !workspace.id.starts_with("local/"))
        });
    }
    sources
        .into_iter()
        .flat_map(|(workspace_key, view)| {
            view.tabs
                .iter()
                .filter_map(|tab| {
                    let workspace = view.workspace.as_ref()?;
                    let source = SourceTab {
                        workspace: workspace_key.to_owned(),
                        tab: tab.id,
                    };
                    let included = match kind {
                        AggregateKind::Shells => workspace.runtime == "shell",
                        AggregateKind::Agents => {
                            !hidden.contains(&source)
                                && agents.iter().any(|agent| {
                                    agent.workspace_id.as_str() == workspace_key
                                        && view.panes.get(&tab.id).is_some_and(|panes| {
                                            panes.iter().any(|pane| pane.id == agent.pane_id)
                                        })
                                })
                        }
                    };
                    included.then(|| AggregateTab {
                        source,
                        title: format!("{} · {}", workspace.name, tab.name),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_keeps_full_source_identity_and_falls_back_only_when_missing() {
        let tabs = vec![
            AggregateTab {
                source: SourceTab {
                    workspace: "local".into(),
                    tab: 1,
                },
                title: "local".into(),
            },
            AggregateTab {
                source: SourceTab {
                    workspace: "remote".into(),
                    tab: 1,
                },
                title: "remote".into(),
            },
        ];
        let mut selection = AggregateSelection {
            kind: Some(AggregateKind::Agents),
            ..Default::default()
        };
        selection.remember(tabs[1].source.clone());
        assert_eq!(selection.selected(&tabs), Some(&tabs[1]));
        assert_eq!(selection.selected(&tabs[..1]), Some(&tabs[0]));
        assert_eq!(selection.selected(&[]), None);
    }
}
