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

use crate::platform::i18n::{self, Key as TextKey};
use crate::platform::linux::quick_pick;
use crate::platform::linux::quickconnect::model::{
    QuickBadge, QuickConnect, QuickConnectEntry, TargetConfig,
};
use crate::platform::linux::quickconnect::store::QuickConnectStore;

const NEW_PROJECT_ID: &str = "__new_project__";

/// 面板回调。
pub struct QuickConnectCallbacks {
    pub on_connect: Box<dyn Fn(TargetConfig)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
}

#[derive(Debug, Clone)]
pub(crate) enum PanelItem {
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

fn build_items(store: &QuickConnectStore, current: Option<&TargetConfig>) -> Vec<PanelItem> {
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
pub fn show(
    parent: &impl IsA<Window>,
    store: &QuickConnectStore,
    current: Option<TargetConfig>,
    callbacks: QuickConnectCallbacks,
) {
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
    panel.set_margin_top(40);
    panel.set_size_request(panel_w, panel_h);
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(i18n::tr(TextKey::QuickConnectPlaceholder))
        .hexpand(true)
        .build();
    entry.add_css_class("quick-pick-entry");
    entry.set_margin_start(12);
    entry.set_margin_end(12);
    entry.set_margin_top(12);
    panel.append(&entry);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.add_css_class("quick-pick-list");

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

    overlay.add_overlay(&backdrop);
    overlay.add_overlay(&panel);

    let all = Rc::new(build_items(store, current.as_ref()));
    let visible: Rc<RefCell<Vec<PanelItem>>> = Rc::new(RefCell::new(all.to_vec()));
    let callbacks = Rc::new(callbacks);
    let finished = Rc::new(RefCell::new(false));

    let dismiss = {
        let overlay = overlay.clone();
        let backdrop = backdrop.clone();
        let panel = panel.clone();
        let finished = finished.clone();
        move || {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            overlay.remove_overlay(&backdrop);
            overlay.remove_overlay(&panel);
        }
    };

    {
        let dismiss = dismiss.clone();
        let gesture = GestureClick::new();
        gesture.connect_released(move |_, _, _, _| dismiss());
        backdrop.add_controller(gesture);
    }

    let rebuild = {
        let list = list.clone();
        let visible = visible.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            for (i, item) in visible.borrow().iter().enumerate() {
                let row = ListBoxRow::new();
                row.set_activatable(true);
                match item {
                    PanelItem::Target(entry, is_current) => {
                        row.set_widget_name(&QuickConnect::unique_id(&entry.config));
                        if *is_current {
                            row.add_css_class("qc-current");
                        }
                        row.set_child(Some(&target_row(entry, *is_current)));
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
                        row.add_controller(dbl);
                    }
                    PanelItem::NewProject => {
                        row.set_widget_name(NEW_PROJECT_ID);
                        let label =
                            Label::new(Some(&format!("＋ {}", i18n::tr(TextKey::NewProject))));
                        label.set_halign(Align::Start);
                        label.set_margin_start(16);
                        label.set_margin_top(10);
                        label.set_margin_bottom(10);
                        row.set_child(Some(&label));
                    }
                }
                list.append(&row);
                if i == 0 {
                    list.select_row(Some(&row));
                }
            }
        }
    };
    rebuild();

    {
        let visible = visible.clone();
        let all = all.clone();
        let rebuild = rebuild.clone();
        entry.connect_changed(move |e| {
            *visible.borrow_mut() = filter_panel_items(&all, &e.text());
            rebuild();
        });
    }

    let activate = {
        let list = list.clone();
        let visible = visible.clone();
        let callbacks = callbacks.clone();
        let dismiss = dismiss.clone();
        move || {
            let Some(row) = list.selected_row() else {
                return;
            };
            let idx = row.index() as usize;
            let item = visible.borrow().get(idx).cloned();
            match item {
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
    };

    list.connect_row_activated({
        let activate = activate.clone();
        move |_, _| activate()
    });

    {
        let dismiss = dismiss.clone();
        let activate = activate.clone();
        let list = list.clone();
        let controller = EventControllerKey::new();
        controller.connect_key_pressed(move |_c, key, _code, _mods| match key {
            Key::Escape => {
                dismiss();
                glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter => {
                activate();
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
