//! Linux title bar, Alt+P, and workspace sidebar e2e.

#![cfg(feature = "gtk")]

mod support;

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Revealer, ToggleButton, Widget};

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

use support::linux_gtk::*;

fn count_widget_names(root: &impl IsA<Widget>, prefix: &str) -> usize {
    let root = root.as_ref();
    let mut n = usize::from(root.widget_name().starts_with(prefix));
    let mut child = root.first_child();
    while let Some(c) = child {
        n += count_widget_names(&c, prefix);
        child = c.next_sibling();
    }
    n
}

#[test]
fn alt_p_opens_quick_connect() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let app = AppWindow::new(Config::default(), load_theme());
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);

        let ctrl = window_key_controller(&app.window).expect("window key controller");
        simulate_key_press(&ctrl, gdk::Key::p, gdk::ModifierType::ALT_MASK);
        pump_main_loop(100);

        assert!(app.test_panel_open(), "Alt+P must open QuickConnect");
        assert_eq!(app.test_active_panel_tab(), 0, "Alt+P must open Workspaces");

        app.shutdown();
        pump_main_loop(100);
    });
}

#[test]
fn title_bar_actions_and_workspace_sidebar() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let app = AppWindow::new(Config::default(), load_theme());
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);

        for name in [
            "muxterm-sidebar-toggle",
            "muxterm-quick-connect-button",
            "muxterm-settings-button",
        ] {
            assert!(
                find_by_name(&app.window, name).is_some(),
                "title bar action {name} must exist"
            );
        }

        let sidebar_toggle = find_by_name(&app.window, "muxterm-sidebar-toggle")
            .expect("sidebar toggle")
            .downcast::<ToggleButton>()
            .expect("sidebar toggle is ToggleButton");
        sidebar_toggle.set_active(true);
        pump_main_loop(100);

        let sidebar = find_by_name(&app.window, "muxterm-sidebar-revealer")
            .expect("sidebar revealer")
            .downcast::<Revealer>()
            .expect("sidebar revealer");
        assert!(
            sidebar.reveals_child(),
            "sidebar toggle must reveal sidebar"
        );
        assert_eq!(
            count_widget_names(&app.window, "muxterm-sidebar-row"),
            1,
            "startup shell workspace must be listed"
        );

        app.test_open_spec(WorkspaceSpec::local_shell(
            "/tmp/muxterm-sidebar-second-workspace",
        ));
        pump_main_loop(200);
        assert_eq!(
            count_widget_names(&app.window, "muxterm-sidebar-row"),
            2,
            "sidebar must list every connected workspace"
        );

        sidebar_toggle.set_active(false);
        pump_main_loop(100);
        assert!(
            !sidebar.reveals_child(),
            "collapsed sidebar must be invisible"
        );

        let quick = find_by_name(&app.window, "muxterm-quick-connect-button")
            .expect("quick connect button")
            .downcast::<gtk4::Button>()
            .expect("quick connect button is Button");
        quick.emit_clicked();
        pump_main_loop(100);
        assert!(app.test_panel_open(), "title-bar quick connect must work");

        app.shutdown();
        pump_main_loop(100);
    });
}
