//! QuickConnect 面板：Recent + Project 快速连接（GTK Overlay）。
//!
//! 行为对齐 macOS `QuickConnectController`：搜索、badges、当前连接高亮、
//! 回车连接、双击编辑、末行 New Project。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk::Key;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Entry, EventControllerKey, GestureClick, Label, ListBox, ListBoxRow,
    Orientation, Overlay, ScrolledWindow, SelectionMode, Window,
};

use crate::core::attention::engine::PaneAttention;
use crate::core::attention::state::PaneStatus;
use crate::platform::i18n::{self, Key as TextKey};
use crate::platform::linux::panel_model::{
    filter_attention_rows, filter_workspace_rows, search_rows, PanelModel, PanelTab, SearchRow,
};
use crate::platform::linux::quick_pick;
use crate::platform::linux::quickconnect::model::{
    QuickBadge, QuickConnect, QuickConnectEntry, TargetConfig,
};
use crate::platform::linux::quickconnect::store::QuickConnectStore;
use crate::platform::linux::scrollback_view::{peek_view, set_peek_text};

const NEW_PROJECT_ID: &str = "__new_project__";

/// 面板回调。
pub struct QuickConnectCallbacks {
    pub on_connect: Box<dyn Fn(TargetConfig)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelItem {
    Target(QuickConnectEntry, bool),
    NewProject,
}

/// 按查询过滤（纯逻辑，便于单测）。
pub(crate) fn filter_panel_items(items: &[PanelItem], query: &str) -> Vec<PanelItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return items.to_vec();
    }
    items
        .iter()
        .filter(|item| match item {
            PanelItem::Target(entry, _) => QuickConnect::search_text(&entry.config).contains(&q),
            PanelItem::NewProject => {
                let label = format!(
                    "new project {}",
                    i18n::tr(TextKey::NewProject).to_lowercase()
                );
                label.contains(&q)
            }
        })
        .cloned()
        .collect()
}

pub fn build_items(store: &QuickConnectStore, current: Option<&TargetConfig>) -> Vec<PanelItem> {
    let current_id = current.map(QuickConnect::unique_id);
    let mut items: Vec<PanelItem> = QuickConnect::entries(&store.recents, &store.projects, 5)
        .into_iter()
        .map(|entry| {
            let is_current = current_id
                .as_ref()
                .is_some_and(|id| QuickConnect::unique_id(&entry.config) == *id);
            PanelItem::Target(entry, is_current)
        })
        .collect();
    items.push(PanelItem::NewProject);
    items
}

/// 弹出 QuickConnect 面板。
/// 三 tab 面板参数（LINUX-PLAN §10 C3.2/C3.3）。
pub struct PanelShowArgs {
    pub initial_tab: PanelTab,
    pub workspaces: Vec<PanelItem>,
    pub attention: Vec<PaneAttention>,
    pub on_connect: Box<dyn Fn(TargetConfig)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
    pub on_jump_pane: Box<dyn Fn(String, u32)>,
    pub on_reply: Box<dyn Fn(String, u32, String)>,
    pub on_mute: Box<dyn Fn(String, u32)>,
    pub peek_text: Box<dyn Fn(String, u32) -> String>,
    /// Search tab：query → replica 命中行。
    pub search: Box<dyn Fn(&str) -> Vec<SearchRow>>,
    /// 面板关闭回调（window 侧清 panel_open 状态）。
    pub on_close: Box<dyn Fn()>,
}

