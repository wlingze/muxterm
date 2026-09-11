//! Persistent workspace layout projection for the GTK frontend.
//!
//! Topology is projected from the frontend-owned `ViewStore` into resident
//! `LayoutHost` / `PaneSurface` widgets. This module never asks Core to
//! activate or recapture a workspace as part of a layout refresh.

use gtk4::prelude::*;

use muxterm_core::protocol::{PaneId, WorkspaceId};

use super::window_status::{maybe_refresh_status, sync_chrome_visibility};
use super::window_surface::seed_unseeded_pane_for;
use super::{resident_pane_view, ClientLayout, SurfaceInput, UiState};

pub(super) fn refresh_ui(s: &mut UiState) {
    let wid = s.active_ws_id();
    refresh_workspace_layout(s, &wid, true);
    maybe_refresh_status(s, true);
    sync_chrome_visibility(s);
}

/// Commit one workspace's final topology to its persistent LayoutHost.
///
/// This function intentionally does not switch the active window for a
/// background workspace. It only creates/reparents resident PaneViews and
/// feeds an already-realized Surface from the frontend-owned ViewStore state.
pub(super) fn refresh_workspace_layout(s: &mut UiState, wid: &WorkspaceId, seed_from_core: bool) {
    let is_active = s.active_ws_id() == *wid;
    let workspace_key = wid.as_str();
    let Some(view) = s.view_store.workspace(&workspace_key) else {
        return;
    };
    let tab_ids: Vec<u32> = view.tabs.iter().map(|tab| tab.id).collect();
    let stored_tab = s.visible_tabs.get(&workspace_key).copied();
    if stored_tab.is_some_and(|tab_id| !tab_ids.contains(&tab_id)) {
        s.visible_tabs.remove(&workspace_key);
        s.local_tab_overrides.remove(&workspace_key);
    }
    let active_tab = s
        .visible_tab_id(&workspace_key)
        .or_else(|| tab_ids.first().copied());
    // W4：topology sync 必须为**所有** tab 的 leaves 建立常驻 PaneView，
    // 不能只建 active tab；hidden tab 的 frame/output 隐藏期间继续 feed。
    let layouts = tab_ids
        .iter()
        .filter_map(|tab_id| {
            view.layouts
                .get(tab_id)
                .cloned()
                .or_else(|| {
                    view.panes
                        .get(tab_id)
                        .and_then(|panes| panes.first())
                        .map(|pane| ClientLayout::Leaf { pane_id: pane.id })
                })
                .map(|layout| (*tab_id, layout))
        })
        .collect::<Vec<_>>();
    let panes: Vec<(u32, u16, u16, bool)> = active_tab
        .and_then(|tab_id| view.panes.get(&tab_id))
        .map(|panes| {
            panes
                .iter()
                .map(|pane| (pane.id, pane.cols, pane.rows, pane.is_active))
                .collect()
        })
        .unwrap_or_default();

    s.scenes.ensure(wid);

    if is_active {
        // tab 列表由 status bar 中区渲染（apply 时按签名重建），这里只维护
        // 当前 frontend-visible tab 的兼容缓存。
        if let Some(active) = active_tab {
            s.active_tab = active;
            s.visible_tabs.insert(workspace_key.to_owned(), active);
        }
    }

    let owner = wid.clone();
    let input_queue = s.surface_input_queue.clone();
    let input_cb = move |pane_id: u32, data: &[u8]| {
        input_queue.borrow_mut().push_back(SurfaceInput {
            workspace: owner.clone(),
            pane: PaneId(pane_id),
            data: data.to_vec(),
        });
    };

    // 重建布局（pane 控件跨 tab 保留：像素缓存，不因换 tab 销毁）。
    if !layouts.is_empty() {
        if let Some(layout) = s.scenes.get_mut(wid) {
            for (tab, client_layout) in &layouts {
                layout.apply_client_layout(*tab, client_layout, &input_cb);
            }
            // 全部 tab 常驻后，把 active tab 放回可见页（apply_layout 会
            // 依次 set_visible_child，最后一次调用决定显示页）。
            if let Some(active) = active_tab {
                layout.show_tab(active);
            }
        }

        for (pane_id, cols, rows, pane_active) in panes {
            if let Some(view) = resident_pane_view(s, wid, pane_id) {
                // Surface：已有 pane 只 show/hide，不 reset、不 dump；
                // 滚动走 VTE 自身 scrollback（F5）。
                view.ensure_grid_size(cols, rows);
                // attach 保真（1820.log 白屏）：布局建好后，把 core 里
                // capture-pane 快照播种进 VTE。快照事件可能在视图创建前
                // 已消费，不能只依赖 PaneOutput 增量。
                if seed_from_core {
                    seed_unseeded_pane_for(s, wid, &view, pane_id, cols, rows);
                }
                if is_active && pane_active {
                    s.active_pane = pane_id;
                    // 临时输入面板存在时不能由 topology refresh 抢走焦点；
                    // 没有输入面板时，键盘归当前 terminal。
                    if s.panel_open.is_none() && !s.overlay.pane_find.is_visible() {
                        view.grab_focus();
                    }
                }
            }
        }
    }
}
