//! 真实 GTK 剪贴板 → Core → 本地/SSH 文件 → 原 pane 路径，无自动 Enter。
#![cfg(all(feature = "gtk", feature = "test-harness"))]
mod support;

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use muxterm::test_support::core::config::Config;
use muxterm::test_support::frontend::linux::{keymap::Action, window::AppWindow};
use muxterm::test_support::frontend::utils::corebridge::ClientTarget;
use support::linux_gtk::*;

#[test]
fn clipboard_image_pastes_private_png_path_locally_and_over_ssh() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        let app = AppWindow::new(Config::default(), load_theme());
        app.window.present();
        assert!(wait_until_widget(5000, || app.test_active_pane_seeded()));
        let local = app.test_active_workspace_replica_id();
        let texture = gdk::MemoryTexture::new(
            2,
            1,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(vec![255u8, 0, 0, 255, 0, 255, 0, 255]),
            8,
        );
        let png = gdk::pixbuf_get_from_texture(&texture)
            .unwrap()
            .save_to_bufferv("png", &[])
            .unwrap();
        app.window.clipboard().set_texture(&texture);
        assert!(wait_until_widget(1000, || {
            gtk4::prelude::GtkWindowExt::focus(&app.window).is_some()
        }));
        let controller = window_key_controller(&app.window).unwrap();
        simulate_key_press(&controller, gdk::Key::v, gdk::ModifierType::CONTROL_MASK);
        let path = wait_image_path(&app, 0);
        assert_eq!(std::fs::read(&path).unwrap(), png);
        assert!(wait_until_widget(2000, || app
            .test_active_pane_vte_text()
            .contains(&path)));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // 路径尚未执行；Ctrl+C 清理本测试输入行。
        app.test_send_input(b"\x03");
        std::fs::remove_file(&path).unwrap();

        assert!(
            support::sshd_test_support::loopback_sshd_available(),
            "SSH fixture required"
        );
        let ssh = support::sshd_test_support::LoopbackSshd::start("image-paste").unwrap();
        let previous = std::env::var_os("MUXTERM_SSH_CONFIG_PATH");
        ssh.apply_ssh_config_env();
        app.test_open_target(
            ClientTarget {
                name: "image-ssh".into(),
                runtime: "shell".into(),
                transport: "ssh".into(),
                target: Some(ssh.alias.clone()),
                path: "/tmp".into(),
                session: None,
                socket: None,
            },
            "image-ssh".into(),
            None,
        );
        assert!(wait_until_widget(5000, || app.test_active_pane_seeded()));
        let remote = app.test_active_workspace_replica_id();
        let before = app.test_notifications_recorded().len();
        app.test_handle_action(Action::Paste);
        // 剪贴板异步回调还没完成就切走：不得改投当前 shell。
        app.test_activate_workspace(&local);
        let path = wait_image_path(&app, before);
        let output = ssh.ssh_cmd().arg(format!("cat {path}")).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, png);
        assert!(!app.test_active_pane_vte_text().contains(&path));
        app.test_activate_workspace(&remote);
        assert!(wait_until_widget(2000, || app
            .test_active_pane_vte_text()
            .contains(&path)));
        app.test_send_input(b"\x03");
        assert!(ssh
            .ssh_cmd()
            .arg(format!("unlink {path}"))
            .status()
            .unwrap()
            .success());
        app.window.close();
        pump_main_loop(100);
        match previous {
            Some(value) => std::env::set_var("MUXTERM_SSH_CONFIG_PATH", value),
            None => std::env::remove_var("MUXTERM_SSH_CONFIG_PATH"),
        }
    });
}

/// 显式指定时才连接真实测试目标；只创建/清理本次随机临时文件，不碰会话。
#[test]
fn image_transport_explicit_remote_host() {
    let Ok(host) = std::env::var("MUXTERM_IMAGE_TEST_HOST") else {
        return;
    };
    use muxterm::test_support::core::transport::{Connect, TargetConnection};
    let bytes: Vec<_> = (0..131072).map(|i| (i % 256) as u8).collect();
    let path = Connect::new("ssh", &host)
        .store_temporary_file(&bytes, "png")
        .unwrap();
    assert!(path.starts_with("/tmp/muxterm-paste-") && path.ends_with(".png"));
    let output = std::process::Command::new("ssh")
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            &host,
            "--",
            &format!("cat {path}"),
        ])
        .output()
        .unwrap();
    let removed = std::process::Command::new("ssh")
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            &host,
            "--",
            &format!("test -f {path} && unlink {path}"),
        ])
        .status()
        .unwrap();
    assert!(removed.success());
    assert!(output.status.success());
    assert_eq!(output.stdout, bytes);
}

fn wait_image_path(app: &AppWindow, after: usize) -> String {
    let mut path = None;
    assert!(
        wait_until_widget(10000, || {
            path = app
                .test_notifications_recorded()
                .iter()
                .skip(after)
                .find_map(|s| s.strip_prefix("Image pasted: ").map(str::to_owned));
            path.is_some()
        }),
        "{:?}",
        app.test_notifications_recorded()
    );
    path.unwrap()
}
