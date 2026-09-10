//! Workspace / tab Scene navigation for the GTK window.
//!
//! This module owns only frontend-local scene selection. Core mutations stay
//! at the lifecycle call sites in `window.rs`; changing the visible scene must
//! not query or activate a Workspace through FFI.

use std::time::{Duration, Instant};

use gtk4::prelude::*;
use muxterm_protocol::WorkspaceId;

use super::window_actions::{handle_pane_menu_action, report_all_pane_colours};
use super::window_layout::{refresh_ui, refresh_workspace_layout};
use super::window_render::mark_active_attention_visible;
use super::window_sidebar::{refresh_sidebar_if_open, refresh_sidebar_workspaces_if_open};
use super::window_status::{maybe_refresh_status, sync_chrome_visibility};
use super::{active_workspace_key, parse_workspace_id, recent_target_configs, LayoutHost, UiState};

pub(super) fn switch_tab_n(s: &mut UiState, n: usize) {
    let workspace_id = active_workspace_key(s);
    if let Some(tab_id) = s
        .view_store
        .workspace(&workspace_id)
        .and_then(|view| view.tabs.get(n.saturating_sub(1)).map(|tab| tab.id))
    {
        request_switch_tab(s, tab_id);
    }
}

pub(super) fn switch_workspace_n(s: &mut UiState, n: usize) {
    let target = s
        .view_store
        .workspaces()
        .nth(n.saturating_sub(1))
        .and_then(|(_, view)| view.workspace.as_ref())
        .and_then(|workspace| parse_workspace_id(&workspace.id));
    if let Some(target) = target {
        if s.active_ws_id() != target {
            activate_existing(s, target);
        }
    }
}

/// 切 tab 只显示已经常驻的 GTK Stack page，不通知 Core。
pub(super) fn show_tab_scene(s: &mut UiState, tab_id: u32) -> bool {
    let workspace_id = s.active_ws_id();
    let workspace_key = workspace_id.as_str();
    let active_pane = s
        .view_store
        .workspace(&workspace_key)
        .and_then(|view| view.panes.get(&tab_id))
        .and_then(|panes| {
            panes
                .iter()
                .find(|pane| pane.is_active)
                .or_else(|| panes.first())
        })
        .map(|pane| pane.id);

    if s.view_store
        .workspace(&workspace_key)
        .is_none_or(|view| !view.tabs.iter().any(|tab| tab.id == tab_id))
    {
        return false;
    }

    s.visible_tabs.insert(workspace_key, tab_id);
    s.local_tab_overrides
        .insert(workspace_id.as_str().to_owned());
    s.active_tab = tab_id;
    if let Some(pane) = active_pane {
        s.active_pane = pane;
    }
    s.last_client_size = None;
    s.last_pane_sizes.clear();
    s.pending_client_size = None;
    s.pending_client_hits = 0;
    let shown = s
        .scenes
        .get_mut(&workspace_id)
        .is_some_and(|layout| layout.show_tab(tab_id));
    if !shown {
        // A topology event can expose the tab in ViewStore one GTK tick
        // before its resident root has been built. Build from the owned
        // snapshot and retry; this still does not touch Core. The visible
        // selection above remains pending if the layout is not available yet.
        refresh_workspace_layout(s, &workspace_id, false);
    }
    let shown = s
        .scenes
        .get_mut(&workspace_id)
        .is_some_and(|layout| layout.show_tab(tab_id));
    if shown && s.panel_open.is_none() && !s.overlay.pane_find.is_visible() {
        if let Some(pane) = active_pane.and_then(|pane| s.active_layout().pane(pane).cloned()) {
            pane.grab_focus();
        }
    }
    maybe_refresh_status(s, true);
    sync_chrome_visibility(s);
    true
}

pub(super) fn request_switch_tab(s: &mut UiState, tab_id: u32) {
    if tab_id == s.active_tab {
        return;
    }
    let _ = show_tab_scene(s, tab_id);
}

pub(super) fn activate_existing(s: &mut UiState, id: WorkspaceId) {
    if s.active_ws_id() == id {
        return;
    }
    let key = id.as_str();
    if s.view_store.workspace(&key).is_none() {
        tracing::warn!(
            target = "muxterm::linux",
            workspace = %key,
            "workspace scene is missing from the owned snapshot"
        );
        return;
    }
    show_workspace_scene(s, id, false);
}

/// Core open/activate 完成后，把 Core snapshot 的 active workspace 交给
/// frontend-visible Scene；Core activation 本身只发生在 open/close 等生命周期。
pub(super) fn after_activate(s: &mut UiState) {
    let Some(id) = s
        .view_store
        .active_workspace_id()
        .and_then(parse_workspace_id)
    else {
        return;
    };
    show_workspace_scene(s, id, true);
    mark_active_attention_visible(s);
    refresh_sidebar_if_open(s);
    report_all_pane_colours(s);
    maybe_refresh_status(s, true);
}

/// 切工作区只改 GtkStack 可见页和前端缓存，不调用 Core。
pub(super) fn show_workspace_scene(s: &mut UiState, id: WorkspaceId, seed_from_core: bool) {
    s.visible_workspace = id.clone();
    s.scenes.ensure(&id);
    let _ = s.scenes.show(&id);
    let had_cache = s.scenes.contains(&id);
    let switching = s.mounted_ws.as_ref() != Some(&id);
    if switching {
        if !s.scenes.contains(&id) {
            let uses = s.uses_tmux();
            let weak = s.self_weak.clone();
            let mut layout =
                LayoutHost::new(s.theme.clone(), s.font.clone(), uses, s.scrollback_lines);
            layout.set_menu_callback(move |pane_id, action| {
                if let Some(state) = weak.upgrade() {
                    handle_pane_menu_action(&state, pane_id, action);
                }
            });
            s.scenes.insert(id.clone(), layout);
        }
        // C8：后台 cache 的字号与当前字号不同才补（不在 Ctrl+= 里遍历全部）。
        let needs_font = s
            .scenes
            .get(&id)
            .map(|l| (l.font_size() - s.font.size).abs() > f32::EPSILON)
            .unwrap_or(false);
        if needs_font {
            let font = s.font.clone();
            s.scenes
                .get_mut(&id)
                .expect("layout 必须存在")
                .set_font(&font);
        }
        if !s.scenes.has_page(&id) {
            let root = s.scenes.get(&id).expect("layout 必须存在").root_box.clone();
            s.scenes.add_page(&id, &root);
        } else {
            let _ = s.scenes.show(&id);
        }
        s.mounted_ws = Some(id.clone());
    }
    if switching && had_cache && !s.uses_tmux() {
        s.hold_pane_resize_until = Some(Instant::now() + Duration::from_millis(400));
    } else {
        s.last_client_size = None;
        s.last_pane_sizes.clear();
        s.pending_client_size = None;
        s.pending_client_hits = 0;
        s.hold_pane_resize_until = None;
    }
    let recents = recent_target_configs(
        &s.view_store,
        &s.workspace_sockets,
        s.view_store.workspace_ids().count(),
    );
    s.qc_store.replace_all_recents(&recents);
    if seed_from_core {
        refresh_ui(s);
    } else {
        refresh_workspace_layout(s, &id, false);
        maybe_refresh_status(s, true);
        sync_chrome_visibility(s);
        refresh_sidebar_workspaces_if_open(s);
    }
}
