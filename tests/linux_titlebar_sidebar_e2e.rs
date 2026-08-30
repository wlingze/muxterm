//! Linux title bar, Alt+P, and workspace sidebar e2e.

#![cfg(feature = "gtk")]

mod support;

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Paned, Revealer, ToggleButton, Widget};

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
        // 固定顶层分配，确保拖动 Paned 改的是两列宽度，不是让无窗口管理器的
        // Xvfb 测试窗口按 natural width 自行长大。
        app.window.set_default_size(960, 640);
        app.window.set_resizable(false);
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
        let content = find_by_name(&app.window, "muxterm-content")
            .expect("main split must exist")
            .downcast::<Paned>()
            .expect("main content must be a resizable Paned");
        let sidebar_shell = content
            .start_child()
            .expect("main split must have a sidebar child");
        assert_eq!(
            sidebar_shell.widget_name(),
            "muxterm-sidebar-shell",
            "first content child must be the sidebar"
        );
        let terminal_column = content
            .end_child()
            .expect("main split must have a terminal column");
        assert_eq!(
            terminal_column.widget_name(),
            "muxterm-terminal-column",
            "terminal surface and chrome must share the right column"
        );
        let status = find_by_name(&app.window, "muxterm-status-bar").expect("status bar");
        assert_eq!(
            status.parent().as_ref(),
            Some(&terminal_column),
            "status bar must not span underneath the sidebar"
        );

        let terminal_width = terminal_column.allocated_width();
        assert!(
            terminal_width < 960,
            "terminal must shrink when sidebar is open, got {terminal_width}"
        );
        let divider_width = content
            .allocated_width()
            .saturating_sub(sidebar_shell.allocated_width())
            .saturating_sub(terminal_column.allocated_width());
        assert!(
            divider_width <= 8,
            "sidebar and terminal must be adjacent; divider={divider_width}px"
        );

        let original_terminal_width = terminal_width;
        let original_sidebar_width = sidebar_shell.allocated_width();
        let original_position = content.position();
        content.set_position(original_position + 80);
        pump_main_loop(100);
        assert!(
            sidebar_shell.allocated_width() > original_sidebar_width,
            "dragging the divider must resize the sidebar: position {} -> {}, sidebar {} -> {}, terminal {} -> {}",
            original_position,
            content.position(),
            original_sidebar_width,
            sidebar_shell.allocated_width(),
            original_terminal_width,
            terminal_column.allocated_width(),
        );
        let sidebar_panel = find_by_name(&app.window, "muxterm-sidebar")
            .expect("sidebar panel")
            .downcast::<gtk4::Box>()
            .expect("sidebar panel is Box");
        let terminal = find_by_name(&app.window, "muxterm-sidebar-scroll")
            .and_then(|widget| widget.ancestor(gtk4::Widget::static_type()))
            .expect("sidebar widget has an ancestor");
        let _ = (sidebar_panel, terminal);
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
        assert!(
            terminal_column.allocated_width() >= content.allocated_width() - 8,
            "collapsed sidebar must return all horizontal space to the terminal"
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
