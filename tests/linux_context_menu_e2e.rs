//! Linux pane context menu e2e.

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;

use muxterm::core::config::Config;
use muxterm::platform::linux::window::AppWindow;

use support::linux_gtk::*;

#[test]
fn pane_context_menu_splits_active_pane() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let app = AppWindow::new(Config::default(), load_theme());
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);

        for name in [
            "muxterm-pane-menu",
            "muxterm-pane-menu-copy",
            "muxterm-pane-menu-paste",
            "muxterm-pane-menu-split-vertical",
            "muxterm-pane-menu-split-horizontal",
        ] {
            assert!(
                find_by_name(&app.window, name).is_some(),
                "pane context menu must expose {name}"
            );
        }

        let split = find_by_name(&app.window, "muxterm-pane-menu-split-vertical")
            .expect("split vertical menu item")
            .downcast::<gtk4::Button>()
            .expect("split menu item is Button");
        split.emit_clicked();
        pump_main_loop(500);
        app.test_poll_once();
        pump_main_loop(200);

        let (_, panes) = app.test_tab_and_pane_counts();
        assert!(
            panes >= 2,
            "pane menu split must create a second pane, got {panes}"
        );

        app.shutdown();
        pump_main_loop(100);
    });
}
