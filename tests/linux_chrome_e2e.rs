//! 占位：后续 C 填实（LINUX-PLAN §7）。

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
