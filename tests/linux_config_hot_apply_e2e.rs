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

fn find_close(root: &impl IsA<gtk4::Widget>) -> Option<gtk4::Widget> {
    let root = root.as_ref();
    if root.has_css_class("muxterm-sidebar-close") {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = find_close(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

fn assert_osc_colours(socket: &str, pane: u32, fg: &str, bg: &str) {
    let target = format!("%{pane}");
    let script = r#"python3 -c 'import os,sys,tty,termios,select,time; f=sys.stdin.fileno(); old=termios.tcgetattr(f); tty.setraw(f); os.write(1,b"\x1b[2J\x1b[H\x1b]10;?\x07\x1b]11;?\x07"); data=b""; end=time.monotonic()+1
while time.monotonic()<end:
 if select.select([f],[],[],0.05)[0]: data+=os.read(f,4096)
termios.tcsetattr(f,termios.TCSANOW,old); print("OSC_RESULT",repr(data))'
"#;
    assert!(std::process::Command::new("tmux")
        .args(["-L", socket, "send-keys", "-t", &target, "-l", script])
        .status()
        .unwrap()
        .success());
    let mut screen = String::new();
    assert!(
        wait_until_widget(3000, || {
            let output = std::process::Command::new("tmux")
                .args(["-L", socket, "capture-pane", "-p", "-J", "-t", &target])
                .output()
                .unwrap();
            screen = String::from_utf8_lossy(&output.stdout).into_owned();
            screen.contains("OSC_RESULT") && screen.contains(fg) && screen.contains(bg)
        }),
        "OSC colours: {screen}"
    );
}

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
        let shell_workspace = app.test_active_workspace_replica_id();

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
        // 普通 Project 编辑器只填写 name/path，没有显式 session。
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

        let inspect = || {
            let output = std::process::Command::new("tmux")
                .args([
                    "-L",
                    server.socket(),
                    "list-panes",
                    "-a",
                    "-F",
                    "#{session_name}|#{pane_current_path}",
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(inspect().trim(), "panel-project|/tmp");
        find_by_name(&app.window, "muxterm-sidebar-toggle")
            .unwrap()
            .downcast::<gtk4::ToggleButton>()
            .unwrap()
            .set_active(true);
        pump_main_loop(200);
        find_close(&app.window)
            .unwrap()
            .downcast::<gtk4::Button>()
            .unwrap()
            .emit_clicked();
        pump_main_loop(100);
        quickconnect_panel::close_current();
        app.test_open_panel(0);
        pump_main_loop(100);
        let row = find_by_name(&app.window, &QuickConnect::unique_id(&target))
            .expect("project remains after detach")
            .downcast::<gtk4::ListBoxRow>()
            .unwrap();
        let list = find_by_name(&app.window, "muxterm-panel-list")
            .unwrap()
            .downcast::<gtk4::ListBox>()
            .unwrap();
        list.emit_by_name::<()>("row-activated", &[&row]);
        assert!(
            wait_until_widget(6000, || !app.test_open_pending()
                && app.test_active_workspace_runtime() == "tmux"
                && app.test_active_pane_seeded()),
            "reattach errors: {:?}",
            app.test_notifications_recorded()
        );
        assert_eq!(
            inspect().trim(),
            "panel-project|/tmp",
            "reopening a Project must attach without creating another session"
        );

        // 真实 server 的 OSC 代答颜色必须跟随前端；不能只检查 GTK 主题名字。
        let first_pane = app.test_active_pane_id();
        assert_osc_colours(
            server.socket(),
            first_pane,
            "10;rgb:1f1f/2323/2828",
            "11;rgb:ffff/ffff/ffff",
        );
        let tmux_workspace = app.test_active_workspace_replica_id();
        app.test_handle_action(muxterm::test_support::frontend::linux::keymap::Action::NewTab);
        assert!(wait_until_widget(5000, || app.test_active_pane_id()
            != first_pane
            && app.test_active_pane_seeded()));
        let second_pane = app.test_active_pane_id();
        assert_osc_colours(
            server.socket(),
            second_pane,
            "10;rgb:1f1f/2323/2828",
            "11;rgb:ffff/ffff/ffff",
        );
        app.test_activate_workspace(&shell_workspace);
        app.test_commit_config_path("theme.name", serde_json::json!("black"))
            .unwrap();
        pump_main_loop(100);
        for pane in [first_pane, second_pane] {
            assert_osc_colours(
                server.socket(),
                pane,
                "10;rgb:e6e6/e8e8/ebeb",
                "11;rgb:0b0b/0d0d/1010",
            );
        }
        app.test_commit_config_path("theme.name", serde_json::json!("white"))
            .unwrap();
        pump_main_loop(100);
        for pane in [first_pane, second_pane] {
            assert_osc_colours(
                server.socket(),
                pane,
                "10;rgb:1f1f/2323/2828",
                "11;rgb:ffff/ffff/ffff",
            );
        }
        app.test_activate_workspace(&tmux_workspace);

        let herdr = support::herdr_test_support::herdr_available()
            .then(|| support::herdr_test_support::IsolatedHerdr::start("panel-project"));
        let sshd = (herdr.is_some() && support::sshd_test_support::loopback_sshd_available())
            .then(|| support::sshd_test_support::LoopbackSshd::start("project-herdr").unwrap());
        if let Some(sshd) = &sshd {
            std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &sshd.config_path);
        }
        if let Some(herdr) = &herdr {
            std::env::set_var("HERDR_SOCKET_PATH", herdr.socket_path());
            let mut target = TargetConfigDraft::new(
                "herdr-panel-project",
                TargetRuntime::Herdr,
                TargetTransport::Local,
                "/tmp",
            );
            target.session = Some(herdr.name().into());
            target.socket = Some(herdr.socket_path().to_string_lossy().into_owned());
            app.test_commit_config_path(
                "projects",
                serde_json::json!([ProjectDocument::from_draft(&target)]),
            )
            .unwrap();
            pump_main_loop(100);
            for attempt in 0..if sshd.is_some() { 4 } else { 2 } {
                if attempt == 2 {
                    target.name = "herdr-ssh-project".into();
                    target.transport = TargetTransport::Ssh {
                        name: sshd.as_ref().unwrap().alias.clone(),
                    };
                    app.test_commit_config_path(
                        "projects",
                        serde_json::json!([ProjectDocument::from_draft(&target)]),
                    )
                    .unwrap();
                    pump_main_loop(100);
                }
                quickconnect_panel::close_current();
                app.test_open_panel(0);
                pump_main_loop(100);
                let row = find_by_name(&app.window, &QuickConnect::unique_id(&target))
                    .unwrap()
                    .downcast::<gtk4::ListBoxRow>()
                    .unwrap();
                let list = find_by_name(&app.window, "muxterm-panel-list")
                    .unwrap()
                    .downcast::<gtk4::ListBox>()
                    .unwrap();
                list.emit_by_name::<()>("row-activated", &[&row]);
                assert!(
                    wait_until_widget(8000, || !app.test_open_pending()
                        && app.test_active_workspace_runtime() == "herdr"
                        && app.test_active_pane_seeded()),
                    "Herdr attempt {attempt}: {:?}",
                    app.test_notifications_recorded()
                );
                assert!(app.test_emit_active_pane_commit("printf 'HERDR_CWD=%s\\n' \"$PWD\"\r"));
                assert!(
                    wait_until_widget(3000, || app
                        .test_active_pane_vte_text()
                        .contains("HERDR_CWD=/tmp")),
                    "Herdr Project must be interactive in its configured cwd"
                );
                let output = herdr.cli().args(["workspace", "list"]).output().unwrap();
                assert!(output.status.success());
                let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                let workspaces = json["result"]["workspaces"]
                    .as_array()
                    .expect("workspace list");
                assert_eq!(
                    workspaces.len(),
                    if attempt < 2 { 1 } else { 2 },
                    "Project reopen must reuse Herdr identity: {json}"
                );
                if attempt % 2 == 0 {
                    pump_main_loop(200);
                    let list = find_by_name(&app.window, "muxterm-sidebar-list")
                        .unwrap()
                        .downcast::<gtk4::ListBox>()
                        .unwrap();
                    let mut index = 0;
                    loop {
                        let row = list.row_at_index(index).expect("active Herdr row");
                        if row.has_css_class("active") {
                            find_close(&row)
                                .unwrap()
                                .downcast::<gtk4::Button>()
                                .unwrap()
                                .emit_clicked();
                            break;
                        }
                        index += 1;
                    }
                    pump_main_loop(200);
                }
            }
            std::env::remove_var("HERDR_SOCKET_PATH");
        }

        app.shutdown();
        pump_main_loop(80);
        std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
        drop(sshd);
        drop(herdr);
        match previous_config_home {
            Some(path) => std::env::set_var("XDG_CONFIG_HOME", path),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&config_home);
    });
}
