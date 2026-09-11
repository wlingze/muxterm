//! Render-mailbox and event-driven projection for resident Linux scenes.

use std::collections::HashSet;

use crate::frontend::command_queue::ClientCommand;
use crate::frontend::linux::view_store::PaneRenderPolicy;
use crate::frontend::utils::corebridge::ClientTask;
use crate::protocol::WorkspaceId;

use super::super::view_store::WorkspaceView;
use super::window_event_pump::enqueue_workspace_input;
use super::window_layout::refresh_workspace_layout;
use super::window_scene::show_workspace_scene;
use super::window_surface::{drain_view_store_render_events, seed_unseeded_pane_for};
use super::{
    active_workspace_key, parse_workspace_id, resident_pane_view, ClientRuntimeCapability,
    ClientWorkspaceEvent, UiState,
};

/// Consume every opened scene's render mailbox without recapturing hidden
/// workspaces. Resident surfaces keep feeding while their scene is hidden.
pub(super) fn sync_pane_outputs(s: &mut UiState) {
    let workspace_ids: Vec<String> = s.view_store.workspace_ids().map(str::to_string).collect();
    for workspace_key in workspace_ids {
        let Some(wid) = parse_workspace_id(&workspace_key) else {
            continue;
        };
        let panes: Vec<(u32, u16, u16)> = s
            .view_store
            .workspace(&workspace_key)
            .map(|view| {
                view.panes
                    .values()
                    .flat_map(|panes| panes.iter())
                    .map(|pane| (pane.id, pane.cols, pane.rows))
                    .collect()
            })
            .unwrap_or_default();
        for (pane_id, cols, rows) in panes {
            let Some(view) = resident_pane_view(s, &wid, pane_id) else {
                continue;
            };
            view.ensure_grid_size(cols, rows);
            seed_unseeded_pane_for(s, &wid, &view, pane_id, cols, rows);
            if view.is_seeded() {
                if let Some(bytes) = s
                    .view_store
                    .take_pane_resume_baseline(&workspace_key, pane_id)
                {
                    view.feed_full(&bytes);
                }
                drain_view_store_render_events(s, &wid, &view, pane_id);
                forward_parser_replies_for_key(s, &workspace_key, pane_id);
            }
        }
    }
}

/// Assign render delivery tiers from Scene visibility and transport cost.
///
/// Visible scenes stay live. Hidden SSH scenes pause incremental output and
/// resume from an asynchronous baseline; other hidden scenes coalesce adjacent
/// output without dropping bytes.
pub(super) fn sync_render_policies(s: &mut UiState) {
    let visible_workspace = s.active_workspace_key();
    let targets: Vec<(String, u32, PaneRenderPolicy)> = s
        .view_store
        .workspaces()
        .flat_map(|(workspace_id, view)| {
            let policy = if workspace_id == visible_workspace {
                PaneRenderPolicy::Live
            } else {
                hidden_render_policy(view)
            };
            view.panes
                .values()
                .flat_map(|panes| panes.iter())
                .map(move |pane| (workspace_id.to_string(), pane.id, policy))
        })
        .collect();

    for (workspace_id, pane_id, policy) in targets {
        let previous = s
            .view_store
            .set_pane_render_policy(&workspace_id, pane_id, policy);
        if previous == PaneRenderPolicy::Pause && policy != PaneRenderPolicy::Pause {
            s.command_queue.borrow_mut().push(ClientCommand::Task {
                workspace_id: Some(workspace_id),
                task: ClientTask::RequestPaneSnapshot { pane_id },
            });
        }
    }
}

fn hidden_render_policy(view: &WorkspaceView) -> PaneRenderPolicy {
    let transport = view
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.resolved_target.as_ref())
        .and_then(|target| target.pointer("/canonical/transport"))
        .and_then(serde_json::Value::as_str);
    if transport == Some("ssh") {
        PaneRenderPolicy::Pause
    } else {
        PaneRenderPolicy::Coalesce
    }
}

