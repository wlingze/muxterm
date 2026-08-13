//! VSCode 风格快捷选择器（Quick Pick）。
//!
//! 顶部输入框模糊过滤 + 下方列表；↑↓ 选中，Enter 确认，Esc 取消。
//! 命令面板、tmux session 选择、pane 切换器都基于此组件。
//!
//! 以 Overlay 挂在父窗口上（非独立 Window），高度钳在父窗口一半内，
//! 列表在固定高度的 ScrolledWindow 内滚动，不会溢出屏幕。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk::Key;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Entry, EventControllerKey, GestureClick, Label, ListBox, ListBoxRow,
    Orientation, Overlay, ScrolledWindow, SelectionMode, Widget, Window,
};

/// 一条可选项。
#[derive(Debug, Clone)]
pub struct QuickPickItem {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

/// 根据父窗口高度计算面板/列表高度（纯函数，保证列表不溢出）。
/// 返回 `(panel_h, list_h)`。
pub fn panel_list_heights(parent_h: i32) -> (i32, i32) {
    let panel_h = (parent_h / 2).clamp(200, 420);
    let entry_h = 44;
    let list_h = (panel_h - entry_h - 8).max(100);
    (panel_h, list_h)
}

/// 按 query 过滤候选项（label / detail 模糊匹配）。
pub fn filter_items(items: &[QuickPickItem], query: &str) -> Vec<QuickPickItem> {
    items
        .iter()
        .filter(|it| {
            fuzzy_match(query, &it.label)
                || it.detail.as_ref().is_some_and(|d| fuzzy_match(query, d))
        })
        .cloned()
        .collect()
}

/// 弹出 Quick Pick。`on_done(None)` 表示取消；`Some(item)` 表示选中。
pub fn show<F>(parent: &impl IsA<Window>, placeholder: &str, items: Vec<QuickPickItem>, on_done: F)
where
    F: Fn(Option<QuickPickItem>) + 'static,
{
    let parent = parent.as_ref();
    let parent_h = parent_height(parent);
    let (panel_h, list_h) = panel_list_heights(parent_h);
    let entry_h = 44;
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
    panel.set_margin_top(40);
    panel.set_size_request(panel_w, panel_h);
    // 禁止随内容长高
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .vexpand(false)
        .build();
    entry.add_css_class("quick-pick-entry");
    entry.set_size_request(-1, entry_h);
    panel.append(&entry);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.set_vexpand(false);
    list.add_css_class("quick-pick-list");

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
                    .spacing(0)
                    .margin_start(8)
                    .margin_end(8)
                    .margin_top(4)
                    .margin_bottom(4)
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
        entry.connect_changed(move |e| {
            let q = e.text().to_string();
            let next: Vec<QuickPickItem> = all_items
                .iter()
                .filter(|it| {
                    fuzzy_match(&q, &it.label)
                        || it.detail.as_ref().is_some_and(|d| fuzzy_match(&q, d))
                })
                .cloned()
                .collect();
            *filtered.borrow_mut() = next;
            rebuild();
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
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        panel.add_controller(controller);
    }

    entry.grab_focus();
}

/// 带自由输入的 Quick Pick：输入框非空时，始终把当前文本作为首选项。
///
/// 用于 SSH 目标等「可从列表选、也可直接敲」的场景。选中自由输入项时
/// `id == FREEFORM_ID`。
pub const FREEFORM_ID: &str = "__typed__";

/// 自由输入过滤（纯函数）：query 非空时首项为 typed target。
pub fn freeform_filter(presets: &[QuickPickItem], query: &str) -> Vec<QuickPickItem> {
    let mut next = Vec::new();
    let qtrim = query.trim();
    if !qtrim.is_empty() {
        next.push(QuickPickItem {
            id: FREEFORM_ID.into(),
            label: qtrim.to_string(),
            detail: Some(crate::platform::i18n::tr(
                crate::platform::i18n::Key::FreeformUseTypedTarget,
            )),
        });
    }
    for it in presets {
        if qtrim.is_empty()
            || fuzzy_match(qtrim, &it.label)
            || it.detail.as_ref().is_some_and(|d| fuzzy_match(qtrim, d))
        {
            next.push(it.clone());
        }
    }
    next
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
    let entry_h = 44;
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
    panel.set_margin_top(40);
    panel.set_size_request(panel_w, panel_h);
    panel.set_overflow(gtk4::Overflow::Hidden);

    let entry = Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .vexpand(false)
        .build();
    entry.add_css_class("quick-pick-entry");
    entry.set_size_request(-1, entry_h);
    panel.append(&entry);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.set_vexpand(false);
    list.add_css_class("quick-pick-list");

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
                    .spacing(0)
                    .margin_start(8)
                    .margin_end(8)
                    .margin_top(4)
                    .margin_bottom(4)
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
        entry.connect_changed(move |e| {
            apply_filter(&e.text());
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
                            detail: Some(crate::platform::i18n::tr(
                                crate::platform::i18n::Key::FreeformUseTypedTarget,
                            )),
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
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        panel.add_controller(controller);
    }

    entry.grab_focus();
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

/// 模糊匹配：查询的每个字符按序出现在目标中（大小写不敏感）。
pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t.contains(&q) {
        return true;
    }
    let mut ti = t.chars().peekable();
    for qc in q.chars() {
        loop {
            match ti.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_empty_matches_all() {
        assert!(fuzzy_match("", "tmux: attach"));
    }

    #[test]
    fn fuzzy_substring() {
        assert!(fuzzy_match("tmux", "tmux: attach to session"));
        assert!(fuzzy_match("tab", "new tab"));
        assert!(!fuzzy_match("zzz", "new tab"));
    }

    #[test]
    fn fuzzy_subsequence() {
        assert!(fuzzy_match("ntb", "new tab"));
        assert!(fuzzy_match("tcns", "tmux: create new session"));
    }

    /// 对应：模糊匹配大小写不敏感。
    #[test]
    fn test_quick_pick_fuzzy_case_insensitive() {
        assert!(fuzzy_match("TMUX", "tmux: attach"));
        assert!(fuzzy_match("NeW tAb", "new tab"));
        assert!(fuzzy_match("ntb", "NEW TAB"));
    }

    /// 对应：中文命令名可匹配。
    #[test]
    fn test_quick_pick_fuzzy_chinese() {
        assert!(fuzzy_match("命令", "打开命令面板"));
        assert!(fuzzy_match("面板", "打开命令面板"));
        assert!(!fuzzy_match("窗口", "打开命令面板"));
    }

    #[test]
    fn test_quick_pick_fuzzy_no_match() {
        assert!(!fuzzy_match("zzz", "new tab"));
        assert!(!fuzzy_match("abcdef", "ab"));
    }

    /// 对应：过滤保持输入顺序（无额外排序）。
    #[test]
    fn test_quick_pick_filter_preserves_order() {
        let items = vec![
            QuickPickItem {
                id: "a".into(),
                label: "new tab".into(),
                detail: None,
            },
            QuickPickItem {
                id: "b".into(),
                label: "tmux: attach".into(),
                detail: None,
            },
            QuickPickItem {
                id: "c".into(),
                label: "close tab".into(),
                detail: None,
            },
        ];
        let f = filter_items(&items, "tab");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].id, "a");
        assert_eq!(f[1].id, "c");
    }

    #[test]
    fn test_quick_pick_filter_empty_query_keeps_all() {
        let items = vec![QuickPickItem {
            id: "1".into(),
            label: "x".into(),
            detail: Some("detail".into()),
        }];
        assert_eq!(filter_items(&items, "").len(), 1);
    }

    #[test]
    fn test_quick_pick_filter_empty_list() {
        assert!(filter_items(&[], "anything").is_empty());
    }

    /// 对应：命令面板滚动——列表高度钳制，不超过面板可用区。
    #[test]
    fn test_quick_pick_list_height_clamped() {
        let (panel, list) = panel_list_heights(900);
        assert_eq!(panel, 420); // clamp 上限
        assert_eq!(list, panel - 44 - 8);
        assert!(list <= panel);

        let (panel2, list2) = panel_list_heights(100);
        assert_eq!(panel2, 200); // clamp 下限
        assert_eq!(list2, 148); // 200 - 44 - 8 = 148 (>100)
        assert!(list2 <= panel2);
        assert!(list2 >= 100);
    }

    #[test]
    fn test_quick_pick_filter_matches_detail() {
        let items = vec![QuickPickItem {
            id: "s".into(),
            label: "session".into(),
            detail: Some("main · 2 windows".into()),
        }];
        assert_eq!(filter_items(&items, "windows").len(), 1);
    }

    #[test]
    fn test_quick_pick_freeform_filter_prepends_typed() {
        let presets = vec![QuickPickItem {
            id: "cfg".into(),
            label: "alice@box:22".into(),
            detail: Some("from config".into()),
        }];
        let f = freeform_filter(&presets, "bob@h");
        assert_eq!(f[0].id, FREEFORM_ID);
        assert_eq!(f[0].label, "bob@h");
        // 不匹配预设时只有 typed 一项
        assert_eq!(f.len(), 1);

        let f2 = freeform_filter(&presets, "alice");
        assert_eq!(f2[0].id, FREEFORM_ID);
        assert!(f2.iter().any(|i| i.id == "cfg"));

        let empty_q = freeform_filter(&presets, "");
        assert_eq!(empty_q.len(), 1);
        assert_eq!(empty_q[0].id, "cfg");
    }
}
