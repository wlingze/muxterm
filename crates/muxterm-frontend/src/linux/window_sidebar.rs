//! Workspace sidebar and capacity actions for the GTK window.
//!
//! The sidebar projects owned ViewStore/activity snapshots and dispatches
//! workspace lifecycle actions. Scene selection remains in `window_scene`;
//! event/config consumption remains in `window_event_pump`.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box, Button, CheckButton, Label, Orientation, Window};

use muxterm_core::protocol::WorkspaceId;

use crate::ffi_client::{ClientOpenIntent, ClientTarget, ClientTask};
use crate::i18n::{self, Key};

use super::window_connection::recent_workspaces;
use super::window_event_pump::{activity_snapshot, sync_view_store};
use super::window_scene::{after_activate, request_switch_tab, show_workspace_scene};
use super::window_status::maybe_refresh_status;
use super::{parse_workspace_id, UiState};

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceCapacityCandidate {
    id: WorkspaceId,
    name: String,
}

/// 超过 soft capacity 时提醒用户选择关闭最久未使用的后台 Workspace。
///
/// 提醒状态按 slot 数量去重：点“全部保留”不会在每个 16ms poll 重复弹窗，
/// 新建或关闭导致数量变化后才重新评估。当前活动 Workspace 只会是 active，
/// 不会进入 `oldest_background_candidates`。
pub(super) fn maybe_warn_workspace_capacity(state: &Rc<RefCell<UiState>>, parent: &Window) {
    const MAX_CANDIDATES: usize = 8;

    let details = {
        let mut s = state.borrow_mut();
        let count = s.view_store.workspace_ids().count();
        let over_capacity = count > s.capacity_limit;
        if !over_capacity || s.capacity_warning_presented_for_slot_count == Some(count) {
            if !over_capacity {
                s.capacity_warning_presented_for_slot_count = None;
            }
            None
        } else {
            let overflow = count.saturating_sub(s.capacity_limit).max(1);
            let active = s.active_workspace_key();
            let mut candidates: Vec<WorkspaceCapacityCandidate> = s
                .view_store
                .workspaces()
                .filter_map(|(_, view)| view.workspace.as_ref())
                .filter(|workspace| active != workspace.id)
                .filter_map(|workspace| {
                    Some(WorkspaceCapacityCandidate {
                        id: parse_workspace_id(&workspace.id)?,
                        name: workspace.name.clone(),
                    })
                })
                .collect();
            candidates.sort_by_key(|candidate| candidate.id.as_str());
            candidates.truncate(overflow.min(MAX_CANDIDATES));
            if candidates.is_empty() {
                None
            } else {
                s.capacity_warning_presented_for_slot_count = Some(count);
                Some((count, s.capacity_limit, candidates))
            }
        }
    };
    let Some((count, limit, candidates)) = details else {
        return;
    };

    let dialog = Window::builder()
        .title(i18n::tr(Key::WorkspaceCapacityTitle))
        .modal(true)
        .transient_for(parent)
        .default_width(500)
        .default_height(300)
        .build();
    let root = Box::new(Orientation::Vertical, 10);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let count_text = count.to_string();
    let limit_text = limit.to_string();
    let message = Label::new(Some(&i18n::tr_args(
        Key::WorkspaceCapacityMessage,
        &[
            ("count", count_text.as_str()),
            ("limit", limit_text.as_str()),
        ],
    )));
    message.set_halign(gtk4::Align::Start);
    message.set_wrap(true);
    root.append(&message);

    let candidate_box = Box::new(Orientation::Vertical, 4);
    let choices: Vec<(WorkspaceCapacityCandidate, CheckButton)> = candidates
        .iter()
        .map(|candidate| {
            let check = CheckButton::with_label(&format_capacity_candidate(candidate));
            (candidate.clone(), check)
        })
        .collect();
    for (_, check) in &choices {
        candidate_box.append(check);
    }
    root.append(&candidate_box);

    let actions = Box::new(Orientation::Horizontal, 8);
    actions.set_halign(gtk4::Align::End);
    let keep = Button::with_label(&i18n::tr(Key::WorkspaceCapacityKeepAll));
    let close_selected = Button::with_label(&i18n::tr(Key::WorkspaceCapacityCloseSelected));
    actions.append(&keep);
    actions.append(&close_selected);
    root.append(&actions);
    dialog.set_child(Some(&root));

    let dialog_for_keep = dialog.clone();
    keep.connect_clicked(move |_| dialog_for_keep.close());

    let dialog_for_close = dialog.clone();
    let state_for_close = state.clone();
    close_selected.connect_clicked(move |_| {
        let ids: Vec<WorkspaceId> = choices
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(candidate, _)| candidate.id.clone())
            .collect();
        let mut s = state_for_close.borrow_mut();
        for id in ids {
            close_sidebar_workspace(&mut s, &id);
        }
        s.capacity_warning_presented_for_slot_count =
            if s.view_store.workspace_ids().count() > s.capacity_limit {
                Some(s.view_store.workspace_ids().count())
            } else {
                None
            };
        dialog_for_close.close();
    });

    dialog.present();
}

