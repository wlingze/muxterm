//! Overlay and attention navigation orchestration for the GTK window.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::Window;

use muxterm_protocol::WorkspaceId;

use super::window_event_pump::{activity_snapshot, panel_attention_rows};
use super::window_sidebar::refresh_sidebar_if_open;
use super::window_status::refresh_attention_chrome;
use super::{
    activate_existing, active_workspace_id, active_workspace_key, build_root_items,
    build_search_items, collect_ssh_reach, connect_open_request, open_target_config,
    parse_workspace_id, recent_target_configs, request_switch_tab, spawn_existing_ssh_probe,
    spawn_local_existing_probe, workspace_replica_id, workspace_to_target_config, AgentSidebarItem,
    ClientTask, ExistingNav, PanelTab, UiState,
};

pub(super) fn open_pane_find(state: &Rc<RefCell<UiState>>) {
    let s = state.borrow();
    s.overlay.pane_find.set_visible(true);
    s.overlay.pane_find_entry.grab_focus();
}

pub(super) fn open_quick_connect(state: &Rc<RefCell<UiState>>, window: &Window) {
    open_panel(state, window, PanelTab::Workspaces);
}

/// 打开三 tab 面板（initial_tab 由入口决定：Alt+Q → Workspaces，红点 → Attention）。
pub(super) fn open_panel(state: &Rc<RefCell<UiState>>, window: &Window, initial_tab: PanelTab) {
    let (workspaces, workspace_search_items, agents, attention, win, st, ssh_reach) = {
        let mut s = state.borrow_mut();
        let recents = recent_target_configs(
            &s.view_store,
            &s.workspace_sockets,
            s.view_store.workspace_ids().count(),
        );
        s.qc_store.replace_all_recents(&recents);
        let current = {
            let workspace_key = s.active_workspace_key();
            let workspace = s
                .view_store
                .workspace(&workspace_key)
                .and_then(|view| view.workspace.as_ref());
            workspace.and_then(|workspace| {
                let id = parse_workspace_id(&workspace_key)?;
                let socket = s
                    .workspace_sockets
                    .get(&id)
                    .and_then(|value| value.as_deref());
                Some(workspace_to_target_config(workspace, socket))
            })
        };
        let store = s.qc_store.clone();
        let win = window.clone();
        let st = state.clone();
        let workspaces = build_root_items(&store, current.as_ref());
        let workspace_search_items = build_search_items(&store, current.as_ref());
        let ssh_reach = collect_ssh_reach(&mut s, &workspaces);
        // 临时输入 surface 互斥：QuickConnect 打开后不保留 pane-find。
        s.overlay.pane_find.set_visible(false);
        // C7：本地列出搬后台线程（GTK 线程禁止 ssh / 扫 herdr socket），
        // 结果经 16ms poll 收编，和 SSH probe 同一模式。
        spawn_local_existing_probe(&mut s);
        let activity = activity_snapshot(&s);
        let agents = AgentSidebarItem::from_views(&s.view_store, &activity);
        let attention = panel_attention_rows(&activity);
        s.panel_open = Some(initial_tab);
        (
            workspaces,
            workspace_search_items,
            agents,
            attention,
            win,
            st,
            ssh_reach,
        )
    };
    if !window.is_visible() {
        window.present();
    }
    crate::frontend::linux::quickconnect_panel::show(
        &win,
        crate::frontend::linux::quickconnect_panel::PanelShowArgs {
            initial_tab,
            workspaces,
            workspace_search_items,
            agents,
            attention,
            on_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |request| {
                    connect_open_request(&st, request);
                })
            },
            on_existing_connect: {
                let st = st.clone();
                std::boxed::Box::new(move |request| {
                    connect_open_request(&st, request);
                })
            },
            on_edit: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move |cfg| {
                    open_target_config(&st, &win, Some(cfg));
                })
            },
            on_new_project: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move || {
                    open_target_config(&st, &win, None);
                })
            },
            on_jump_pane: {
                let st = st.clone();
                std::boxed::Box::new(move |ws, pane, seq| {
                    jump_to_attention_pane(&st, &ws, pane, seq);
                })
            },
            on_mute: {
                let st = st.clone();
                let win = win.clone();
                std::boxed::Box::new(move |ws, pane, duration| {
                    let seconds = duration.as_secs();
                    let rc = {
                        let s = st.borrow();
                        attention_workspace_id(&s, &ws)
                            .map(|workspace_id| {
                                s.event_pump.client().workspace_attention_mute(
                                    &workspace_id.as_str(),
                                    pane,
                                    seconds,
                                )
                            })
                            .unwrap_or(-1)
                    };
                    if rc == 0 {
                        let mut s = st.borrow_mut();
                        if let Some(workspace_id) = attention_workspace_id(&s, &ws) {
                            s.compatibility_activity.mute_for(
                                &workspace_id.as_str(),
                                pane,
                                seconds,
                            );
                        }
                        refresh_sidebar_if_open(&mut s);
                        refresh_attention_chrome(&s, &win);
                    } else {
                        tracing::warn!(
                            target = "muxterm::linux",
                            "Core attention mute failed: workspace={ws}, pane={pane}, code={rc}"
                        );
                    }
                })
            },
            search: {
                let st = st.clone();
                std::boxed::Box::new(move |query, scope| {
                    // C8：空 query 不扫 replica（emulate 已返回空）。
                    if query.trim().is_empty() {
                        return Vec::new();
                    }
                    let s = st.borrow();
                    let workspace_replica = active_workspace_id(&s);
                    let hits = s
                        .event_pump
                        .client()
                        .search_all(query)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|hit| match scope {
                            crate::frontend::linux::panel_model::SearchScope::Pane => {
                                hit.workspace_id == workspace_replica
                                    && hit.pane_id == s.active_pane
                            }
                            crate::frontend::linux::panel_model::SearchScope::Workspace => {
                                hit.workspace_id == workspace_replica
                            }
                            crate::frontend::linux::panel_model::SearchScope::All => true,
                        })
                        .map(crate::frontend::linux::panel_model::SearchRow::from)
                        .collect();
                    hits
                })
            },
            on_close: {
                let st = st.clone();
                std::boxed::Box::new(move || {
                    let active_view = {
                        let mut s = st.borrow_mut();
                        s.panel_open = None;
                        let pane = s.active_pane;
                        s.active_layout().pane(pane).cloned()
                    };
                    if let Some(view) = active_view {
                        view.grab_focus();
                    }
                })
            },
            ssh_reach,
            existing: state.borrow().existing.clone(),
            on_existing_nav: {
                let st = st.clone();
                std::boxed::Box::new(move |nav| {
                    if nav == ExistingNav::SshHosts {
                        spawn_existing_ssh_probe(&st);
                    }
                })
            },
        },
    );
}

