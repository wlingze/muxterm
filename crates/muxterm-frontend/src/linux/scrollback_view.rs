//! 只读 scrollback/peek 视图（LINUX-PLAN §10 C3.3）。
//!
//! 用 GtkTextView 展示 ReplicaStore 副本尾部，不碰 VTE 缓冲。

use gtk4::prelude::*;
use gtk4::{ScrolledWindow, TextView};

/// 构建只读 peek 视图（widget_name = `muxterm-peek-view`）。
pub fn peek_view() -> (ScrolledWindow, TextView) {
    let view = TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_wrap_mode(gtk4::WrapMode::None);
    view.set_widget_name("muxterm-peek-view");
    view.add_css_class("muxterm-peek-view");
    let sw = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&view)
        .build();
    (sw, view)
}

/// 设置 peek 文本（多行副本尾部）。
pub fn set_peek_text(view: &TextView, text: &str) {
    view.buffer().set_text(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_view_is_readonly_and_named() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("skip: 无 DISPLAY");
            return;
        }
        gtk4::test_synced(|| {
            let (_sw, view) = peek_view();
            assert_eq!(view.widget_name(), "muxterm-peek-view");
            assert!(!view.is_editable());
            set_peek_text(&view, "line1\nline2");
            let buf = view.buffer();
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            assert_eq!(text.as_str(), "line1\nline2");
        });
    }
}
