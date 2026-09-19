//! Linux pane context menu e2e.

#![cfg(feature = "gtk")]

mod support;

use gtk4::prelude::*;

use muxterm::test_support::core::config::Config;
use muxterm::test_support::frontend::linux::window::AppWindow;

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

        let source_tab = app.test_active_tab_id();
        let focus_pane = app.test_layout_leaf_ids()[0];
        let focus_header =
            find_by_name(&app.window, &format!("muxterm-pane-header-{focus_pane}")).unwrap();
        focus_header.next_sibling().unwrap().grab_focus();
        app.test_handle_action(
            muxterm::test_support::frontend::linux::keymap::Action::TogglePaneFullscreen,
        );
        pump_main_loop(100);
        assert_eq!(app.test_active_pane_id(), focus_pane);
        assert!(
            app.test_gtk_paned_orientations().is_empty(),
            "放大后 GTK 只显示输入 pane"
        );
        app.test_handle_action(
            muxterm::test_support::frontend::linux::keymap::Action::TogglePaneFullscreen,
        );
        pump_main_loop(100);
        assert_eq!(app.test_layout_leaf_ids().len(), panes);
        assert!(!app.test_gtk_paned_orientations().is_empty());
        let pane = app.test_active_pane_id();
        let header = find_by_name(&app.window, &format!("muxterm-pane-header-{pane}"))
            .expect("multi-pane title bar");
        assert!(header.is_visible());
        assert!(header.height() >= 28);
        let resets = app.test_pane_render_trace(pane).0;
        app.test_set_agent_attention(
            pane,
            "codex",
            muxterm::test_support::frontend::utils::corebridge::ClientAttentionStatus::Working,
        );
        find_by_name(&app.window, "muxterm-sidebar-agents")
            .unwrap()
            .downcast::<gtk4::Button>()
            .unwrap()
            .emit_clicked();
        pump_main_loop(100);
        assert_eq!(app.test_active_tab_id(), source_tab);
        assert_eq!(app.test_layout_leaf_ids().len(), panes);
        assert_eq!(
            app.test_pane_render_trace(pane).0,
            resets,
            "聚合不能重置 Surface"
        );
        assert!(!find_by_name(&app.window, "muxterm-new-tab")
            .unwrap()
            .is_sensitive());
        find_by_name(&app.window, "muxterm-status-tab-close-1")
            .unwrap()
            .downcast::<gtk4::Button>()
            .unwrap()
            .emit_clicked();
        pump_main_loop(100);
        assert_eq!(
            app.test_tab_and_pane_counts().1,
            panes,
            "隐藏投影不得关闭源 pane"
        );
        find_by_name(&app.window, "muxterm-sidebar-shells")
            .unwrap()
            .downcast::<gtk4::Button>()
            .unwrap()
            .emit_clicked();
        pump_main_loop(100);
        assert_eq!(app.test_active_tab_id(), source_tab);

        app.test_open_panel(0);
        pump_main_loop(100);
        assert!(find_by_name(&app.window, "muxterm-panel-workspace-navigation").is_some());
        find_by_name(&app.window, "muxterm-panel-workspace-S")
            .unwrap()
            .downcast::<gtk4::Button>()
            .unwrap()
            .emit_clicked();
        pump_main_loop(100);
        assert!(!app.test_panel_open());
        assert_eq!(app.test_active_tab_id(), source_tab);
        find_by_name(&app.window, "muxterm-sidebar-toggle")
            .unwrap()
            .downcast::<gtk4::ToggleButton>()
            .unwrap()
            .set_active(true);
        app.window.set_default_size(1100, 720);
        pump_main_loop(400);
        gtk4::test_widget_wait_for_draw(&app.window);

        if let Some(path) = std::env::var_os("MUXTERM_UI_SCREENSHOT") {
            let paintable = gtk4::WidgetPaintable::new(Some(&app.window));
            let snapshot = gtk4::Snapshot::new();
            paintable.snapshot(
                &snapshot,
                f64::from(app.window.width()),
                f64::from(app.window.height()),
            );
            let node = snapshot.to_node().expect("rendered window");
            app.window
                .renderer()
                .unwrap()
                .render_texture(&node, None)
                .save_to_png(path)
                .unwrap();
        }

        app.shutdown();
        pump_main_loop(100);
    });
}
