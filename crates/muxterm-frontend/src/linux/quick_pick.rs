//! VSCode 风格快捷选择器（Quick Pick）。
//!
//! 顶部输入框模糊过滤 + 下方列表；↑↓ 选中，Enter 确认，Esc 取消。
//! 命令面板、tmux session 选择、pane 切换器都基于此组件。
//!
//! 以 Overlay 挂在父窗口上（非独立 Window），高度钳在父窗口一半内，
//! 列表在固定高度的 ScrolledWindow 内滚动，不会溢出屏幕。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk::Key;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Entry, EventControllerKey, GestureClick, Label, ListBox, ListBoxRow,
    Orientation, Overlay, ScrolledWindow, SelectionMode, Widget, Window,
};

#[path = "quick_pick_model.rs"]
mod quick_pick_model;

pub use quick_pick_model::{
    filter_items, freeform_filter, fuzzy_match, panel_list_heights, QuickPickItem, ENTRY_HEIGHT,
    FREEFORM_ID,
};

const PANEL_TOP_MARGIN: i32 = 28;
const ROW_VERTICAL_MARGIN: i32 = 2;

/// 弹出 Quick Pick。`on_done(None)` 表示取消；`Some(item)` 表示选中。
pub fn show<F>(parent: &impl IsA<Window>, placeholder: &str, items: Vec<QuickPickItem>, on_done: F)
where
    F: Fn(Option<QuickPickItem>) + 'static,
{
    let parent = parent.as_ref();
    let parent_h = parent_height(parent);
    let (panel_h, list_h) = panel_list_heights(parent_h);
    let panel_w = 520;

    let overlay = ensure_overlay(parent);

    // 半透明遮罩：点击关闭
    let backdrop = GtkBox::new(Orientation::Vertical, 0);
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    backdrop.add_css_class("quick-pick-backdrop");

    let panel = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .halign(Align::Center)
        .valign(Align::Start)
        .hexpand(false)
        .vexpand(false)
        .build();
    panel.add_css_class("quick-pick-root");
    panel.set_widget_name("muxterm-quick-pick");
    panel.set_margin_top(PANEL_TOP_MARGIN);
    panel.set_size_request(panel_w, panel_h);
    // 禁止随内容长高
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .vexpand(false)
        .build();
    entry.add_css_class("quick-pick-entry");
    entry.set_widget_name("muxterm-quick-pick-entry");
    entry.set_size_request(-1, ENTRY_HEIGHT);
    panel.append(&entry);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.set_vexpand(false);
    list.add_css_class("quick-pick-list");
    list.set_widget_name("muxterm-quick-pick-list");

    // 固定高度：不传播 natural height，由 size_request 约束，溢出则滚动
    let sw = ScrolledWindow::builder()
        .vexpand(false)
        .hexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .propagate_natural_height(false)
        .propagate_natural_width(false)
        .min_content_height(list_h)
        .max_content_height(list_h)
        .child(&list)
        .build();
    sw.set_size_request(panel_w, list_h);
    panel.append(&sw);

    overlay.add_overlay(&backdrop);
    overlay.add_overlay(&panel);

    let all_items = Rc::new(items);
    let filtered: Rc<RefCell<Vec<QuickPickItem>>> = Rc::new(RefCell::new(all_items.to_vec()));
    let done = Rc::new(RefCell::new(Some(on_done)));
    let finished = Rc::new(RefCell::new(false));

    let finish = {
        let overlay = overlay.clone();
        let backdrop = backdrop.clone();
        let panel = panel.clone();
        let done = done.clone();
        let finished = finished.clone();
        move |item: Option<QuickPickItem>| {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            overlay.remove_overlay(&backdrop);
            overlay.remove_overlay(&panel);
            if let Some(cb) = done.borrow_mut().take() {
                cb(item);
            }
        }
    };

    // 点击遮罩 → 取消
    {
        let finish = finish.clone();
        let gesture = GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            finish(None);
        });
        backdrop.add_controller(gesture);
    }

    let rebuild = {
        let list = list.clone();
        let filtered = filtered.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            for item in filtered.borrow().iter() {
                let row = ListBoxRow::new();
                row.set_activatable(true);
                let box_ = GtkBox::builder()
                    .orientation(Orientation::Vertical)
                    .spacing(1)
                    .margin_start(10)
                    .margin_end(10)
                    .margin_top(ROW_VERTICAL_MARGIN)
                    .margin_bottom(ROW_VERTICAL_MARGIN)
                    .build();
                let label = Label::builder()
                    .label(&item.label)
                    .halign(Align::Start)
                    .xalign(0.0)
                    .build();
                label.add_css_class("quick-pick-label");
                box_.append(&label);
                if let Some(detail) = &item.detail {
                    let d = Label::builder()
                        .label(detail)
                        .halign(Align::Start)
                        .xalign(0.0)
                        .build();
                    d.add_css_class("quick-pick-detail");
                    box_.append(&d);
                }
                row.set_child(Some(&box_));
                list.append(&row);
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
        }
    };

    rebuild();

    {
        let all_items = all_items.clone();
        let filtered = filtered.clone();
        let rebuild = rebuild.clone();
        let filter_pending = Rc::new(Cell::new(false));
        let finished = finished.clone();
        entry.connect_changed(move |e| {
            if *finished.borrow() || filter_pending.replace(true) {
                return;
            }
            let entry = e.clone();
            let all_items = all_items.clone();
            let filtered = filtered.clone();
            let rebuild = rebuild.clone();
            let filter_pending = filter_pending.clone();
            let finished = finished.clone();
            glib::idle_add_local_once(move || {
                filter_pending.set(false);
                if *finished.borrow() {
                    return;
                }
                *filtered.borrow_mut() = filter_items(&all_items, &entry.text());
                rebuild();
                entry.grab_focus();
            });
        });
    }

    {
        let filtered = filtered.clone();
        let finish = finish.clone();
        list.connect_row_activated(move |_lb, row| {
            let idx = row.index() as usize;
            let item = filtered.borrow().get(idx).cloned();
            finish(item);
        });
    }

    {
        let finish = finish.clone();
        let list = list.clone();
        let filtered = filtered.clone();
        let entry_for_keys = entry.clone();
        let controller = EventControllerKey::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_c, keyval, _keycode, _mods| {
            if keyval == Key::Escape {
                finish(None);
                return glib::Propagation::Stop;
            }
            if keyval == Key::Return || keyval == Key::KP_Enter {
                if let Some(row) = list.selected_row() {
                    let idx = row.index() as usize;
                    let item = filtered.borrow().get(idx).cloned();
                    finish(item);
                } else {
                    finish(None);
                }
                return glib::Propagation::Stop;
            }
            if keyval == Key::Down {
                if let Some(row) = list.selected_row() {
                    let i = row.index();
                    if let Some(next) = list.row_at_index(i + 1) {
                        list.select_row(Some(&next));
                    }
                } else if let Some(first) = list.row_at_index(0) {
                    list.select_row(Some(&first));
                }
                entry_for_keys.grab_focus();
                return glib::Propagation::Stop;
            }
            if keyval == Key::Up {
                if let Some(row) = list.selected_row() {
                    let i = row.index();
                    if i > 0 {
                        if let Some(prev) = list.row_at_index(i - 1) {
                            list.select_row(Some(&prev));
                        }
                    }
                }
                entry_for_keys.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        panel.add_controller(controller);
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

pub fn show_freeform<F>(
    parent: &impl IsA<Window>,
    placeholder: &str,
    presets: Vec<QuickPickItem>,
    on_done: F,
) where
    F: Fn(Option<QuickPickItem>) + 'static,
{
    let parent = parent.as_ref();
    let parent_h = parent_height(parent);
    let (panel_h, list_h) = panel_list_heights(parent_h);
    let panel_w = 520;

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
        .hexpand(false)
        .vexpand(false)
        .build();
    panel.add_css_class("quick-pick-root");
    panel.set_widget_name("muxterm-quick-pick");
    panel.set_margin_top(PANEL_TOP_MARGIN);
    panel.set_size_request(panel_w, panel_h);
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .vexpand(false)
        .build();
    entry.add_css_class("quick-pick-entry");
    entry.set_widget_name("muxterm-quick-pick-entry");
    entry.set_size_request(-1, ENTRY_HEIGHT);
    panel.append(&entry);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.set_vexpand(false);
    list.add_css_class("quick-pick-list");
    list.set_widget_name("muxterm-quick-pick-list");

    let sw = ScrolledWindow::builder()
        .vexpand(false)
        .hexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .propagate_natural_height(false)
        .propagate_natural_width(false)
        .min_content_height(list_h)
        .max_content_height(list_h)
        .child(&list)
        .build();
    sw.set_size_request(panel_w, list_h);
    panel.append(&sw);

    overlay.add_overlay(&backdrop);
    overlay.add_overlay(&panel);

    let all_items = Rc::new(presets);
    let filtered: Rc<RefCell<Vec<QuickPickItem>>> = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(RefCell::new(Some(on_done)));
    let finished = Rc::new(RefCell::new(false));

    let finish = {
        let overlay = overlay.clone();
        let backdrop = backdrop.clone();
        let panel = panel.clone();
        let done = done.clone();
        let finished = finished.clone();
        move |item: Option<QuickPickItem>| {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            overlay.remove_overlay(&backdrop);
            overlay.remove_overlay(&panel);
            if let Some(cb) = done.borrow_mut().take() {
                cb(item);
            }
        }
    };

    {
        let finish = finish.clone();
        let gesture = GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            finish(None);
        });
        backdrop.add_controller(gesture);
    }

    let rebuild = {
        let list = list.clone();
        let filtered = filtered.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            for item in filtered.borrow().iter() {
                let row = ListBoxRow::new();
                row.set_activatable(true);
                let box_ = GtkBox::builder()
                    .orientation(Orientation::Vertical)
                    .spacing(1)
                    .margin_start(10)
                    .margin_end(10)
                    .margin_top(ROW_VERTICAL_MARGIN)
                    .margin_bottom(ROW_VERTICAL_MARGIN)
                    .build();
                let label = Label::builder()
                    .label(&item.label)
                    .halign(Align::Start)
                    .xalign(0.0)
                    .build();
                label.add_css_class("quick-pick-label");
                box_.append(&label);
                if let Some(detail) = &item.detail {
                    let d = Label::builder()
                        .label(detail)
                        .halign(Align::Start)
                        .xalign(0.0)
                        .build();
                    d.add_css_class("quick-pick-detail");
                    box_.append(&d);
                }
                row.set_child(Some(&box_));
                list.append(&row);
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
        }
    };

    let apply_filter = Rc::new({
        let all_items = all_items.clone();
        let filtered = filtered.clone();
        let rebuild = rebuild.clone();
        move |q: &str| {
            *filtered.borrow_mut() = freeform_filter(&all_items, q);
            rebuild();
        }
    });

    apply_filter("");

    {
        let apply_filter = apply_filter.clone();
        let filter_pending = Rc::new(Cell::new(false));
        let finished = finished.clone();
        entry.connect_changed(move |e| {
            if *finished.borrow() || filter_pending.replace(true) {
                return;
            }
            let entry = e.clone();
            let apply_filter = apply_filter.clone();
            let filter_pending = filter_pending.clone();
            let finished = finished.clone();
            glib::idle_add_local_once(move || {
                filter_pending.set(false);
                if *finished.borrow() {
                    return;
                }
                apply_filter(&entry.text());
                entry.grab_focus();
            });
        });
    }

    {
        let filtered = filtered.clone();
        let finish = finish.clone();
        list.connect_row_activated(move |_lb, row| {
            let idx = row.index() as usize;
            let item = filtered.borrow().get(idx).cloned();
            finish(item);
        });
    }

    {
        let finish = finish.clone();
        let list = list.clone();
        let filtered = filtered.clone();
        let entry = entry.clone();
        let controller = EventControllerKey::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_c, keyval, _keycode, _mods| {
            if keyval == Key::Escape {
                finish(None);
                return glib::Propagation::Stop;
            }
            if keyval == Key::Return || keyval == Key::KP_Enter {
                if let Some(row) = list.selected_row() {
                    let idx = row.index() as usize;
                    let item = filtered.borrow().get(idx).cloned();
                    finish(item);
                } else {
                    let t = entry.text().trim().to_string();
                    if t.is_empty() {
                        finish(None);
                    } else {
                        finish(Some(QuickPickItem {
                            id: FREEFORM_ID.into(),
                            label: t,
                            detail: Some(crate::i18n::tr(crate::i18n::Key::FreeformUseTypedTarget)),
                        }));
                    }
                }
                return glib::Propagation::Stop;
            }
            if keyval == Key::Down {
                if let Some(row) = list.selected_row() {
                    let i = row.index();
                    if let Some(next) = list.row_at_index(i + 1) {
                        list.select_row(Some(&next));
                    }
                } else if let Some(first) = list.row_at_index(0) {
                    list.select_row(Some(&first));
                }
                entry.grab_focus();
                return glib::Propagation::Stop;
            }
            if keyval == Key::Up {
                if let Some(row) = list.selected_row() {
                    let i = row.index();
                    if i > 0 {
                        if let Some(prev) = list.row_at_index(i - 1) {
                            list.select_row(Some(&prev));
                        }
                    }
                }
                entry.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        panel.add_controller(controller);
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

fn parent_height(parent: &Window) -> i32 {
    let h = parent.height();
    if h > 80 {
        return h;
    }
    let d = parent.default_height();
    if d > 80 {
        d
    } else {
        650
    }
}

/// 确保父窗口内容包在 Overlay 里（只包一次）。
pub(crate) fn ensure_overlay(parent: &Window) -> Overlay {
    match parent.child() {
        Some(child) if child.is::<Overlay>() => child.downcast::<Overlay>().expect("Overlay"),
        Some(child) => {
            parent.set_child(None::<&Widget>);
            let ov = Overlay::new();
            ov.set_hexpand(true);
            ov.set_vexpand(true);
            ov.set_child(Some(&child));
            parent.set_child(Some(&ov));
            ov
        }
        None => {
            let ov = Overlay::new();
            ov.set_hexpand(true);
            ov.set_vexpand(true);
            parent.set_child(Some(&ov));
            ov
        }
    }
}
