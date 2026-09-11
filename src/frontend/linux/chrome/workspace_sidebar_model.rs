//! Owned sidebar row models and activity aggregation.
//!
//! This module contains no GTK widgets; it projects Core-owned topology and
//! activity DTOs into the rows consumed by the sidebar view.

use std::collections::HashMap;

use crate::frontend::linux::view_store::ViewStore;
use crate::frontend::utils::corebridge::{
    ClientActivitySnapshot, ClientAttentionPane, ClientWorkspace,
};
use crate::protocol::WorkspaceId;

/// A workspace row in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSidebarItem {
    pub id: WorkspaceId,
    pub name: String,
    pub runtime: String,
    pub transport: String,
    pub active: bool,
    /// Ctrl+Alt+N 快捷编号；目前只暴露固定顺序的前五个 workspace。
    pub shortcut: Option<u8>,
}

impl WorkspaceSidebarItem {
    /// Build sidebar rows from the frontend-owned workspace snapshot.
    pub fn from_views(store: &ViewStore) -> Vec<Self> {
        Self::from_views_with_active(store, store.active_workspace_id())
    }

    /// Build rows using the frontend-visible workspace instead of Core's
    /// transport-level active marker.  Scene switching is local and must not
    /// rewrite the Core snapshot just to update sidebar selection.
    pub fn from_views_with_active(store: &ViewStore, active_id: Option<&str>) -> Vec<Self> {
        let mut workspaces: Vec<&ClientWorkspace> = store
            .workspaces()
            .filter_map(|(_, view)| view.workspace.as_ref())
            .collect();
        workspaces.sort_by(|left, right| left.id.cmp(&right.id));
        workspaces
            .into_iter()
            .enumerate()
            .filter_map(|(index, workspace)| {
                let id = parse_workspace_id(&workspace.id)?;
                Some(Self {
                    id,
                    name: workspace.name.clone(),
                    runtime: workspace.runtime.clone(),
                    transport: transport_label(&workspace.id),
                    active: active_id == Some(workspace.id.as_str()),
                    shortcut: (index < 5).then_some((index + 1) as u8),
                })
            })
            .collect()
    }
}

/// Agent、Command 与 Attention 共用的状态点语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityIndicator {
    Running,
    Done,
    None,
}

/// 跨全部 Workspace 汇总的一条 agent。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSidebarItem {
    pub workspace_id: WorkspaceId,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: ActivityIndicator,
}

impl AgentSidebarItem {
    /// Build agent rows from owned topology and Core's owned activity DTO.
    pub fn from_views(store: &ViewStore, activity: &ClientActivitySnapshot) -> Vec<Self> {
        let by_pane = activity_by_pane(activity);
        let mut items = Vec::new();
        for (workspace_key, view) in store.workspaces() {
            let Some(workspace) = view.workspace.as_ref() else {
                continue;
            };
            let Some(workspace_id) = parse_workspace_id(workspace_key) else {
                continue;
            };
            let activity_key = workspace_id.replica_id();
            for panes in view.panes.values() {
                for pane in panes {
                    let Some(attention) = by_pane.get(&(activity_key.as_str(), pane.id)) else {
                        continue;
                    };
                    let agent_name = attention.agent_name.as_deref().or_else(|| {
                        attention
                            .process_is_agent
                            .then_some(attention.process_name.as_deref())
                            .flatten()
                    });
                    let Some(agent_name) = agent_name else {
                        continue;
                    };
                    items.push(Self {
                        workspace_id: workspace_id.clone(),
                        pane_id: pane.id,
                        title: agent_name.to_string(),
                        detail: client_activity_detail(workspace, attention),
                        indicator: client_attention_indicator(attention),
                    });
                }
            }
        }
        items
    }
}

/// 跨全部 Workspace 汇总的一条正在运行或尚未阅读的非 agent 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSidebarItem {
    pub workspace_id: WorkspaceId,
    pub pane_id: u32,
    pub title: String,
    pub detail: String,
    pub indicator: ActivityIndicator,
}

