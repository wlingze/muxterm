//! GTK 主窗口的 EventPump 与 CommandQueue 适配。
//!
//! Core 事件只能由 GTK poll owner 消费，UI 动作只把命令放入队列；本模块
//! 集中维护这两个边界以及 activity 快照的前端兼容合并。

use std::collections::HashSet;

use crate::frontend::command_queue::ClientCommand;
use crate::frontend::utils::corebridge::{
    ClientActivitySnapshot, ClientAttentionPane, ClientWorkspaceAttention, ClientWorkspaceEvent,
};

use super::window_actions;
use super::{SurfaceInput, UiState};

/// Flush commands only from the GTK event-loop owner.
pub(super) fn flush_command_queue(s: &UiState) -> Vec<i32> {
    let client = s.event_pump.client();
    let results = s.command_queue.borrow_mut().flush(client);
    for code in results.iter().copied().filter(|code| *code != 0) {
        tracing::warn!(
            target = "muxterm::linux",
            code,
            "Core command queue dispatch failed"
        );
    }
    results
}

pub(super) fn enqueue_workspace_input(
    s: &UiState,
    workspace_id: &str,
    pane_id: u32,
    data: &[u8],
    quiet: bool,
) {
    s.command_queue.borrow_mut().push(ClientCommand::Input {
        workspace_id: Some(workspace_id.to_owned()),
        pane_id,
        data: data.to_vec(),
        quiet,
    });
}

pub(super) fn enqueue_workspace_resize(
    s: &UiState,
    workspace_id: &str,
    pane_id: Option<u32>,
    cols: u16,
    rows: u16,
) {
    s.command_queue.borrow_mut().push(ClientCommand::Resize {
        workspace_id: Some(workspace_id.to_owned()),
        pane_id,
        cols,
        rows,
    });
}

/// Drain VTE input callbacks on the production GTK poll.
///
/// The queue is deliberately independent from `UiState`: PaneView callbacks
/// can outlive the layout pass that installed them, so they must never borrow
/// or mutate the state directly. An input whose workspace/pane disappeared is
/// dropped with a diagnostic instead of being redirected to the active pane.
pub(super) fn drain_surface_input(s: &mut UiState) {
    let pending = take_surface_input(&s.surface_input_queue);
    let mut pending = pending.into_iter().peekable();
    while let Some(first) = pending.next() {
        // GTK/VTE emits one commit for each typed character. Keep adjacent
        // commits for the same owner together so a newly-created tmux pane
        // receives one ordered write instead of a burst of independent
        // control-mode commands that can race pane startup.
        let workspace_id = first.workspace.clone();
        let pane_id = first.pane;
        let mut data = first.data;
        while let Some(next) = pending.peek() {
            if next.workspace != workspace_id || next.pane != pane_id {
                break;
            }
            let next = pending.next().expect("peeked SurfaceInput");
            data.extend_from_slice(&next.data);
        }
        s.last_raw_input = data.clone();
        let workspace_key = workspace_id.as_str();
        enqueue_workspace_input(s, &workspace_key, pane_id.0, &data, false);
    }
}

pub(super) fn take_surface_input(
    queue: &std::rc::Rc<std::cell::RefCell<std::collections::VecDeque<SurfaceInput>>>,
) -> Vec<SurfaceInput> {
    queue.borrow_mut().drain(..).collect()
}

pub(super) fn poll_event_store(s: &mut UiState) -> Vec<ClientWorkspaceEvent> {
    let event_pump = &s.event_pump;
    let events = event_pump.poll_into_with_events(&mut s.view_store);
    poll_config_events(s);
    events
}

