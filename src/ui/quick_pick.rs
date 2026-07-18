//! VSCode 风格 Quick Pick（可复用选择器）。
//!
//! 顶部输入框模糊过滤 + 下方列表；↑↓ 选中，Enter 确认，Esc 取消。
//! 命令面板、tmux session 选择、pane 切换器都基于此组件。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk::Key;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Entry, EventControllerKey, Label, ListBox, ListBoxRow, Orientation,
    ScrolledWindow, SelectionMode, Window,
};

/// 一条可选项。
#[derive(Debug, Clone)]
pub struct QuickPickItem {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

/// 弹出 Quick Pick。`on_done(None)` 表示取消；`Some(item)` 表示选中。
pub fn show<F>(parent: &impl IsA<Window>, placeholder: &str, items: Vec<QuickPickItem>, on_done: F)
where
    F: Fn(Option<QuickPickItem>) + 'static,
{
    let win = Window::builder()
        .transient_for(parent)
        .modal(true)
        .decorated(false)
        .resizable(false)
        .default_width(520)
        .default_height(320)
        .title("Quick Pick")
        .build();
    win.add_css_class("quick-pick");

    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .build();
    root.add_css_class("quick-pick-root");

    let entry = Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    entry.add_css_class("quick-pick-entry");
    root.append(&entry);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Browse);
    list.add_css_class("quick-pick-list");

    let sw = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .min_content_height(240)
        .child(&list)
        .build();
    root.append(&sw);

    win.set_child(Some(&root));

    let all_items = Rc::new(items);
    let filtered: Rc<RefCell<Vec<QuickPickItem>>> = Rc::new(RefCell::new(all_items.to_vec()));
    let done = Rc::new(RefCell::new(Some(on_done)));
    let finished = Rc::new(RefCell::new(false));

    let finish = {
        let win = win.clone();
        let done = done.clone();
        let finished = finished.clone();
        move |item: Option<QuickPickItem>| {
            if *finished.borrow() {
                return;
            }
            *finished.borrow_mut() = true;
            if let Some(cb) = done.borrow_mut().take() {
                cb(item);
            }
            win.close();
        }
    };

    let rebuild = {
        let list = list.clone();
        let filtered = filtered.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            for (i, item) in filtered.borrow().iter().enumerate() {
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
                let _ = i; // 顺序即 ListBox 索引
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

    // 键盘：Esc 取消；↑↓ 已由 ListBox 处理；Enter 在 entry 上确认当前选中
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
        win.add_controller(controller);
    }

    {
        win.connect_close_request(move |_| {
            if !*finished.borrow() {
                if let Some(cb) = done.borrow_mut().take() {
                    cb(None);
                }
                *finished.borrow_mut() = true;
            }
            glib::Propagation::Proceed
        });
    }

    win.present();
    entry.grab_focus();
}

/// 模糊匹配：查询的每个字符按序出现在目标中（大小写不敏感）。
pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    // 优先：子串
    if t.contains(&q) {
        return true;
    }
    // 次选：子序列
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
}