impl CommandSidebarItem {
    /// Build command rows from the owned activity projection.  A command row
    /// must have Core's process name; pane titles are never used as a command
    /// fallback because they are presentation text, not activity identity.
    pub fn from_views(store: &ViewStore, activity: &ClientActivitySnapshot) -> Vec<Self> {
        let by_pane = activity_by_pane(activity);
        let mut items = Vec::new();
        for (workspace_key, view) in store.workspaces() {
            let Some(workspace) = view.workspace.as_ref() else {
                continue;
            };
            let Some(workspace_id) = parse_workspace_id(workspace_key) else {
                continue;
            };
            let activity_key = workspace_id.replica_id();
            for panes in view.panes.values() {
                for pane in panes {
                    let Some(attention) = by_pane.get(&(activity_key.as_str(), pane.id)) else {
                        continue;
                    };
                    let active = matches!(attention.status.as_str(), "working")
                        || (matches!(attention.status.as_str(), "blocked" | "done")
                            && !attention.acknowledged);
                    if !active || attention.process_is_agent {
                        continue;
                    }
                    let Some(title) = attention
                        .process_name
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    else {
                        continue;
                    };
                    items.push(Self {
                        workspace_id: workspace_id.clone(),
                        pane_id: pane.id,
                        title: title.to_string(),
                        detail: client_activity_detail(workspace, attention),
                        indicator: client_attention_indicator(attention),
                    });
                }
            }
        }
        items
    }
}

fn parse_workspace_id(value: &str) -> Option<WorkspaceId> {
    let mut parts = value.splitn(5, '/');
    let transport = parts.next()?;
    let alias = parts.next()?;
    let session = parts.next()?;
    let runtime = parts.next()?;
    let path = parts.next().unwrap_or_default();
    if transport.is_empty() || runtime.is_empty() {
        return None;
    }
    Some(WorkspaceId::new(
        transport,
        (!alias.is_empty()).then_some(alias),
        session,
        runtime,
        path,
    ))
}

fn transport_label(value: &str) -> String {
    parse_workspace_id(value)
        .map(|id| {
            if id.transport == "ssh" {
                id.alias.unwrap_or_else(|| "ssh".into())
            } else {
                "local".into()
            }
        })
        .unwrap_or_else(|| "local".into())
}

fn activity_by_pane(
    activity: &ClientActivitySnapshot,
) -> HashMap<(&str, u32), &ClientAttentionPane> {
    activity
        .workspaces
        .iter()
        .flat_map(|workspace| {
            workspace
                .panes
                .iter()
                .map(move |pane| (workspace.workspace_id.as_str(), pane.pane_id, pane))
        })
        .map(|(workspace, pane, attention)| ((workspace, pane), attention))
        .collect()
}

fn client_attention_indicator(attention: &ClientAttentionPane) -> ActivityIndicator {
    match attention.status.as_str() {
        "working" => ActivityIndicator::Running,
        "blocked" | "done" if !attention.acknowledged => ActivityIndicator::Done,
        _ => ActivityIndicator::None,
    }
}

fn client_activity_detail(workspace: &ClientWorkspace, attention: &ClientAttentionPane) -> String {
    let identity = parse_workspace_id(&workspace.id)
        .map(|id| {
            let transport = if id.transport == "ssh" {
                id.alias.unwrap_or_else(|| "ssh".into())
            } else {
                "local".into()
            };
            format!("{}@{}@{}", workspace.name, workspace.runtime, transport)
        })
        .unwrap_or_else(|| workspace.name.clone());
    let path = attention
        .last_line
        .trim()
        .is_empty()
        .then(|| parse_workspace_id(&workspace.id).map(|id| id.path))
        .flatten()
        .filter(|path| !path.trim().is_empty());
    path.map(|path| format!("{identity} · {path}"))
        .unwrap_or(identity)
}
