//! QuickConnect GTK controller.
//!
//! Candidate construction and filtering stay in `quickconnect_panel`; this
//! module owns the widget tree, rebuild debounce, keyboard navigation, and
//! overlay lifetime.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk::Key;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Entry, EventControllerKey, GestureClick, Label, ListBox,
    ListBoxRow, MenuButton, Orientation, Popover, PositionType, ScrolledWindow, SelectionMode,
    Window,
};

use crate::frontend::ffi_client::ClientAttentionStatus;
use crate::frontend::i18n::{self, Key as TextKey};
use crate::frontend::linux::panel_model::{
    filter_attention_panel_rows, filter_workspace_rows, search_rows, PanelModel, PanelTab,
    SearchScope,
};
use crate::frontend::linux::quick_pick;
use crate::frontend::linux::quickconnect::existing::ExistingRuntime;
use crate::frontend::linux::quickconnect::model::{QuickConnect, TargetTransport, WorkspaceQuery};

use super::{
    attention_panel_row, ensure_overlay, existing_connect_name, existing_items, existing_row,
    reachability_dot, root_items_with_existing_and_search, target_row, visible_action_for_item,
    ExistingNav, PanelItem, PanelShowArgs, VisibleAction, PANEL_TEXT_MAX_CHARS,
};

const NEW_PROJECT_ID: &str = "__new_project__";
const PANEL_ENTRY_HEIGHT: i32 = 36;
const PANEL_MAX_WIDTH: i32 = 640;
const PANEL_REBUILD_DEBOUNCE_MS: u64 = 24;

