//! 占位 e2e crate：后续 C 在此实现真实用例。
//!
//! 本 crate 不构造 `AppWindow`（LINUX-PLAN §0.1/§0.5）。

#![cfg(feature = "gtk")]

mod support;

use support::linux_gtk::*;

#[test]
fn placeholder_compiles() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
    });
}