/// 跳到注意力 pane：若目标工作区不是当前前台连接，先切连接；
/// 命中在别的 tab 时先 `SwitchTab` 再 `SwitchPane`（W15b）。
pub(super) fn jump_to_attention_pane(state: &Rc<RefCell<UiState>>, ws: &str, pane: u32, seq: u64) {
    let mut s = state.borrow_mut();
    activate_attention_workspace(&mut s, ws);
    let workspace_key = active_workspace_key(&s);
    let tab_id = {
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
    if let Some(tid) = tab_id {
        if tid != s.active_tab {
            request_switch_tab(&mut s, tid);
        }
    }
    let _ = s.execute_active_task(ClientTask::SwitchPane { pane_id: pane });
    if seq > 0 {
        let row = s
            .event_pump
            .client()
            .workspace_pane_viewport_for_seq(&workspace_key, pane, seq);
        if let Some(row) = row {
            if let Some(view) = s.active_layout().pane(pane).cloned() {
                if let Some(adj) = view.terminal().vadjustment() {
                    adj.set_value(adj.lower() + row as f64);
                }
                s.overlay.search_highlight.set_visible(true);
            }
        }
    }
    drop(s);
    crate::frontend::linux::quickconnect_panel::close_current();
}

pub(super) fn activate_attention_workspace(s: &mut UiState, ws: &str) {
    if active_workspace_id(s) == ws {
        return;
    }
    if let Some(id) = attention_workspace_id(s, ws) {
        activate_existing(s, id);
    }
}

pub(super) fn attention_workspace_id(s: &UiState, ws: &str) -> Option<WorkspaceId> {
    s.view_store
        .workspaces()
        .filter_map(|(_, view)| view.workspace.as_ref())
        .filter_map(|workspace| parse_workspace_id(&workspace.id))
        .find(|id| workspace_replica_matches(id, ws))
}

pub(super) fn workspace_replica_matches(id: &WorkspaceId, requested: &str) -> bool {
    if workspace_replica_id(id) == requested {
        return true;
    }
    if id.session.is_empty() {
        return false;
    }
    let transport = if id.transport == "ssh" {
        id.alias.as_deref().unwrap_or("ssh")
    } else {
        "local"
    };
    requested == format!("{}@{transport}", id.session)
}
