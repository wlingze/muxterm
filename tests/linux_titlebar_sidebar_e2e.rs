//! Linux title bar, Alt+P, and workspace sidebar e2e.

#![cfg(feature = "gtk")]

mod support;

use std::process::Command;

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Paned, Revealer, ToggleButton, Widget};

use muxterm::core::attention::state::PaneStatus;
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

fn find_by_css_class(root: &impl IsA<Widget>, class: &str) -> Option<Widget> {
    let root = root.as_ref();
    if root.has_css_class(class) {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = find_by_css_class(&widget, class) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

fn widget_owns_window_focus(win: &gtk4::Window, widget: &impl IsA<Widget>) -> bool {
    let widget = widget.as_ref();
    gtk4::prelude::GtkWindowExt::focus(win).is_some_and(|focused| {
        focused == *widget || gtk4::prelude::WidgetExt::is_ancestor(&focused, widget)
    })
}

fn entry_owns_window_focus(win: &gtk4::Window, entry: &gtk4::Entry) -> bool {
    widget_owns_window_focus(win, entry)
}

/// GTK/VTE teardown cannot safely destroy two AppWindow instances in one
/// process on the Xvfb runner. Keep each scenario in its own child process.
fn enter_isolated(test_name: &'static str) -> bool {
    if skip_no_display() {
        return false;
    }
    if std::env::var_os("MUXTERM_TITLEBAR_CHILD").is_none() {
        let executable = std::env::current_exe().expect("current test executable");
        let status = Command::new(executable)
            .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
            .env("MUXTERM_TITLEBAR_CHILD", "1")
            .status()
            .unwrap_or_else(|error| panic!("spawn GTK titlebar child {test_name}: {error}"));
        assert!(
            status.success(),
            "GTK titlebar child {test_name} exited with {status}"
        );
        return false;
    }
    true
}

#[test]
fn alt_p_panel_escape_restores_terminal_focus() {
    if !enter_isolated("alt_p_panel_escape_restores_terminal_focus") {
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

        let entry = find_by_name(&app.window, "muxterm-panel-entry")
            .expect("QuickConnect entry")
            .downcast::<gtk4::Entry>()
            .expect("QuickConnect entry type");
        assert!(
            entry_owns_window_focus(&app.window, &entry),
            "QuickConnect entry must own focus while open"
        );
        let entry_ctrl = window_key_controller(&entry).expect("QuickConnect entry controller");
        simulate_key_press(&entry_ctrl, gdk::Key::Escape, gdk::ModifierType::empty());

        assert!(!app.test_panel_open(), "Escape must close QuickConnect");
        assert!(
            app.test_active_terminal_has_focus(),
            "Escape must synchronously return keyboard focus to the active terminal"
        );
        pump_main_loop(100);
        assert!(
            app.test_active_terminal_has_focus(),
            "active terminal must keep focus after the main loop settles"
        );

        app.shutdown();
        pump_main_loop(100);
    });
}

#[test]
fn title_bar_actions_and_workspace_sidebar() {
    if !enter_isolated("title_bar_actions_and_workspace_sidebar") {
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
        let workspace_section_toggle =
            find_by_name(&app.window, "muxterm-sidebar-workspaces-toggle")
                .expect("workspace section toggle")
                .downcast::<ToggleButton>()
                .expect("workspace section toggle type");
        let agent_section_toggle = find_by_name(&app.window, "muxterm-sidebar-agents-toggle")
            .expect("agent section toggle")
            .downcast::<ToggleButton>()
            .expect("agent section toggle type");
        let sections = find_by_name(&app.window, "muxterm-sidebar-sections")
            .expect("vertical sidebar split")
            .downcast::<Paned>()
            .expect("sidebar sections must be a Paned");
        let workspace_scroll =
            find_by_name(&app.window, "muxterm-sidebar-scroll").expect("workspace section body");
        let agent_scroll =
            find_by_name(&app.window, "muxterm-sidebar-agent-scroll").expect("agent section body");
        assert!(workspace_section_toggle.is_active());
        assert!(agent_section_toggle.is_active());
        assert!(workspace_scroll.is_visible());
        assert!(agent_scroll.is_visible());

        let workspace_list = find_by_name(&app.window, "muxterm-sidebar-list")
            .expect("workspace list")
            .downcast::<gtk4::ListBox>()
            .expect("workspace list type");
        let sidebar_labels = widget_label_texts(&workspace_list);
        assert!(
            sidebar_labels.iter().any(|label| label == "shell @ local"),
            "workspace subtitle must be runtime @ transport: {sidebar_labels:?}"
        );

        let divider = sections.position();
        sections.set_position(divider + 40);
        pump_main_loop(100);
        assert!(
            sections.position() > divider,
            "two expanded sections must have an adjustable divider"
        );

        agent_section_toggle.set_active(false);
        pump_main_loop(100);
        assert!(workspace_scroll.is_visible());
        assert!(!agent_scroll.is_visible());

        workspace_section_toggle.set_active(false);
        pump_main_loop(100);
        assert!(!workspace_scroll.is_visible());
        assert!(!agent_scroll.is_visible());
        assert!(
            !sections.property::<bool>("vexpand"),
            "all collapsed section headers must stack at the top"
        );

        agent_section_toggle.set_active(true);
        pump_main_loop(100);
        assert!(!workspace_scroll.is_visible());
        assert!(agent_scroll.is_visible());
        workspace_section_toggle.set_active(true);
        pump_main_loop(100);

        let agent_list = find_by_name(&app.window, "muxterm-sidebar-agent-list")
            .expect("agent list")
            .downcast::<gtk4::ListBox>()
            .expect("agent list type");
        app.test_set_agent_attention(1, "codex", PaneStatus::Working);
        pump_main_loop(100);
        assert_eq!(
            count_widget_names(&agent_list, "muxterm-sidebar-agent-row"),
            1,
            "all detected agents must appear in the lower section"
        );
        let row = agent_list.row_at_index(0).expect("agent row");
        let dot = find_by_name(&row, "muxterm-sidebar-agent-dot").expect("agent status dot");
        assert!(dot.has_css_class("running"), "working agent must be green");

        app.test_set_agent_attention(1, "codex", PaneStatus::Blocked);
        pump_main_loop(100);
        let row = agent_list.row_at_index(0).expect("blocked agent row");
        let dot = find_by_name(&row, "muxterm-sidebar-agent-dot").expect("blocked status dot");
        assert!(
            dot.has_css_class("needs-attention"),
            "waiting agent must be yellow"
        );
        row.activate();
        pump_main_loop(100);
        let row = agent_list.row_at_index(0).expect("acknowledged agent row");
        let dot = find_by_name(&row, "muxterm-sidebar-agent-dot").expect("seen status dot");
        assert!(
            dot.has_css_class("seen"),
            "viewed agent must have no colored status dot"
        );

        app.test_set_agent_attention(1, "codex", PaneStatus::Done);
        pump_main_loop(100);
        let row = agent_list.row_at_index(0).expect("finished agent row");
        let dot = find_by_name(&row, "muxterm-sidebar-agent-dot").expect("finished status dot");
        assert!(
            dot.has_css_class("needs-attention"),
            "finished but unseen agent must be yellow"
        );
        row.activate();
        pump_main_loop(100);
        let row = agent_list
            .row_at_index(0)
            .expect("viewed finished agent row");
        let dot = find_by_name(&row, "muxterm-sidebar-agent-dot").expect("viewed finished dot");
        assert!(dot.has_css_class("seen"));

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

        let original_workspaces = app.test_workspace_replica_ids();
        app.test_open_spec(WorkspaceSpec::local_shell(
            "/tmp/muxterm-sidebar-second-workspace",
        ));
        pump_main_loop(200);
        assert_eq!(
            count_widget_names(&app.window, "muxterm-sidebar-row"),
            2,
            "sidebar must list every connected workspace"
        );
        let second_row = workspace_list
            .row_at_index(1)
            .expect("second workspace row");
        let close = find_by_css_class(&second_row, "muxterm-sidebar-close")
            .expect("workspace row must provide a close button")
            .downcast::<gtk4::Button>()
            .expect("workspace close control type");
        assert!(close.has_css_class("muxterm-sidebar-close"));
        close.emit_clicked();
        pump_main_loop(200);
        assert_eq!(
            app.test_workspace_replica_ids(),
            original_workspaces,
            "closing the active workspace must return to the stable neighboring workspace"
        );
        assert_eq!(count_widget_names(&app.window, "muxterm-sidebar-row"), 1);
        let last_row = workspace_list.row_at_index(0).expect("last workspace row");
        let close_last = find_by_css_class(&last_row, "muxterm-sidebar-close")
            .expect("last workspace still has close")
            .downcast::<gtk4::Button>()
            .expect("last workspace close type");
        close_last.emit_clicked();
        pump_main_loop(200);
        assert_eq!(app.test_workspace_replica_ids().len(), 1);
        assert_eq!(
            app.test_active_workspace_runtime(),
            "shell",
            "closing the last workspace must leave a fresh usable local shell"
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
