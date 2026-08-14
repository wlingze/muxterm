//! `tests/support/linux_gtk.rs` 共享助手的独立单测。
//!
//! 本 crate 不构造 `AppWindow`，只验证普通 GTK widget 树查找契约
//! （LINUX-PLAN C0.1：`find_by_name` 必须能找到带 `widget_name` 的控件）。

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;
use support::linux_gtk::*;

#[test]
fn find_by_name_locates_named_widget() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let label = gtk4::Label::new(Some("hello"));
        label.set_widget_name("muxterm-test-label");
        let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        box_.append(&label);

        let found =
            find_by_name(&box_, "muxterm-test-label").expect("应找到带 widget_name 的 Label");
        assert_eq!(found.widget_name(), "muxterm-test-label");
        assert!(find_by_name(&box_, "missing").is_none());
    });
}
