//! GTK fault 兜底：glib trampoline 不能 unwind，进 C 之前先 catch_unwind。
//!
//! - [`run`]：包住 16ms poll / idle / connect_* 回调，Err 走
//!   `core::fault::report` + 弹 `muxterm-fault-dialog`，进程继续。
//! - 同时最多一个对话框；OK 按钮 `muxterm-fault-dialog-ok`。

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Window};

use crate::core::fault;
use crate::platform::i18n::{self, TextKey};

/// 全局「已弹过对话框」标记（同时最多一个）。
static DIALOG_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 包住一个会进 glib 回调的闭包：panic 被接住、报告、弹窗，返回 None。
pub fn run<T>(label: &str, f: impl FnOnce() -> T) -> Option<T> {
    fault::run(label, f).or_else(|| {
        show_fault_dialog();
        None
    })
}

/// 弹内部错误对话框（标题 internal_error，正文含第一行 panic 信息 +
/// internal_error_logged）。同时最多一个。
pub fn show_fault_dialog() {
    if DIALOG_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let message = fault::last_message().unwrap_or_else(|| "unknown".to_string());
    let first_line = message.lines().next().unwrap_or(&message).to_string();
    let dialog = Window::builder()
        .title(i18n::tr(TextKey::InternalError))
        .modal(true)
        .default_width(480)
        .build();
    dialog.set_widget_name("muxterm-fault-dialog");
    let vbox = GtkBox::new(Orientation::Vertical, 10);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);
    let body = Label::new(Some(&format!(
        "{}\n\n{}",
        first_line,
        i18n::tr(TextKey::InternalErrorLogged)
    )));
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.set_yalign(0.0);
    let ok = Button::with_label("OK");
    ok.set_widget_name("muxterm-fault-dialog-ok");
    ok.set_halign(Align::End);
    let dlg = dialog.clone();
    ok.connect_clicked(move |_| dlg.close());
    vbox.append(&body);
    vbox.append(&ok);
    dialog.set_child(Some(&vbox));
    dialog.present();
}

/// 测试用：重置「已弹过」标记（每个测试一个 AppWindow，隔离状态）。
pub fn reset_dialog_shown_for_test() {
    DIALOG_SHOWN.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 测试用：当前是否已弹过 fault 对话框。
pub fn dialog_shown() -> bool {
    DIALOG_SHOWN.load(std::sync::atomic::Ordering::SeqCst)
}

/// 测试用：手动注入一次 fault（走 report + 弹窗，不真的炸 emulate）。
pub fn inject_fault(token: &str) {
    fault::report("test.inject", Box::new(token.to_string()));
    show_fault_dialog();
}

/// 占位：Rc 生命周期内保持对话框引用（防止被 GC）。
#[allow(dead_code)]
struct _DialogGuard(Rc<Cell<bool>>);