pub(super) fn refresh_event_workspaces(s: &mut UiState, events: &[ClientWorkspaceEvent]) {
    // A Core task (for example NewTab/CloseTab) may change its authoritative
    // active tab. The status-bar click path never emits this event because it
    // only changes the frontend-visible Scene.
    let core_active_tab_workspaces: HashSet<String> = events
        .iter()
        .filter(|event| event.event.type_ == crate::protocol::ffi::types::STATE_ACTIVE_TAB_CHANGED)
        .map(|event| event.workspace_id.clone())
        .collect();
    for workspace_key in core_active_tab_workspaces {
        if s.local_tab_overrides.contains(&workspace_key) {
            continue;
        }
        let active = s
            .view_store
            .workspace(&workspace_key)
            .and_then(|view| view.tabs.iter().find(|tab| tab.is_active).map(|tab| tab.id));
        if let Some(active) = active {
            s.visible_tabs.insert(workspace_key, active);
        } else {
            s.visible_tabs.remove(&workspace_key);
        }
    }

    let workspace_ids: Vec<WorkspaceId> = events
        .iter()
        .filter(|event| event.event.is_topology())
        .filter_map(|event| parse_workspace_id(&event.workspace_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    for workspace_id in workspace_ids {
        refresh_workspace_layout(s, &workspace_id, true);
    }
    repair_visible_workspace(s);
    apply_attention_visibility_events(s, events);
    mark_active_attention_visible(s);
}

pub(super) fn repair_visible_workspace(s: &mut UiState) {
    let visible_key = s.active_workspace_key();
    if s.view_store.workspace(&visible_key).is_some() {
        return;
    }
    let fallback = s
        .view_store
        .active_workspace_id()
        .and_then(parse_workspace_id)
        .or_else(|| s.view_store.workspace_ids().find_map(parse_workspace_id));
    if let Some(fallback) = fallback {
        show_workspace_scene(s, fallback, true);
    }
}

pub(super) fn apply_attention_visibility_events(s: &UiState, events: &[ClientWorkspaceEvent]) {
    let active_workspace = s.active_workspace_key();
    for event in events
        .iter()
        .filter(|event| event.workspace_id == active_workspace)
    {
        let pane = match event.event.type_ {
            crate::protocol::ffi::types::STATE_ACTIVE_PANE_CHANGED => Some(event.event.pane_id),
            crate::protocol::ffi::types::STATE_PANE_OUTPUT
            | crate::protocol::ffi::types::STATE_PANE_FRAME
            | crate::protocol::ffi::types::STATE_PANE_SNAPSHOT
            | crate::protocol::ffi::types::STATE_PANE_HISTORY
            | crate::protocol::ffi::types::STATE_PANE_AGENT_CHANGED
            | crate::protocol::ffi::types::STATE_STATUS_SUBSCRIPTION
                if event.event.pane_id == s.active_pane =>
            {
                Some(event.event.pane_id)
            }
            _ => None,
        };
        let Some(pane) = pane else {
            continue;
        };
        let _ = s
            .event_pump
            .client()
            .workspace_attention_on_became_visible(&event.workspace_id, pane);
    }
}

pub(super) fn mark_active_attention_visible(s: &UiState) {
    let workspace_id = active_workspace_key(s);
    if workspace_id.is_empty() {
        return;
    }
    let _ = s
        .event_pump
        .client()
        .workspace_attention_on_became_visible(&workspace_id, s.active_pane);
}

pub(super) fn sync_pane_grid_size(s: &UiState, pane_id: u32) {
    let Some(view) = s.active_layout().pane(pane_id) else {
        return;
    };
    let workspace_key = s.active_ws_id().as_str();
    let active_tab = s.active_tab_id();
    let Some(pane) = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|workspace| workspace.panes.get(&active_tab))
        .and_then(|panes| panes.iter().find(|pane| pane.id == pane_id))
    else {
        return;
    };
    view.ensure_grid_size(pane.cols, pane.rows);
}

/// 按 `(WorkspaceId, PaneId)` 对齐字符格（hidden tab / background 也适用）。
pub(super) fn sync_pane_grid_size_for(s: &UiState, wid: &WorkspaceId, pane_id: u32) {
    let Some(view) = resident_pane_view(s, wid, pane_id) else {
        return;
    };
    let workspace_key = wid.as_str();
    let (cols, rows) = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|workspace| {
            workspace
                .panes
                .values()
                .flat_map(|panes| panes.iter())
                .find(|pane| pane.id == pane_id)
        })
        .map(|pane| (pane.cols, pane.rows))
        .unwrap_or((80, 24));
    view.ensure_grid_size(cols, rows);
}

pub(super) fn forward_parser_replies(s: &mut UiState, pane_id: u32) {
    let workspace_id = active_workspace_key(s);
    forward_parser_replies_for_key(s, &workspace_id, pane_id);
}

/// 按 WorkspaceId 转发 parser replies（background workspace 也 flush）。
pub(super) fn forward_parser_replies_for(s: &mut UiState, wid: &WorkspaceId, pane_id: u32) {
    let workspace_id = wid.as_str();
    forward_parser_replies_for_key(s, &workspace_id, pane_id);
}

pub(super) fn forward_parser_replies_for_key(s: &mut UiState, workspace_id: &str, pane_id: u32) {
    // tmux/SSH mirror 的远端 Runtime 已经负责 query reply；把 GTK 无头
    // parser 的应答写回会把 OSC/DA 字节泄漏到用户 shell。
    if s.workspace_supports(workspace_id, ClientRuntimeCapability::SharedClientResize) {
        return;
    }
    let replies = s
        .event_pump
        .client()
        .take_workspace_pane_reply(workspace_id, pane_id);
    if replies.is_empty() {
        return;
    }
    enqueue_workspace_input(s, workspace_id, pane_id, &replies, false);
}

#[cfg(test)]
mod tests {
    use super::hidden_render_policy;
    use crate::frontend::linux::view_store::{PaneRenderPolicy, WorkspaceView};
    use crate::frontend::utils::corebridge::ClientWorkspace;

    fn workspace_with_transport(transport: &str) -> WorkspaceView {
        let mut view = WorkspaceView::default();
        view.workspace = Some(ClientWorkspace {
            id: "local//test/shell/".into(),
            name: "test".into(),
            runtime: "shell".into(),
            active: false,
            resolved_target: Some(serde_json::json!({
                "canonical": { "transport": transport }
            })),
        });
        view
    }

    #[test]
    fn hidden_ssh_uses_pause_policy() {
        assert_eq!(
            hidden_render_policy(&workspace_with_transport("ssh")),
            PaneRenderPolicy::Pause
        );
    }

    #[test]
    fn hidden_local_uses_byte_preserving_coalesce_policy() {
        assert_eq!(
            hidden_render_policy(&workspace_with_transport("local")),
            PaneRenderPolicy::Coalesce
        );
    }
}