thread_local! {
    static PANEL_DISMISS: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

thread_local! {
    static PANEL_REFRESH: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// 测试/生产共用：让当前面板按最新状态重建列表（SSH 探测回来再填）。
pub(super) fn refresh_current() {
    PANEL_REFRESH.with(|slot| {
        if let Some(refresh) = slot.borrow().as_ref() {
            refresh();
        }
    });
}

/// 测试/生产共用：关闭当前 QuickConnect 面板（AppWindow 跳转后关面板，W15b）。
pub(super) fn close_current() {
    PANEL_DISMISS.with(|slot| {
        if let Some(dismiss) = slot.borrow().as_ref() {
            dismiss();
        }
    });
    clear_panel_hooks();
}

/// 窗口销毁前拆掉 thread_local 回调，避免探测线程 refresh 已死 GTK 控件。
pub(super) fn clear_panel_hooks() {
    PANEL_REFRESH.with(|slot| *slot.borrow_mut() = None);
    PANEL_DISMISS.with(|slot| *slot.borrow_mut() = None);
}

/// ListBox 保留搜索框焦点时不会自动跟随选中行滚动；按行坐标只移动
/// 必要距离，确保键盘选中的整行始终留在 ScrolledWindow 视口内。
fn reveal_selected_row(scroller: &ScrolledWindow, list: &ListBox, row: &ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    scroller.vadjustment().clamp_page(
        f64::from(bounds.y()),
        f64::from(bounds.y() + bounds.height()),
    );
}

/// 弹出三 tab QuickConnect 面板（普通 Overlay，不构造 AppWindow）。
pub(super) fn show(parent: &impl IsA<Window>, args: PanelShowArgs) {
    let parent = parent.as_ref();
    let parent_h = parent.height().max(400);
    let (panel_h, list_h) = quick_pick::panel_list_heights(parent_h);
    let parent_w = parent.width();
    let panel_w = if parent_w > 0 {
        parent_w.saturating_sub(32).clamp(1, PANEL_MAX_WIDTH)
    } else {
        PANEL_MAX_WIDTH
    };

    let PanelShowArgs {
        initial_tab,
        workspaces,
        workspace_search_items,
        agents,
        attention,
        on_connect,
        on_existing_connect,
        on_edit,
        on_new_project,
        on_jump_pane,
        on_mute,
        search,
        on_close,
        ssh_reach,
        existing,
        on_existing_nav,
    } = args;
    let ssh_reach = Rc::new(ssh_reach);
    let existing = Rc::new(existing);

    let overlay = ensure_overlay(parent);
    let backdrop = GtkBox::new(Orientation::Vertical, 0);
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    backdrop.add_css_class("quick-pick-backdrop");

    let panel = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .halign(Align::Center)
        .valign(Align::Start)
        .build();
    panel.add_css_class("quick-pick-root");
    panel.set_widget_name("muxterm-panel");
    panel.set_margin_top(28);
    panel.set_size_request(panel_w, panel_h);
    panel.set_hexpand(false);
    panel.set_vexpand(false);
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(i18n::tr(TextKey::QuickConnectPlaceholder))
        .hexpand(true)
        .build();
    entry.set_widget_name("muxterm-panel-entry");
    entry.add_css_class("quick-pick-entry");
    entry.set_margin_start(10);
    entry.set_margin_end(10);
    entry.set_margin_top(8);
    entry.set_size_request(-1, PANEL_ENTRY_HEIGHT);
    panel.append(&entry);

    // 三 tab 按钮
    let tab_bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(4)
        .margin_start(10)
        .margin_end(10)
        .margin_top(6)
        .build();
    let tab_workspaces = gtk4::ToggleButton::with_label(&i18n::tr(TextKey::PanelTabWorkspaces));
    tab_workspaces.set_widget_name("muxterm-panel-tab-workspaces");
    let tab_attention = gtk4::ToggleButton::with_label(&i18n::tr(TextKey::PanelTabAttention));
    tab_attention.set_widget_name("muxterm-panel-tab-attention");
    let tab_search = gtk4::ToggleButton::with_label(&i18n::tr(TextKey::PanelTabSearch));
    tab_search.set_widget_name("muxterm-panel-tab-search");
    tab_bar.append(&tab_workspaces);
    tab_bar.append(&tab_attention);
    tab_bar.append(&tab_search);
    panel.append(&tab_bar);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.add_css_class("quick-pick-list");
    list.set_widget_name("muxterm-panel-list");

    let sw = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&list)
        .build();
    sw.set_margin_top(6);
    sw.set_size_request(-1, list_h);
    panel.append(&sw);

    // Tab3 占位（搜索中…行，阶段 C 前固定显示占位文案）
    let search_status = Label::new(Some(&i18n::tr(TextKey::SearchPlaceholderPhaseC)));
    search_status.set_widget_name("muxterm-search-status");
    search_status.set_halign(Align::Start);
    search_status.set_margin_start(16);
    search_status.set_margin_top(10);
    search_status.set_margin_bottom(10);
    search_status.set_visible(false);
    panel.append(&search_status);

    // W18f：搜索范围（当前 pane / 本工作区 / 全部）。
    let scope_bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(4)
        .margin_start(12)
        .margin_end(12)
        .build();
    let scope_pane = gtk4::ToggleButton::with_label("pane");
    scope_pane.set_widget_name("muxterm-search-scope-pane");
    let scope_workspace = gtk4::ToggleButton::with_label("workspace");
    scope_workspace.set_widget_name("muxterm-search-scope-workspace");
    let scope_all = gtk4::ToggleButton::with_label("all");
    scope_all.set_widget_name("muxterm-search-scope-all");
    scope_bar.append(&scope_pane);
    scope_bar.append(&scope_workspace);
    scope_bar.append(&scope_all);
    scope_bar.set_visible(false);
    panel.append(&scope_bar);

    // Attention 操作：静音由 Core 保存，面板只负责选择 pane 与触发回调。
    let attention_actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(12)
        .build();
    let mute_button = MenuButton::new();
    mute_button.set_widget_name("muxterm-attention-mute");
    mute_button.set_label(&i18n::tr(TextKey::Mute1h));
    mute_button.set_sensitive(false);
    let mute_popover = Popover::new();
    let mute_box = GtkBox::new(Orientation::Vertical, 0);
    let mut mute_items = Vec::new();
    for (id, label) in [
        ("5m", "5m"),
        ("10m", "10m"),
        ("30m", "30m"),
        ("1h", "1h"),
        ("4h", "4h"),
        ("24h", "24h"),
    ] {
        let item = Button::with_label(label);
        item.set_widget_name(&format!("muxterm-attention-mute-{id}"));
        mute_box.append(&item);
        mute_items.push(item);
    }
    mute_popover.set_child(Some(&mute_box));
    mute_button.set_popover(Some(&mute_popover));
    attention_actions.append(&mute_button);
    attention_actions.set_visible(false);
    panel.append(&attention_actions);

    overlay.add_overlay(&backdrop);
    overlay.add_overlay(&panel);

    // GTK4 没有 GTK3 的 EntryCompletion；用轻量 Popover 提供同等行为，
    // Tab 也会直接采用第一条候选并保留前面的查询条件。
    let completion_list = ListBox::new();
    completion_list.set_selection_mode(SelectionMode::Browse);
    completion_list.add_css_class("quick-pick-completions");
    let completion_popover = Popover::builder()
        .has_arrow(false)
        .position(PositionType::Bottom)
        .build();
    completion_popover.set_parent(&entry);
    completion_popover.set_child(Some(&completion_list));
    completion_popover.set_width_request(220);

    let model = Rc::new(RefCell::new(PanelModel::open(initial_tab)));
    let all = Rc::new(workspaces);
    let search_all = Rc::new(workspace_search_items);
    let agents = Rc::new(agents);
    let attention = Rc::new(attention);
    let callbacks = Rc::new(PanelShowArgs {
        initial_tab,
        workspaces: Vec::new(),
        workspace_search_items: Vec::new(),
        agents: Vec::new(),
        attention: Vec::new(),
        on_connect,
        on_existing_connect,
        on_edit,
        on_new_project,
        on_jump_pane,
        on_mute,
        search,
        on_close: std::boxed::Box::new(|| {}),
        ssh_reach: HashMap::new(),
        existing: (*existing).clone(),
        on_existing_nav,
    });
    let finished = Rc::new(RefCell::new(false));
    let completion_values = Rc::new(RefCell::new(Vec::<String>::new()));

    let on_close = Rc::new(on_close);
    let dismiss = {
        let overlay = overlay.clone();
        let backdrop = backdrop.clone();
        let panel = panel.clone();
        let finished = finished.clone();
        let on_close = on_close.clone();
        let completion_popover = completion_popover.clone();
        move || {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            completion_popover.popdown();
            completion_popover.set_child(None::<&gtk4::Widget>);
            completion_popover.unparent();
            overlay.remove_overlay(&backdrop);
            overlay.remove_overlay(&panel);
            on_close();
        }
    };
    PANEL_DISMISS.with(|slot| *slot.borrow_mut() = Some(Box::new(dismiss.clone())));

    let update_completion: Rc<dyn Fn()> = {
        let entry = entry.clone();
        let model = model.clone();
        let existing = existing.clone();
        let completion_list = completion_list.clone();
        let completion_popover = completion_popover.clone();
        let completion_values = completion_values.clone();
        Rc::new(move || {
            let query = entry.text().to_string();
            let is_workspace_tab = model.borrow().tab == PanelTab::Workspaces;
            let aliases = {
                let ex = existing.borrow();
                let mut aliases = ex.ssh_aliases.clone();
                aliases.extend(ex.hosts.iter().cloned());
                aliases
            };
            let candidates = if is_workspace_tab {
                WorkspaceQuery::completion_candidates(&query, &aliases)
            } else {
                Vec::new()
            };
            *completion_values.borrow_mut() = candidates.clone();
            while let Some(child) = completion_list.first_child() {
                completion_list.remove(&child);
            }
            for candidate in &candidates {
                let row = ListBoxRow::new();
                row.set_activatable(true);
                let label = Label::new(Some(candidate));
                label.set_halign(Align::Start);
                label.set_margin_start(10);
                label.set_margin_end(10);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                row.set_child(Some(&label));
                completion_list.append(&row);
            }
            if candidates.is_empty() || !entry.has_focus() {
                completion_popover.popdown();
            } else {
                completion_list.select_row(completion_list.row_at_index(0).as_ref());
                completion_popover.popup();
            }
        })
    };
    completion_list.connect_row_activated({
        let entry = entry.clone();
        let completion_popover = completion_popover.clone();
        let completion_values = completion_values.clone();
        move |_, row| {
            let index = row.index() as usize;
            let Some(replacement) = completion_values.borrow().get(index).cloned() else {
                return;
            };
            let query = entry.text().to_string();
            entry.set_text(&WorkspaceQuery::replace_current_token(&query, &replacement));
            entry.set_position(-1);
            completion_popover.popdown();
            entry.grab_focus();
        }
    });

    {
        let dismiss = dismiss.clone();
        let gesture = GestureClick::new();
        gesture.connect_released(move |_, _, _, _| dismiss());
        backdrop.add_controller(gesture);
    }

    // 工作区级状态：blocked 优先于 done（从 attention 行推导）。
    let workspace_status = {
        let mut map: std::collections::HashMap<String, ClientAttentionStatus> =
            std::collections::HashMap::new();
        for p in attention.iter() {
            let entry = map
                .entry(p.workspace_id.clone())
                .or_insert(ClientAttentionStatus::Idle);
            if p.status_kind() == ClientAttentionStatus::Blocked {
                *entry = ClientAttentionStatus::Blocked;
            } else if *entry != ClientAttentionStatus::Blocked
                && p.status_kind() == ClientAttentionStatus::Done
            {
                *entry = ClientAttentionStatus::Done;
            }
        }
        map
    };

    let selected_attention = Rc::new(RefCell::new(None::<(String, u32)>));
    let visible_actions = Rc::new(RefCell::new(Vec::<VisibleAction>::new()));
    let rebuild = {
        let list = list.clone();
        let model = model.clone();
        let all = all.clone();
        let search_all = search_all.clone();
        let agents = agents.clone();
        let attention = attention.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        let search_status = search_status.clone();
        let tab_workspaces = tab_workspaces.clone();
        let tab_attention = tab_attention.clone();
        let tab_search = tab_search.clone();
        let workspace_status = workspace_status.clone();
        let ssh_reach = ssh_reach.clone();
        let scope_pane = scope_pane.clone();
        let scope_workspace = scope_workspace.clone();
        let scope_all = scope_all.clone();
        let scope_bar = scope_bar.clone();
        let attention_actions = attention_actions.clone();
        let mute_button = mute_button.clone();
        let selected_attention = selected_attention.clone();
        let existing = existing.clone();
        let visible_actions = visible_actions.clone();
        let update_completion = update_completion.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let mut actions = Vec::new();
            // 先取 tab/query 并释放 RefCell，再 set_active（toggled 会重入 rebuild）。
            let (tab, query) = {
                let m = model.borrow();
                (m.tab, m.query.clone())
            };
            // W20：非 Root 时列表来自已有的连接子目录；Root 在有 query
            // 时把所有已发现可 attach 的 workspace 也并入搜索候选。
            let all: Vec<PanelItem> = {
                let ex = existing.borrow();
                if ex.nav == ExistingNav::Root {
                    root_items_with_existing_and_search(&all, &search_all, &ex, &query)
                } else {
                    existing_items(
                        ex.nav.clone(),
                        &ex.locals,
                        &ex.hosts,
                        ex.probe_inflight,
                        |alias| ex.remote.get(alias).cloned().unwrap_or_default(),
                    )
                }
            };
            tab_workspaces.set_active(tab == PanelTab::Workspaces);
            tab_attention.set_active(tab == PanelTab::Attention);
            tab_search.set_active(tab == PanelTab::Search);
            search_status.set_visible(tab == PanelTab::Search);
            scope_bar.set_visible(tab == PanelTab::Search);
            attention_actions.set_visible(tab == PanelTab::Attention);
            selected_attention.borrow_mut().take();
            mute_button.set_sensitive(false);
            let scope = model.borrow().scope;
            scope_pane.set_active(scope == SearchScope::Pane);
            scope_workspace.set_active(scope == SearchScope::Workspace);
            scope_all.set_active(scope == SearchScope::All);
            match tab {
                PanelTab::Workspaces => {
                    let rows = filter_workspace_rows(&all, &query, |item| {
                        let id = match item {
                            PanelItem::Target(entry, _) => QuickConnect::unique_id(&entry.config),
                            _ => return None,
                        };
                        workspace_status.get(&id).copied()
                    });
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_activatable(true);
                        actions.push(visible_action_for_item(&row.item, &existing.borrow().nav));
                        match &row.item {
                            PanelItem::Target(entry, is_current) => {
                                row_widget.set_widget_name(&QuickConnect::unique_id(&entry.config));
                                if *is_current {
                                    row_widget.add_css_class("qc-current");
                                }
                                let reach = match &entry.config.transport {
                                    TargetTransport::Ssh { name } => ssh_reach.get(name).copied(),
                                    TargetTransport::Local => None,
                                };
                                let boxed = target_row(entry, *is_current, reach);
                                if let Some(status) = row.status {
                                    let mark = Label::new(Some(match status {
                                        ClientAttentionStatus::Blocked => "● ",
                                        ClientAttentionStatus::Done => "✓ ",
                                        _ => "",
                                    }));
                                    mark.add_css_class("qc-status-mark");
                                    boxed.prepend(&mark);
                                }
                                row_widget.set_child(Some(&boxed));
                                let cfg = entry.config.clone();
                                let on_edit = {
                                    let callbacks = callbacks.clone();
                                    let cfg = cfg.clone();
                                    let dismiss = dismiss.clone();
                                    move || {
                                        dismiss();
                                        (callbacks.on_edit)(cfg.clone());
                                    }
                                };
                                let dbl = GestureClick::new();
                                dbl.set_button(1);
                                dbl.connect_pressed(move |g, n, _, _| {
                                    if n == 2 {
                                        on_edit();
                                        g.set_state(gtk4::EventSequenceState::Claimed);
                                    }
                                });
                                row_widget.add_controller(dbl);
                            }
                            PanelItem::NewProject => {
                                row_widget.set_widget_name(NEW_PROJECT_ID);
                                let label = Label::new(Some(&format!(
                                    "＋ {}",
                                    i18n::tr(TextKey::NewProject)
                                )));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Folder { id, title } => {
                                row_widget.set_widget_name(&format!("muxterm-{id}"));
                                let label = Label::new(Some(title));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Back => {
                                row_widget.set_widget_name("muxterm-existing-back");
                                let label = Label::new(Some(&i18n::tr(TextKey::ExistingBack)));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Existing(entry) => {
                                let identity = entry
                                    .herdr_workspace_id
                                    .as_deref()
                                    .or(entry.tmux_session.as_deref())
                                    .unwrap_or(&entry.title);
                                // workspace ids are only unique inside a Herdr
                                // named session. Keep the session in the widget
                                // identity so two sessions exposing `w1` cannot
                                // make the first row win attach.
                                let identity = if entry.runtime == ExistingRuntime::Herdr {
                                    format!(
                                        "{}-{}",
                                        identity,
                                        entry.herdr_session.as_deref().unwrap_or("default")
                                    )
                                } else {
                                    identity.to_string()
                                };
                                row_widget.set_widget_name(&format!(
                                    "muxterm-existing-row-{}-{}-{}",
                                    entry.runtime.as_str(),
                                    existing_connect_name(entry),
                                    identity,
                                ));
                                let boxed = existing_row(entry);
                                row_widget.set_child(Some(&boxed));
                            }
                            PanelItem::Host { alias } => {
                                row_widget
                                    .set_widget_name(&format!("muxterm-existing-host-{alias}"));
                                let reach = ssh_reach.get(alias).copied();
                                let label = Label::new(Some(alias));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                let boxed = GtkBox::new(Orientation::Horizontal, 8);
                                boxed.append(&label);
                                if let Some(reach) = reach {
                                    let dot = reachability_dot(reach);
                                    boxed.append(&dot);
                                }
                                row_widget.set_child(Some(&boxed));
                            }
                            PanelItem::Loading => {
                                row_widget.set_widget_name("muxterm-existing-ssh-loading");
                                row_widget.set_activatable(false);
                                let label = Label::new(Some(&i18n::tr(TextKey::ExistingProbing)));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                            PanelItem::Empty { title } => {
                                row_widget.set_widget_name("muxterm-existing-empty");
                                row_widget.set_activatable(false);
                                let label = Label::new(Some(title));
                                label.set_halign(Align::Start);
                                label.set_margin_start(12);
                                label.set_margin_top(6);
                                label.set_margin_bottom(6);
                                row_widget.set_child(Some(&label));
                            }
                        }
                        list.append(&row_widget);
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
                PanelTab::Attention => {
                    let rows = filter_attention_panel_rows(&agents, &attention, &query);
                    let count = Label::new(Some(&i18n::tr_args(
                        TextKey::AttentionCount,
                        &[("n", &rows.len().to_string())],
                    )));
                    count.set_halign(Align::Start);
                    count.add_css_class("qc-attention-count");
                    count.set_margin_start(12);
                    count.set_margin_top(4);
                    count.set_margin_bottom(2);
                    let count_row = ListBoxRow::new();
                    count_row.set_activatable(false);
                    count_row.set_child(Some(&count));
                    list.append(&count_row);
                    actions.push(VisibleAction::None);
                    if rows.is_empty() {
                        let empty = Label::new(Some(&i18n::tr(TextKey::AttentionEmpty)));
                        empty.set_halign(Align::Start);
                        empty.set_margin_start(12);
                        empty.set_margin_top(6);
                        empty.set_margin_bottom(6);
                        let empty_row = ListBoxRow::new();
                        empty_row.set_activatable(false);
                        empty_row.set_child(Some(&empty));
                        list.append(&empty_row);
                        actions.push(VisibleAction::None);
                    }
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_activatable(true);
                        row_widget.set_widget_name(&format!(
                            "muxterm-attention-{}-{}",
                            row.workspace_id, row.pane_id
                        ));
                        row_widget.set_child(Some(&attention_panel_row(row)));
                        list.append(&row_widget);
                        actions.push(VisibleAction::Jump {
                            workspace_id: row.workspace_id.clone(),
                            pane_id: row.pane_id,
                            seq: 0,
                        });
                        if i == 0 {
                            *selected_attention.borrow_mut() =
                                Some((row.workspace_id.clone(), row.pane_id));
                            mute_button.set_sensitive(true);
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
                PanelTab::Search => {
                    let scope = model.borrow().scope;
                    let hits = (callbacks.search)(&query, scope);
                    let (rows, placeholder) = search_rows(&query, hits);
                    search_status.set_visible(placeholder);
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_widget_name(&format!(
                            "muxterm-search-hit-{}-{}-{}",
                            row.workspace_id, row.pane_id, row.seq
                        ));
                        let text = format!("{} · {} · {}", row.workspace_id, row.pane_id, row.line);
                        let label = Label::new(Some(&text));
                        label.set_halign(Align::Start);
                        label.set_hexpand(true);
                        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                        label.set_max_width_chars(PANEL_TEXT_MAX_CHARS);
                        label.set_margin_start(12);
                        label.set_margin_top(5);
                        label.set_margin_bottom(5);
                        row_widget.set_child(Some(&label));
                        list.append(&row_widget);
                        actions.push(VisibleAction::Jump {
                            workspace_id: row.workspace_id.clone(),
                            pane_id: row.pane_id,
                            seq: row.seq,
                        });
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
            }
            *visible_actions.borrow_mut() = actions;
            update_completion();
        }
    };
    rebuild();
    let rebuild_generation = Rc::new(Cell::new(0u64));
    let pending_rebuild = Rc::new(Cell::new(None::<u64>));
    let schedule_rebuild = {
        let rebuild = rebuild.clone();
        let rebuild_generation = rebuild_generation.clone();
        let pending_rebuild = pending_rebuild.clone();
        let finished = finished.clone();
        move || {
            if *finished.borrow() {
                return;
            }
            let generation = rebuild_generation.get().wrapping_add(1);
            rebuild_generation.set(generation);
            pending_rebuild.set(Some(generation));
            let rebuild = rebuild.clone();
            let pending_rebuild = pending_rebuild.clone();
            let finished = finished.clone();
            glib::timeout_add_local_once(
                Duration::from_millis(PANEL_REBUILD_DEBOUNCE_MS),
                move || {
                    if *finished.borrow() || pending_rebuild.get() != Some(generation) {
                        return;
                    }
                    pending_rebuild.set(None);
                    rebuild();
                },
            );
        }
    };
    let flush_rebuild = {
        let rebuild = rebuild.clone();
        let rebuild_generation = rebuild_generation.clone();
        let pending_rebuild = pending_rebuild.clone();
        let finished = finished.clone();
        move || {
            if *finished.borrow() {
                return;
            }
            if pending_rebuild.replace(None).is_some() {
                rebuild_generation.set(rebuild_generation.get().wrapping_add(1));
                rebuild();
            }
        }
    };
    {
        let schedule_rebuild = schedule_rebuild.clone();
        let model = model.clone();
        let update_completion = update_completion.clone();
        PANEL_REFRESH.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                // Existing 探测结果也会成为根目录搜索候选，因此根目录
                // 同样需要刷新；Attention/Search 不受影响。
                if model.borrow().tab == PanelTab::Workspaces {
                    schedule_rebuild();
                }
                update_completion();
            }));
        });
    }

    // 搜索框持续持有输入焦点，因此 ListBox 自身不会替选中行滚动。
    // 所有键盘/程序化选中统一从这里保证可见。
    {
        let sw = sw.clone();
        let selected_attention = selected_attention.clone();
        let mute_button = mute_button.clone();
        list.connect_row_selected(move |list, row| {
            if let Some(row) = row {
                reveal_selected_row(&sw, list, row);
            }
            if let Some((workspace_id, pane_id)) = row.and_then(|row| {
                let name = row.widget_name();
                let rest = name.strip_prefix("muxterm-attention-")?;
                let (workspace_id, pane_id) = rest.rsplit_once('-')?;
                Some((workspace_id.to_string(), pane_id.parse::<u32>().ok()?))
            }) {
                *selected_attention.borrow_mut() = Some((workspace_id, pane_id));
                mute_button.set_sensitive(true);
            }
        });
    }

    // 静音菜单项：选中的 pane 由 row-selected 维护，真正的状态写入由 window/Core 完成。
    for (item, seconds) in mute_items
        .into_iter()
        .zip([300u64, 600, 1800, 3600, 14400, 86400])
    {
        let callbacks = callbacks.clone();
        let selected_attention = selected_attention.clone();
        let mute_popover = mute_popover.clone();
        item.connect_clicked(move |_| {
            if let Some((workspace_id, pane_id)) = selected_attention.borrow().clone() {
                (callbacks.on_mute)(workspace_id, pane_id, Duration::from_secs(seconds));
            }
            mute_popover.popdown();
        });
    }

    {
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        let update_completion = update_completion.clone();
        entry.connect_changed(move |e| {
            model.borrow_mut().query = e.text().to_string();
            update_completion();
            schedule_rebuild();
        });
    }

    // 搜索范围按钮：切换 scope 并重建（W18f）。
    for (btn, scope) in [
        (scope_pane.clone(), SearchScope::Pane),
        (scope_workspace.clone(), SearchScope::Workspace),
        (scope_all.clone(), SearchScope::All),
    ] {
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        let entry = entry.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                let changed = {
                    let mut m = model.borrow_mut();
                    if m.scope == scope {
                        false
                    } else {
                        m.scope = scope;
                        true
                    }
                };
                if changed {
                    schedule_rebuild();
                    entry.grab_focus();
                }
            }
        });
    }

    // tab 按钮点击
    for (btn, tab) in [
        (tab_workspaces.clone(), PanelTab::Workspaces),
        (tab_attention.clone(), PanelTab::Attention),
        (tab_search.clone(), PanelTab::Search),
    ] {
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        let entry = entry.clone();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                // 只有 tab 真正变化才重建，避免 set_active 重入死循环。
                let changed = {
                    let mut m = model.borrow_mut();
                    if m.tab == tab {
                        false
                    } else {
                        m.tab = tab;
                        true
                    }
                };
                if changed {
                    schedule_rebuild();
                    entry.grab_focus();
                }
            }
        });
    }

    let run_action = {
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        let existing = existing.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        let entry = entry.clone();
        move |action: VisibleAction| match action {
            VisibleAction::Connect(config) => {
                dismiss();
                (callbacks.on_connect)(config);
            }
            VisibleAction::ExistingConnect(config) => {
                dismiss();
                (callbacks.on_existing_connect)(config);
            }
            VisibleAction::NewProject => {
                dismiss();
                (callbacks.on_new_project)();
            }
            VisibleAction::Navigate(next) => {
                existing.borrow_mut().nav = next.clone();
                (callbacks.on_existing_nav)(next);
                schedule_rebuild();
                entry.grab_focus();
            }
            VisibleAction::Jump {
                workspace_id,
                pane_id,
                seq,
            } => {
                // 搜索跳转由 window 侧关闭；独立面板测试会保留面板量宽度。
                (callbacks.on_jump_pane)(workspace_id, pane_id, seq);
            }
            VisibleAction::None => {}
        }
    };
    let activate_row = {
        let visible_actions = visible_actions.clone();
        let run_action = run_action.clone();
        let finished = finished.clone();
        move |row: &ListBoxRow| {
            if *finished.borrow() {
                return;
            }
            let idx = row.index() as usize;
            let action = visible_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or(VisibleAction::None);
            run_action(action);
        }
    };

    list.connect_row_activated({
        let activate_row = activate_row.clone();
        move |_, row| activate_row(row)
    });
    entry.connect_activate({
        let activate_row = activate_row.clone();
        let flush_rebuild = flush_rebuild.clone();
        let list = list.clone();
        let finished = finished.clone();
        move |_| {
            if *finished.borrow() {
                return;
            }
            // Enter 紧跟最后一个字符时，timeout 尚未重建列表。先同步提交
            // 最新 query，避免激活旧行或因尚无选中行而静默失效。
            // 点击/row.activate() 不得走这条路径：flush 会毁掉被点的行
            // 并改选第一行（Existing 的 Back、根列表的 Folder），attach 静默失败。
            flush_rebuild();
            if let Some(row) = list.selected_row() {
                activate_row(&row);
            }
        }
    });

    {
        let dismiss = dismiss.clone();
        let model = model.clone();
        let schedule_rebuild = schedule_rebuild.clone();
        let list = list.clone();
        let entry_for_keys = entry.clone();
        let completion_popover = completion_popover.clone();
        let completion_values = completion_values.clone();
        let controller = EventControllerKey::new();
        controller.connect_key_pressed(move |_c, key, _code, mods| match key {
            Key::Escape => {
                dismiss();
                glib::Propagation::Stop
            }
            Key::Tab => {
                let query = entry_for_keys.text().to_string();
                let aliases = {
                    let ex = existing.borrow();
                    let mut aliases = ex.ssh_aliases.clone();
                    aliases.extend(ex.hosts.iter().cloned());
                    aliases
                };
                let candidates = if model.borrow().tab == PanelTab::Workspaces {
                    WorkspaceQuery::completion_candidates(&query, &aliases)
                } else {
                    Vec::new()
                };
                if let Some(replacement) = candidates.first() {
                    let completed = WorkspaceQuery::replace_current_token(&query, replacement);
                    // 完整 token 再按 Tab 应回到面板 tab 切换；否则
                    // `@tmux` 会被原样替换并吞掉每次 Tab。
                    if completed != query {
                        entry_for_keys.set_text(&completed);
                        entry_for_keys.set_position(-1);
                        completion_values.borrow_mut().clear();
                        completion_popover.popdown();
                        schedule_rebuild();
                        return glib::Propagation::Stop;
                    }
                }
                model
                    .borrow_mut()
                    .cycle_tab(mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK));
                schedule_rebuild();
                glib::Propagation::Stop
            }
            Key::Up | Key::Down => {
                let mut rows = 0i32;
                while list.row_at_index(rows).is_some() {
                    rows += 1;
                }
                if rows == 0 {
                    return glib::Propagation::Stop;
                }
                let step = if key == Key::Down { 1 } else { -1 };
                let mut next = if let Some(row) = list.selected_row() {
                    row.index() + step
                } else if step > 0 {
                    0
                } else {
                    rows - 1
                };
                while next >= 0 && next < rows {
                    let Some(row) = list.row_at_index(next) else {
                        break;
                    };
                    if row.is_activatable() {
                        list.select_row(Some(&row));
                        break;
                    }
                    next += step;
                }
                entry_for_keys.grab_focus();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        entry.add_controller(controller);
    }

    entry.grab_focus();
    gtk4::prelude::GtkWindowExt::set_focus(parent, Some(&entry));
    let parent_focus = parent.clone();
    let entry_focus = entry.clone();
    glib::timeout_add_local_once(Duration::from_millis(1), move || {
        gtk4::prelude::GtkWindowExt::set_focus(&parent_focus, Some(&entry_focus));
        entry_focus.grab_focus();
    });
}