/// 弹出三 tab QuickConnect 面板（普通 Overlay，不构造 AppWindow）。
pub fn show(parent: &impl IsA<Window>, args: PanelShowArgs) {
    let parent = parent.as_ref();
    let parent_h = parent.height().max(400);
    let (panel_h, list_h) = quick_pick::panel_list_heights(parent_h);
    let panel_w = 640;

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
    panel.set_margin_top(40);
    panel.set_size_request(panel_w, panel_h);
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(i18n::tr(TextKey::QuickConnectPlaceholder))
        .hexpand(true)
        .build();
    entry.set_widget_name("muxterm-panel-entry");
    entry.add_css_class("quick-pick-entry");
    entry.set_margin_start(12);
    entry.set_margin_end(12);
    entry.set_margin_top(12);
    panel.append(&entry);

    // 三 tab 按钮
    let tab_bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(4)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
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
    sw.set_margin_top(8);
    sw.set_size_request(panel_w, list_h);
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

    // peek + 一行答复（C3.3）：仅 Attention tab 显示。
    let (peek_sw, peek_view) = peek_view();
    peek_sw.set_margin_start(12);
    peek_sw.set_margin_end(12);
    peek_sw.set_margin_top(8);
    peek_sw.set_size_request(panel_w - 24, 120);
    peek_sw.set_visible(false);
    panel.append(&peek_sw);

    let reply_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(12)
        .build();
    let reply_target = Label::new(Some(""));
    reply_target.set_widget_name("muxterm-reply-target");
    reply_target.set_halign(Align::Start);
    reply_target.add_css_class("qc-reply-target");
    let reply_entry = Entry::builder()
        .placeholder_text(i18n::tr(TextKey::ReplyHint))
        .hexpand(true)
        .build();
    reply_entry.set_widget_name("muxterm-reply-entry");
    reply_entry.set_sensitive(false);
    let mute_button = gtk4::Button::with_label(&i18n::tr(TextKey::Mute1h));
    mute_button.set_widget_name("muxterm-mute-1h");
    mute_button.set_sensitive(false);
    reply_row.append(&reply_target);
    reply_row.append(&reply_entry);
    reply_row.append(&mute_button);
    reply_row.set_visible(false);
    panel.append(&reply_row);

    overlay.add_overlay(&backdrop);
    overlay.add_overlay(&panel);

    let PanelShowArgs {
        initial_tab,
        workspaces,
        attention,
        on_connect,
        on_edit,
        on_new_project,
        on_jump_pane,
        on_reply,
        on_mute,
        peek_text,
        search,
        on_close,
    } = args;
    let model = Rc::new(RefCell::new(PanelModel::open(initial_tab)));
    let all = Rc::new(workspaces);
    let attention = Rc::new(attention);
    let callbacks = Rc::new(PanelShowArgs {
        initial_tab,
        workspaces: Vec::new(),
        attention: Vec::new(),
        on_connect,
        on_edit,
        on_new_project,
        on_jump_pane,
        on_reply,
        on_mute,
        peek_text,
        search,
        on_close: std::boxed::Box::new(|| {}),
    });
    let finished = Rc::new(RefCell::new(false));

    let on_close = Rc::new(on_close);
    let dismiss = {
        let overlay = overlay.clone();
        let backdrop = backdrop.clone();
        let panel = panel.clone();
        let finished = finished.clone();
        let on_close = on_close.clone();
        move || {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            overlay.remove_overlay(&backdrop);
            overlay.remove_overlay(&panel);
            on_close();
        }
    };

    {
        let dismiss = dismiss.clone();
        let gesture = GestureClick::new();
        gesture.connect_released(move |_, _, _, _| dismiss());
        backdrop.add_controller(gesture);
    }

    // 工作区级状态：blocked 优先于 done（从 attention 行推导）。
    let workspace_status = {
        let mut map: std::collections::HashMap<String, PaneStatus> =
            std::collections::HashMap::new();
        for p in attention.iter() {
            let entry = map
                .entry(p.workspace_id.clone())
                .or_insert(PaneStatus::Idle);
            if p.status == PaneStatus::Blocked {
                *entry = PaneStatus::Blocked;
            } else if *entry != PaneStatus::Blocked && p.status == PaneStatus::Done {
                *entry = PaneStatus::Done;
            }
        }
        map
    };

    // 根据当前选中行刷新 peek/答复（rebuild 与 row-selected 共用）。
    let update_peek = {
        let model = model.clone();
        let attention = attention.clone();
        let callbacks = callbacks.clone();
        let peek_view = peek_view.clone();
        let reply_target = reply_target.clone();
        let reply_entry = reply_entry.clone();
        let mute_button = mute_button.clone();
        let list = list.clone();
        move || {
            let tab = model.borrow().tab;
            if tab != PanelTab::Attention {
                return;
            }
            let Some(row) = list.selected_row() else {
                set_peek_text(&peek_view, "");
                reply_target.set_text("");
                reply_entry.set_sensitive(false);
                mute_button.set_sensitive(false);
                return;
            };
            let name = row.widget_name();
            let Some(rest) = name.strip_prefix("muxterm-attention-") else {
                return;
            };
            let Some((ws, pane)) = rest.rsplit_once('-') else {
                return;
            };
            let Ok(pane) = pane.parse::<u32>() else {
                return;
            };
            let ws = ws.to_string();
            let Some(sel) = attention
                .iter()
                .find(|p| p.workspace_id == ws && p.pane_id == pane)
                .cloned()
            else {
                return;
            };
            let ws = sel.workspace_id.clone();
            let pane = sel.pane_id;
            let process = sel.process_name.as_deref().unwrap_or("?");
            reply_target.set_text(&format!("{ws} · {process}"));
            reply_entry.set_sensitive(true);
            mute_button.set_sensitive(true);
            let text = (callbacks.peek_text)(ws, pane);
            set_peek_text(&peek_view, &text);
        }
    };

    let rebuild = {
        let list = list.clone();
        let model = model.clone();
        let all = all.clone();
        let attention = attention.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        let search_status = search_status.clone();
        let tab_workspaces = tab_workspaces.clone();
        let tab_attention = tab_attention.clone();
        let tab_search = tab_search.clone();
        let workspace_status = workspace_status.clone();
        let update_peek = update_peek.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            // 先取 tab/query 并释放 RefCell，再 set_active（toggled 会重入 rebuild）。
            let (tab, query) = {
                let m = model.borrow();
                (m.tab, m.query.clone())
            };
            tab_workspaces.set_active(tab == PanelTab::Workspaces);
            tab_attention.set_active(tab == PanelTab::Attention);
            tab_search.set_active(tab == PanelTab::Search);
            search_status.set_visible(tab == PanelTab::Search);
            let show_peek = tab == PanelTab::Attention;
            peek_sw.set_visible(show_peek);
            reply_row.set_visible(show_peek);
            match tab {
                PanelTab::Workspaces => {
                    let rows = filter_workspace_rows(&all, &query, |item| {
                        let id = match item {
                            PanelItem::Target(entry, _) => QuickConnect::unique_id(&entry.config),
                            PanelItem::NewProject => return None,
                        };
                        workspace_status.get(&id).copied()
                    });
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_activatable(true);
                        match &row.item {
                            PanelItem::Target(entry, is_current) => {
                                row_widget.set_widget_name(&QuickConnect::unique_id(&entry.config));
                                if *is_current {
                                    row_widget.add_css_class("qc-current");
                                }
                                let boxed = target_row(entry, *is_current);
                                if let Some(status) = row.status {
                                    let mark = Label::new(Some(match status {
                                        PaneStatus::Blocked => "● ",
                                        PaneStatus::Done => "✓ ",
                                        _ => "",
                                    }));
                                    mark.add_css_class("qc-status-mark");
                                    boxed.prepend(&mark);
                                }
                                row_widget.set_child(Some(&boxed));
                                let cfg = entry.config.clone();
                                let dismiss = dismiss.clone();
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
                                label.set_margin_start(16);
                                label.set_margin_top(10);
                                label.set_margin_bottom(10);
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
                    let rows = filter_attention_rows(&attention, &query);
                    let count = Label::new(Some(&i18n::tr_args(
                        TextKey::AttentionCount,
                        &[("n", &rows.len().to_string())],
                    )));
                    count.set_halign(Align::Start);
                    count.add_css_class("qc-attention-count");
                    count.set_margin_start(16);
                    count.set_margin_top(8);
                    count.set_margin_bottom(4);
                    let count_row = ListBoxRow::new();
                    count_row.set_activatable(false);
                    count_row.set_child(Some(&count));
                    list.append(&count_row);
                    if rows.is_empty() {
                        let empty = Label::new(Some(&i18n::tr(TextKey::AttentionEmpty)));
                        empty.set_halign(Align::Start);
                        empty.set_margin_start(16);
                        empty.set_margin_top(10);
                        empty.set_margin_bottom(10);
                        let empty_row = ListBoxRow::new();
                        empty_row.set_activatable(false);
                        empty_row.set_child(Some(&empty));
                        list.append(&empty_row);
                    }
                    for (i, row) in rows.iter().enumerate() {
                        let row_widget = ListBoxRow::new();
                        row_widget.set_activatable(true);
                        row_widget.set_widget_name(&format!(
                            "muxterm-attention-{}-{}",
                            row.attention.workspace_id, row.attention.pane_id
                        ));
                        let process = row.attention.process_name.as_deref().unwrap_or("?");
                        let text = format!(
                            "{} · {} · {}",
                            row.attention.workspace_id, process, row.attention.last_line
                        );
                        let label = Label::new(Some(&text));
                        label.set_halign(Align::Start);
                        label.set_margin_start(16);
                        label.set_margin_top(8);
                        label.set_margin_bottom(8);
                        row_widget.set_child(Some(&label));
                        list.append(&row_widget);
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
                PanelTab::Search => {
                    let hits = (callbacks.search)(&query);
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
                        label.set_margin_start(16);
                        label.set_margin_top(8);
                        label.set_margin_bottom(8);
                        row_widget.set_child(Some(&label));
                        list.append(&row_widget);
                        if i == 0 {
                            list.select_row(Some(&row_widget));
                        }
                    }
                }
            }
            update_peek();
        }
    };
    rebuild();

    // Tab2 选中行 → peek + 答复目标；无选中 → 清空并禁用答复。
    {
        let update_peek = update_peek.clone();
        list.connect_row_selected(move |_, _| update_peek());
    }

    // 静音 1h：按钮或 `m` 键（选中行）。
    {
        let model = model.clone();
        let callbacks = callbacks.clone();
        let mute_button = mute_button.clone();
        let list_closure = list.clone();
        mute_button.connect_clicked(move |_| {
            let tab = model.borrow().tab;
            if tab != PanelTab::Attention {
                return;
            }
            let Some(row) = list_closure.selected_row() else {
                return;
            };
            let name = row.widget_name();
            let Some(rest) = name.strip_prefix("muxterm-attention-") else {
                return;
            };
            let Some((ws, pane)) = rest.rsplit_once('-') else {
                return;
            };
            let Ok(pane) = pane.parse::<u32>() else {
                return;
            };
            (callbacks.on_mute)(ws.to_string(), pane);
        });
    }
    {
        let model = model.clone();
        let callbacks = callbacks.clone();
        let list_closure = list.clone();
        let controller = EventControllerKey::new();
        controller.connect_key_pressed(move |_c, key, _code, _mods| {
            if key != Key::m {
                return glib::Propagation::Proceed;
            }
            let tab = model.borrow().tab;
            if tab != PanelTab::Attention {
                return glib::Propagation::Proceed;
            }
            let Some(row) = list_closure.selected_row() else {
                return glib::Propagation::Stop;
            };
            let name = row.widget_name();
            let Some(rest) = name.strip_prefix("muxterm-attention-") else {
                return glib::Propagation::Stop;
            };
            let Some((ws, pane)) = rest.rsplit_once('-') else {
                return glib::Propagation::Stop;
            };
            let Ok(pane) = pane.parse::<u32>() else {
                return glib::Propagation::Stop;
            };
            (callbacks.on_mute)(ws.to_string(), pane);
            glib::Propagation::Stop
        });
        reply_entry.add_controller(controller);
    }

    // 一行答复：Enter（无 Shift）发送一行 + \r，不关面板，发完重新 peek。
    {
        let model = model.clone();
        let attention = attention.clone();
        let callbacks = callbacks.clone();
        let peek_view = peek_view.clone();
        let reply_entry = reply_entry.clone();
        let list_closure = list.clone();
        let controller = EventControllerKey::new();
        controller.connect_key_pressed(move |c, key, _code, mods| {
            if key != Key::Return && key != Key::KP_Enter {
                return glib::Propagation::Proceed;
            }
            if mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                return glib::Propagation::Proceed;
            }
            let tab = model.borrow().tab;
            if tab != PanelTab::Attention {
                return glib::Propagation::Proceed;
            }
            // 从 controller 取宿主 Entry，避免闭包自持有造成 GObject 环。
            let Some(host) = c.widget().and_then(|w| w.downcast::<Entry>().ok()) else {
                return glib::Propagation::Stop;
            };
            let text = host.text().to_string();
            if text.trim().is_empty() {
                return glib::Propagation::Stop;
            }
            let Some(row) = list_closure.selected_row() else {
                return glib::Propagation::Stop;
            };
            let name = row.widget_name();
            let Some(rest) = name.strip_prefix("muxterm-attention-") else {
                return glib::Propagation::Stop;
            };
            let Some((ws, pane)) = rest.rsplit_once('-') else {
                return glib::Propagation::Stop;
            };
            let Ok(pane) = pane.parse::<u32>() else {
                return glib::Propagation::Stop;
            };
            let ws = ws.to_string();
            let Some(sel) = attention
                .iter()
                .find(|p| p.workspace_id == ws && p.pane_id == pane)
                .cloned()
            else {
                return glib::Propagation::Stop;
            };
            let ws = sel.workspace_id.clone();
            let pane = sel.pane_id;
            let line = text.trim_end_matches('\n').to_string();
            (callbacks.on_reply)(ws.clone(), pane, line.clone());
            host.set_text("");
            let peek = (callbacks.peek_text)(ws, pane);
            set_peek_text(&peek_view, &peek);
            glib::Propagation::Stop
        });
        reply_entry.add_controller(controller);
    }

    {
        let model = model.clone();
        let rebuild = rebuild.clone();
        entry.connect_changed(move |e| {
            model.borrow_mut().query = e.text().to_string();
            rebuild();
        });
    }

    // tab 按钮点击
    for (btn, tab) in [
        (tab_workspaces.clone(), PanelTab::Workspaces),
        (tab_attention.clone(), PanelTab::Attention),
        (tab_search.clone(), PanelTab::Search),
    ] {
        let model = model.clone();
        let rebuild = rebuild.clone();
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
                    rebuild();
                }
            }
        });
    }

    let activate = {
        let list = list.clone();
        let model = model.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        move || {
            let Some(row) = list.selected_row() else {
                return;
            };
            let idx = row.index() as usize;
            let tab = model.borrow().tab;
            match tab {
                PanelTab::Workspaces => {
                    let visible = filter_panel_items(&all, &model.borrow().query);
                    match visible.get(idx).cloned() {
                        Some(PanelItem::Target(entry, _)) => {
                            dismiss();
                            (callbacks.on_connect)(entry.config);
                        }
                        Some(PanelItem::NewProject) => {
                            dismiss();
                            (callbacks.on_new_project)();
                        }
                        None => {}
                    }
                }
                PanelTab::Attention => {
                    let rows = filter_attention_rows(&attention, &model.borrow().query);
                    if let Some(row) = rows.get(idx) {
                        (callbacks.on_jump_pane)(
                            row.attention.workspace_id.clone(),
                            row.attention.pane_id,
                        );
                    }
                }
                PanelTab::Search => {
                    let hits = (callbacks.search)(&model.borrow().query);
                    let (rows, _) = search_rows(&model.borrow().query, hits);
                    if let Some(row) = rows.get(idx) {
                        (callbacks.on_jump_pane)(row.workspace_id.clone(), row.pane_id);
                    }
                }
            }
        }
    };

    list.connect_row_activated({
        let activate = activate.clone();
        move |_, _| activate()
    });

    {
        let dismiss = dismiss.clone();
        let activate = activate.clone();
        let model = model.clone();
        let rebuild = rebuild.clone();
        let list = list.clone();
        let controller = EventControllerKey::new();
        controller.connect_key_pressed(move |_c, key, _code, mods| match key {
            Key::Escape => {
                dismiss();
                glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter => {
                activate();
                glib::Propagation::Stop
            }
            Key::Tab => {
                model
                    .borrow_mut()
                    .cycle_tab(mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK));
                rebuild();
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
                let cur = list.selected_row().map(|r| r.index()).unwrap_or(0);
                let next = if key == Key::Down {
                    (cur + 1).min(rows - 1)
                } else {
                    (cur - 1).max(0)
                };
                if let Some(row) = list.row_at_index(next) {
                    list.select_row(Some(&row));
                    row.grab_focus();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        entry.add_controller(controller);
    }

    entry.grab_focus();
}

fn target_row(entry: &QuickConnectEntry, is_current: bool) -> GtkBox {
    let col = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_start(16)
        .margin_end(16)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    if is_current {
        col.add_css_class("qc-current-row");
    }
    let title_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    let name = Label::new(Some(&entry.config.name));
    name.set_halign(Align::Start);
    name.add_css_class("qc-name");
    title_row.append(&name);
    for badge in &entry.badges {
        let b = Label::new(Some(&badge_label(*badge)));
        b.add_css_class("qc-badge");
        match badge {
            QuickBadge::Recent => b.add_css_class("qc-badge-recent"),
            QuickBadge::Project => b.add_css_class("qc-badge-project"),
        }
        title_row.append(&b);
    }
    if is_current {
        let cur = Label::new(Some(&i18n::tr(TextKey::Current).to_uppercase()));
        cur.add_css_class("qc-badge");
        cur.add_css_class("qc-badge-current");
        title_row.append(&cur);
    }
    let sub = Label::new(Some(&QuickConnect::subtitle(&entry.config)));
    sub.set_halign(Align::Start);
    sub.add_css_class("qc-sub");
    let path = Label::new(Some(&entry.config.path));
    path.set_halign(Align::Start);
    path.add_css_class("qc-path");
    col.append(&title_row);
    col.append(&sub);
    col.append(&path);
    col
}

fn badge_label(badge: QuickBadge) -> String {
    match badge {
        QuickBadge::Recent => i18n::tr(TextKey::Recent).to_uppercase(),
        QuickBadge::Project => i18n::tr(TextKey::Project).to_uppercase(),
    }
}

fn ensure_overlay(parent: &Window) -> Overlay {
    crate::platform::linux::quick_pick::ensure_overlay(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::linux::quickconnect::model::{TargetRuntime, TargetTransport};

    fn cfg(name: &str) -> TargetConfig {
        TargetConfig::new(name, TargetRuntime::Tmux, TargetTransport::Local, "~/x")
    }

    #[test]
    fn filter_keeps_new_project_on_empty_query() {
        let items = vec![
            PanelItem::Target(
                QuickConnectEntry::new(cfg("muxterm"), vec![QuickBadge::Project]),
                false,
            ),
            PanelItem::NewProject,
        ];
        assert_eq!(filter_panel_items(&items, "").len(), 2);
        let hit = filter_panel_items(&items, "mux");
        assert_eq!(hit.len(), 1);
        assert!(matches!(hit[0], PanelItem::Target(_, _)));
        let new_only = filter_panel_items(&items, "new");
        assert_eq!(new_only.len(), 1);
        assert!(matches!(new_only[0], PanelItem::NewProject));
    }

    #[test]
    fn build_items_marks_current_and_appends_new_project() {
        let mut store = QuickConnectStore::new(None);
        let recent = cfg("recent");
        let project = cfg("project");
        store.recents.push(recent.clone());
        store.projects.push(project.clone());
        let items = build_items(&store, Some(&recent));
        assert_eq!(items.len(), 3);
        assert!(matches!(
            &items[0],
            PanelItem::Target(entry, true) if entry.config == recent
        ));
        assert!(matches!(
            &items[1],
            PanelItem::Target(entry, false) if entry.config == project
        ));
        assert!(matches!(items[2], PanelItem::NewProject));
    }

    #[test]
    fn build_items_dedupes_recent_and_project() {
        let mut store = QuickConnectStore::new(None);
        let dup = cfg("dup");
        store.recents.push(dup.clone());
        store.projects.push(dup.clone());
        let items = build_items(&store, None);
        assert_eq!(items.len(), 2, "重复目标只出现一次 + New Project");
        assert!(matches!(&items[0], PanelItem::Target(entry, false) if entry.config == dup));
        assert!(matches!(items[1], PanelItem::NewProject));
    }

    #[test]
    fn filter_matches_subtitle_and_path() {
        let ssh = TargetConfig::new(
            "srv",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/work",
        );
        let items = vec![PanelItem::Target(
            QuickConnectEntry::new(ssh, vec![]),
            false,
        )];
        assert_eq!(filter_panel_items(&items, "ryzen").len(), 1);
        assert_eq!(filter_panel_items(&items, "work").len(), 1);
        assert_eq!(filter_panel_items(&items, "nomatch").len(), 0);
    }
}