fn format_capacity_candidate(candidate: &WorkspaceCapacityCandidate) -> String {
    let target = candidate
        .id
        .alias
        .as_deref()
        .filter(|alias| !alias.is_empty())
        .unwrap_or("local");
    format!("{} · {} @ {}", candidate.name, candidate.id.runtime, target)
}

pub(super) fn close_sidebar_workspace(s: &mut UiState, id: &WorkspaceId) {
    let mut ordered: Vec<WorkspaceId> = s
        .view_store
        .workspaces()
        .filter_map(|(_, view)| view.workspace.as_ref())
        .filter_map(|workspace| parse_workspace_id(&workspace.id))
        .collect();
    ordered.sort_by_key(|workspace| workspace.as_str());
    let Some(index) = ordered.iter().position(|candidate| candidate == id) else {
        return;
    };
    let was_active = s.active_ws_id() == *id;
    let fallback = if was_active {
        ordered
            .get(index + 1)
            .or_else(|| {
                index
                    .checked_sub(1)
                    .and_then(|previous| ordered.get(previous))
            })
            .cloned()
    } else {
        None
    };
    let workspace_key = id.as_str();
    if let Err(error) = s.event_pump.client().close_workspace(&workspace_key) {
        tracing::warn!(
            target = "muxterm::linux",
            %error,
            workspace = %workspace_key,
            "workspace close failed"
        );
        return;
    }

    let replica_id = id.replica_id();
    s.last_seen
        .retain(|(workspace, _), _| workspace != &replica_id);
    s.surface_input_queue
        .borrow_mut()
        .retain(|input| &input.workspace != id);
    s.scenes.remove(id);
    s.view_store.remove_workspace(&workspace_key);
    s.visible_tabs.remove(&workspace_key);
    s.local_tab_overrides.remove(&workspace_key);
    if s.mounted_ws.as_ref() == Some(id) {
        s.mounted_ws = None;
    }
    s.workspace_sockets.remove(id);
    if let Err(error) = sync_view_store(s) {
        tracing::warn!(target = "muxterm::linux", %error, "workspace list refresh failed after close");
    }
    let recents = recent_workspaces(
        &s.view_store,
        &s.workspace_sockets,
        s.view_store.workspace_ids().count(),
    );
    s.qc_store.replace_all_recents(&recents);

    if was_active {
        if let Some(fallback) = fallback {
            let fallback_key = fallback.as_str();
            let _ = sync_view_store(s);
            if s.view_store.workspace(&fallback_key).is_some() {
                show_workspace_scene(s, fallback, false);
            }
        } else {
            // 主窗口始终需要一个可轮询的前台 Workspace。关闭最后一格时
            // 立即回到一格空本地 shell；Core 负责旧 Runtime 的 detach/
            // shutdown 语义，GTK 只提交产品级 target。
            let target = ClientTarget {
                name: "shell".into(),
                runtime: "shell".into(),
                transport: "local".into(),
                target: None,
                path: String::new(),
                session: None,
                socket: None,
            };
            if let Err(error) = s
                .event_pump
                .client()
                .open_target(&target, ClientOpenIntent::CreateIfMissing)
            {
                tracing::error!(
                    target = "muxterm::linux",
                    "关闭最后工作区后创建本地 shell 失败: {error}"
                );
                return;
            }
            let _ = sync_view_store(s);
        }
        after_activate(s);
    } else {
        refresh_sidebar_if_open(s);
        maybe_refresh_status(s, true);
    }
}

pub(super) fn activate_sidebar_activity(s: &mut UiState, id: &WorkspaceId, pane: u32) {
    if s.active_ws_id() != *id {
        let workspace_key = id.as_str();
        if s.view_store.workspace(&workspace_key).is_none() {
            return;
        }
        show_workspace_scene(s, id.clone(), false);
    }
    let workspace_key = s.active_workspace_key();
    let tab = {
        s.view_store
            .workspace(&workspace_key)
            .and_then(|view| {
                view.tabs.iter().find(|tab| {
                    view.panes
                        .get(&tab.id)
                        .is_some_and(|panes| panes.iter().any(|candidate| candidate.id == pane))
                })
            })
            .map(|tab| tab.id)
    };
    if let Some(tab) = tab {
        if tab != s.active_tab {
            request_switch_tab(s, tab);
        }
        let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id: pane });
    }
    let _ = s
        .event_pump
        .client()
        .workspace_attention_acknowledge(&workspace_key, pane);
    acknowledge_compatibility_attention(s, &id.replica_id(), pane);
    refresh_sidebar_if_open(s);
}

fn acknowledge_compatibility_attention(s: &mut UiState, workspace_id: &str, pane: u32) {
    s.compatibility_activity.acknowledge(workspace_id, pane);
}

pub(super) fn refresh_sidebar_if_open(s: &mut UiState) {
    if !s.sidebar.is_open() {
        return;
    }
    let activity = activity_snapshot(s);
    let active_workspace = s.active_workspace_key();
    s.sidebar
        .refresh_from_views(&s.view_store, Some(&active_workspace), &activity);
}

pub(super) fn refresh_sidebar_workspaces_if_open(s: &UiState) {
    if s.sidebar.is_open() {
        let active_workspace = s.active_workspace_key();
        s.sidebar
            .refresh_workspaces_from_views(&s.view_store, Some(&active_workspace));
    }
}
