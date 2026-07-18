//! 命令面板（Alt+P）：弹一个输入框，输入命令名执行对应动作。
//!
//! MVP：支持 new_window/new_tab/new_pane/new_pane_vertical/tmux_attach/
//! tmux_new/search/quit。输入后回车执行，Esc 关闭。

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Box, Dialog, Entry, Label, Orientation};

/// 命令面板动作回调。`cmd` 是用户输入的命令字符串（小写已规范化）。
pub fn show<F>(parent: &impl IsA<gtk4::Window>, on_run: F)
where
    F: Fn(&str) + 'static,
{
    let dlg = Dialog::with_buttons(
        Some("命令面板"),
        Some(parent),
        gtk4::DialogFlags::MODAL,
        &[("关闭", gtk4::ResponseType::Close)],
    );
    dlg.set_default_size(420, 80);

    let content = dlg.content_area();
    content.set_spacing(6);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);

    let hint = Label::new(Some("输入命令后回车：new_window / new_tab / new_pane / new_pane_vertical / tmux_attach / tmux_new / search / quit"));
    hint.set_wrap(true);
    hint.set_halign(gtk4::Align::Start);
    content.append(&hint);

    let entry = Entry::builder()
        .placeholder_text("命令…")
        .hexpand(true)
        .build();
    content.append(&entry);

    let on_run = std::cell::RefCell::new(Some(on_done(on_run)));
    let on_run = std::rc::Rc::new(on_run);
    let dlg_for_entry = dlg.clone();
    let on_run_clone = on_run.clone();
    entry.connect_activate(move |e| {
        let text = e.text().to_string();
        if !text.is_empty() {
            if let Some(cb) = on_run_clone.borrow_mut().take() {
                cb(&text.to_lowercase());
            }
            dlg_for_entry.close();
        }
    });

    let dlg_clone = dlg.clone();
    dlg.connect_response(move |d, resp| {
        if resp == gtk4::ResponseType::Close {
            d.close();
        }
        let _ = dlg_clone;
    });

    dlg.show();
    entry.grab_focus();
}

fn on_done<F: Fn(&str) + 'static>(f: F) -> impl Fn(&str) {
    f
}

/// 搜索（Alt+R）：MVP 弹一个输入框提示，真正的终端内搜索后续 phase 实现。
pub fn show_search(parent: &impl IsA<gtk4::Window>) {
    let dlg = Dialog::with_buttons(
        Some("搜索"),
        Some(parent),
        gtk4::DialogFlags::MODAL,
        &[("关闭", gtk4::ResponseType::Close)],
    );
    let content = dlg.content_area();
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    let label = Label::new(Some("终端内搜索将在后续 phase 实现。当前可滚动查看输出。"));
    content.append(&label);
    let dlg_clone = dlg.clone();
    dlg.connect_response(move |_, _| {
        dlg_clone.close();
    });
    dlg.show();
}