/// Drain Core configuration events through EventPump and hot-apply committed
/// or reloaded frontend settings. Preview events stay owned by the settings
/// overlay until its transaction commits.
fn poll_config_events(s: &mut UiState) {
    let changed = s
        .event_pump
        .poll_config_events()
        .iter()
        .any(|event| event.changes_values());
    if !changed {
        return;
    }
    match s.event_pump.client().config_describe() {
        Ok(snapshot) => window_actions::apply_config_snapshot(s, snapshot),
        Err(error) => tracing::warn!(
            target = "muxterm::config",
            %error,
            "读取配置变更快照失败，跳过热应用"
        ),
    }
}

pub(super) fn sync_view_store(s: &mut UiState) -> anyhow::Result<usize> {
    let (event_pump, view_store) = (&s.event_pump, &mut s.view_store);
    event_pump.sync_view_store(view_store)
}

/// Read Core-owned activity state and merge the small compatibility fixture
/// used by GTK integration hooks. Production attention state is owned by
/// Core; the local engine only has entries when a test deliberately injects
/// bytes or an authoritative status through an AppWindow test hook.
pub(super) fn activity_snapshot(s: &UiState) -> ClientActivitySnapshot {
    let mut snapshot = s
        .event_pump
        .client()
        .activity_snapshot()
        .unwrap_or_default();
    let core_blocked_workspace_ids: HashSet<String> = snapshot
        .workspaces
        .iter()
        .filter(|workspace| workspace.blocked > 0)
        .map(|workspace| workspace.workspace_id.clone())
        .collect();
    let mut compatibility_workspace_ids = HashSet::new();

    for workspace in s.compatibility_activity.snapshot() {
        let mut has_compatibility_state = false;
        let target = snapshot
            .workspaces
            .iter_mut()
            .find(|item| item.workspace_id == workspace.workspace_id);
        let target = if let Some(target) = target {
            target
        } else {
            snapshot.workspaces.push(ClientWorkspaceAttention {
                workspace_id: workspace.workspace_id.clone(),
                path: String::new(),
                blocked: 0,
                done: 0,
                working: 0,
                panes: Vec::new(),
            });
            snapshot
                .workspaces
                .last_mut()
                .expect("刚插入的 activity workspace 必须存在")
        };
        for pane in workspace.panes {
            let pane_has_compatibility_state = pane.status != "unknown"
                || !pane.last_line.is_empty()
                || pane.seq != 0
                || pane.process_name.is_some()
                || pane.process_is_agent
                || pane.agent_name.is_some()
                || pane.shell_name.is_some();
            if !pane_has_compatibility_state {
                continue;
            }
            has_compatibility_state = true;
            if let Some(existing) = target
                .panes
                .iter_mut()
                .find(|item| item.pane_id == pane.pane_id)
            {
                *existing = pane;
            } else {
                target.panes.push(pane);
            }
        }
        if has_compatibility_state {
            compatibility_workspace_ids.insert(workspace.workspace_id.clone());
        }
    }

    for workspace in &mut snapshot.workspaces {
        if !compatibility_workspace_ids.contains(&workspace.workspace_id) {
            continue;
        }
        workspace.blocked = workspace
            .panes
            .iter()
            .filter(|pane| pane.status == "blocked" && !pane.acknowledged)
            .count();
        workspace.done = workspace
            .panes
            .iter()
            .filter(|pane| pane.status == "done" && !pane.acknowledged)
            .count();
        workspace.working = workspace
            .panes
            .iter()
            .filter(|pane| pane.status == "working")
            .count();
    }
    let compatibility_blocked_count = snapshot
        .workspaces
        .iter()
        .filter(|workspace| {
            compatibility_workspace_ids.contains(&workspace.workspace_id)
                && workspace.blocked > 0
                && !core_blocked_workspace_ids.contains(&workspace.workspace_id)
        })
        .count();
    snapshot.blocked_count = snapshot
        .blocked_count
        .saturating_add(compatibility_blocked_count);
    snapshot
}

pub(super) fn panel_attention_rows(snapshot: &ClientActivitySnapshot) -> Vec<ClientAttentionPane> {
    snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panes.iter())
        .cloned()
        .collect()
}
