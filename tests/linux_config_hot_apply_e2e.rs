//! ConfigChanged 热应用 e2e：同一个 AppWindow 内应用字号、主题和快捷键。

#![cfg(feature = "gtk")]

mod support;

use std::time::{Duration, Instant};

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

use muxterm::test_support::core::config::Config;
use muxterm::test_support::frontend::linux::window::AppWindow;

use support::linux_gtk::*;

#[test]
fn config_changed_hot_applies_theme_font_and_shortcut_without_restart() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();

        let config_home =
            std::env::temp_dir().join(format!("muxterm-config-hot-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config_home);
        std::fs::create_dir_all(config_home.join("muxterm")).unwrap();
        let previous_config_home = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);

        let app = AppWindow::new(Config::default(), load_theme());
        app.window.set_default_size(960, 640);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(80);

        app.test_commit_config_path("font.size", serde_json::json!(17.0))
            .expect("font draft transaction should commit");
        app.test_commit_config_path("theme.name", serde_json::json!("white"))
            .expect("theme draft transaction should commit");
        app.test_commit_config_path(
            "shortcuts.overrides",
            serde_json::json!([
                {
                    "action": "quick_connect",
                    "bindings": [{"key": "z", "modifiers": ["alt"]}]
                }
            ]),
        )
        .expect("shortcut draft transaction should commit");

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.test_poll_once();
            while glib::MainContext::default().iteration(false) {}
            if (app.test_font_size() - 17.0).abs() < f32::EPSILON
                && app.test_theme_name() == "white"
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(app.test_font_size(), 17.0);
        assert_eq!(app.test_theme_name(), "white");

        let controller = window_key_controller(&app.window).expect("window key controller");
        simulate_key_press(&controller, gdk::Key::z, gdk::ModifierType::ALT_MASK);
        assert!(
            app.test_panel_open(),
            "ConfigChanged must hot-apply the shortcut without recreating AppWindow"
        );

        // 保存后的 Project 必须进入面板，并经真实异步打开成为 Workspace。
        use muxterm::test_support::frontend::linux::quickconnect::model::{
            ProjectDocument, QuickConnect, TargetConfigDraft, TargetRuntime, TargetTransport,
        };
        use muxterm::test_support::frontend::linux::quickconnect_panel;
        let server = support::tmux_test_support::TmuxServerGuard::new("panel-project-create");
        let mut target = TargetConfigDraft::new(
            "panel-project",
            TargetRuntime::Tmux,
            TargetTransport::Local,
            "/tmp",
        );
        target.session = Some("panel-project".into());
        target.socket = Some(server.socket().into());
        app.test_commit_config_path(
            "projects",
            serde_json::json!([ProjectDocument::from_draft(&target)]),
        )
        .unwrap();
        pump_main_loop(100);
        quickconnect_panel::close_current();
        app.test_open_panel(0);
        pump_main_loop(100);
        let row = find_by_name(&app.window, &QuickConnect::unique_id(&target))
            .expect("saved project in panel")
            .downcast::<gtk4::ListBoxRow>()
            .unwrap();
        let list = find_by_name(&app.window, "muxterm-panel-list")
            .unwrap()
            .downcast::<gtk4::ListBox>()
            .unwrap();
        list.emit_by_name::<()>("row-activated", &[&row]);
        assert!(
            wait_until_widget(6000, || app.test_active_workspace_runtime() == "tmux"
                && app.test_active_pane_seeded()),
            "runtime={} pending={} errors={:?}",
            app.test_active_workspace_runtime(),
            app.test_open_pending(),
            app.test_notifications_recorded()
        );

        app.shutdown();
        pump_main_loop(80);
        match previous_config_home {
            Some(path) => std::env::set_var("XDG_CONFIG_HOME", path),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&config_home);
    });
}
