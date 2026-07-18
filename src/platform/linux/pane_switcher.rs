//! Pane 切换器（Alt+R）：按名字列出/过滤所有 pane，选中后跳转。
//!
//! 类似 VSCode Ctrl+P 打开文件，这里是打开 pane。输出内容搜索留后续。

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Dialog, Entry, Label, Orientation, Window};

use crate::platform::linux::notebook::{PaneKey, TabKey};
use crate::platform::linux::quick_pick::{self, QuickPickItem};

/// 一条可切换的 pane。
#[derive(Debug, Clone)]
pub struct PaneEntry {
    pub tab: TabKey,
    pub pane: PaneKey,
    pub name: String,
    /// 展示用：如 `1:bash` / `2:vim · pane2`
    pub label: String,
    pub detail: Option<String>,
}

/// 弹出 pane 切换器。选中后回调；取消不回调。
pub fn show<F>(parent: &impl IsA<Window>, panes: Vec<PaneEntry>, on_pick: F)
where
    F: Fn(PaneEntry) + 'static,
{
    let items: Vec<QuickPickItem> = panes
        .iter()
        .enumerate()
        .map(|(i, p)| QuickPickItem {
            id: i.to_string(),
            label: p.label.clone(),
            detail: p.detail.clone(),
        })
        .collect();

    let panes = std::rc::Rc::new(panes);
    quick_pick::show(parent, "Search panes by name…", items, move |picked| {
        if let Some(item) = picked {
            if let Ok(i) = item.id.parse::<usize>() {
                if let Some(entry) = panes.get(i).cloned() {
                    on_pick(entry);
                }
            }
        }
    });
}

/// 弹出重命名输入框。确认后回调新名字；取消不回调。
pub fn show_rename<F>(parent: &impl IsA<Window>, current: &str, on_done: F)
where
    F: Fn(String) + 'static,
{
    let dlg = Dialog::with_buttons(
        Some("Rename pane"),
        Some(parent),
        gtk4::DialogFlags::MODAL,
        &[
            ("Cancel", gtk4::ResponseType::Cancel),
            ("Rename", gtk4::ResponseType::Accept),
        ],
    );
    dlg.set_default_size(360, 100);
    let content = dlg.content_area();
    content.set_spacing(6);
    content.set_margin_start(10);
    content.set_margin_end(10);
    content.set_margin_top(8);
    content.set_margin_bottom(8);

    let hint = Label::builder()
        .label("New pane name:")
        .halign(Align::Start)
        .build();
    content.append(&hint);

    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(0)
        .build();
    let entry = Entry::builder()
        .text(current)
        .hexpand(true)
        .activates_default(true)
        .build();
    row.append(&entry);
    content.append(&row);

    dlg.set_default_response(gtk4::ResponseType::Accept);

    let on_done = std::cell::RefCell::new(Some(on_done));
    let entry_c = entry.clone();
    dlg.connect_response(move |d, resp| {
        if resp == gtk4::ResponseType::Accept {
            let name = entry_c.text().to_string();
            let name = name.trim().to_string();
            if !name.is_empty() {
                if let Some(cb) = on_done.borrow_mut().take() {
                    cb(name);
                }
            }
        }
        d.close();
    });

    dlg.show();
    entry.grab_focus();
    entry.select_region(0, -1);
}
