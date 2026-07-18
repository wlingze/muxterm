//! tmux 集成对话框。
//!
//! 打开后：
//! - 列出当前所有 tmux session（调 `tmux list-sessions`）。
//! - 选中一个 → attach（attach 到已有 session）。
//! - 「新建 session」按钮 → 输入名字 → new-session。
//! - attach/new 成功后关闭对话框，把结果回调给上层（上层据此连 tmux -CC）。

use std::process::Command;

use gtk4::prelude::*;
use gtk4::{
    Box, Button, Dialog, Entry, Label, ListBox, ListBoxRow, Orientation, ScrolledWindow,
    SelectionMode,
};

/// tmux 集成动作结果。
#[derive(Debug, Clone)]
pub enum TmuxAction {
    /// attach 到已有 session（按名字）。
    Attach { session: String },
    /// 新建 session（空名=自动命名）。
    NewSession { name: Option<String> },
}

/// 弹出 tmux 集成对话框。`on_done` 在用户选择一个动作后（UI 线程）被调用。
pub fn show<F>(parent: &impl IsA<gtk4::Window>, on_done: F)
where
    F: Fn(TmuxAction) + 'static,
{
    let dlg = Dialog::with_buttons(
        Some("tmux 集成"),
        Some(parent),
        gtk4::DialogFlags::MODAL,
        &[("关闭", gtk4::ResponseType::Close)],
    );
    dlg.set_default_size(360, 420);

    let content = dlg.content_area();
    content.set_spacing(6);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);

    let hint = Label::new(Some("选择要 attach 的 tmux session，或新建一个："));
    hint.set_halign(gtk4::Align::Start);
    content.append(&hint);

    // session 列表
    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Single);
    let sessions = list_tmux_sessions();
    if sessions.is_empty() {
        let empty = Label::new(Some("（没有检测到 tmux session）"));
        empty.set_sensitive(false);
        list.append(&empty);
    } else {
        for s in &sessions {
            let row = ListBoxRow::new();
            let label = Label::new(Some(s));
            label.set_halign(gtk4::Align::Start);
            label.set_margin_start(6);
            label.set_margin_end(6);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            row.set_child(Some(&label));
            list.append(&row);
        }
    }

    let sw = ScrolledWindow::new();
    sw.set_vexpand(true);
    sw.set_child(Some(&list));
    content.append(&sw);

    // 新建 session 行
    let new_row = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();
    let name_entry = Entry::builder()
        .placeholder_text("新 session 名（可空）")
        .hexpand(true)
        .build();
    let new_btn = Button::with_label("新建并 attach");
    new_row.append(&name_entry);
    new_row.append(&new_btn);
    content.append(&new_row);

    // 状态/错误标签
    let status = Label::new(Some(""));
    status.set_halign(gtk4::Align::Start);
    content.append(&status);

    let dlg_clone = dlg.clone();
    let status_clone = status.clone();
    let on_done_box = std::cell::RefCell::new(Some(on_done));
    let on_done_done = std::rc::Rc::new(on_done_box);

    // 双击 / 选中后点 attach：用 list row activated
    {
        let dlg = dlg_clone.clone();
        let status = status_clone.clone();
        let on_done = on_done_done.clone();
        list.connect_row_activated(move |_lb, row| {
            if let Some(label) = row.child().and_then(|w| w.downcast::<Label>().ok()) {
                let session = label.label().to_string();
                if session.starts_with("（") {
                    return;
                }
                let action = TmuxAction::Attach { session };
                if let Some(cb) = on_done.borrow_mut().take() {
                    cb(action);
                }
                dlg.close();
            } else {
                status.set_label("无法读取选中的 session");
            }
        });
    }

    // 新建按钮
    {
        let dlg = dlg_clone.clone();
        let name_entry = name_entry.clone();
        let on_done = on_done_done.clone();
        new_btn.connect_clicked(move |_| {
            let name = name_entry.text().to_string();
            let action = if name.trim().is_empty() {
                TmuxAction::NewSession { name: None }
            } else {
                TmuxAction::NewSession {
                    name: Some(name.trim().to_string()),
                }
            };
            if let Some(cb) = on_done.borrow_mut().take() {
                cb(action);
            }
            dlg.close();
        });
    }

    dlg.connect_response(move |d, resp| {
        if resp == gtk4::ResponseType::Close {
            d.close();
        }
    });

    dlg.show();
}

/// 调 `tmux list-sessions`，返回 session 名列表（失败返回空）。
fn list_tmux_sessions() -> Vec<String> {
    let out = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        }
        _ => Vec::new(),
    }
}
