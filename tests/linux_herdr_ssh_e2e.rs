//! SSH Herdr GTK 输入契约：真实 loopback sshd + 双 Unix socket forward。
//!
//! 命令文本不包含期望输出 token；只有 VTE commit 逐字输入和 Enter 真正在
//! 远端 shell 执行后，observe 才能把 token 送回 VTE 与 PaneBuf。

#![cfg(feature = "gtk")]

mod support;

use std::time::Instant;

use gtk4::prelude::*;
use support::herdr_test_support::{herdr_available, IsolatedHerdr};
use support::linux_gtk::*;
use support::sshd_test_support::{loopback_sshd_available, LoopbackSshd};

use muxterm::core::config::Config;
use muxterm::core::workspace::spec::WorkspaceSpec;
use muxterm::platform::linux::window::AppWindow;

const INPUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

struct SshConfigEnvGuard;

impl Drop for SshConfigEnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
    }
}

#[test]
fn linux_ssh_herdr_vte_input_executes_remote_command() {
    if skip_no_display() {
        return;
    }
    if !loopback_sshd_available() {
        eprintln!("skip: 无 sshd 二进制，无法自启 loopback sshd");
        return;
    }
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }

    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let sshd = LoopbackSshd::start("herdr-input").expect("启动 loopback sshd 失败");
        sshd.apply_ssh_config_env();
        let _ssh_env = SshConfigEnvGuard;
        let herdr = IsolatedHerdr::start("ssh-input-gtk");
        let (workspace_id, _tab, _pane) = herdr.create_workspace("/tmp", "mux-ssh-input-gtk");

        let app = AppWindow::new(Config::default(), load_theme());
        app.window.set_default_size(1280, 800);
        app.window.present();
        gtk4::test_widget_wait_for_draw(&app.window);
        pump_main_loop(120);

        app.test_open_spec(WorkspaceSpec::ssh_herdr(
            sshd.alias.clone(),
            herdr.name(),
            workspace_id,
            herdr.socket_path().to_string_lossy(),
        ));
        assert!(
            app.test_workspace_runtimes().iter().any(|r| r == "herdr"),
            "SSH attach 必须打开 Herdr runtime: {:?}",
            app.test_workspace_runtimes()
        );
        assert!(
            app.test_workspace_replica_ids()
                .iter()
                .any(|id| id.contains(&sshd.alias)),
            "SSH Herdr workspace id 必须保留 loopback alias: {:?}",
            app.test_workspace_replica_ids()
        );

        // 引号使输入回显不包含连续 token；只有远端 shell 执行 echo 后才会命中。
        let command = "echo HERDR_EXEC_\"GTKSSH\"";
        let output_token = "HERDR_EXEC_GTKSSH";
        assert!(!command.contains(output_token));
        assert!(app.test_search_all(output_token).is_empty());
        for ch in command.chars() {
            assert!(
                app.test_emit_active_pane_commit(&ch.to_string()),
                "SSH Herdr active VTE 必须存在"
            );
        }
        assert!(app.test_emit_active_pane_commit("\r"));

        let pane = app.test_active_pane_id();
        let deadline = Instant::now() + INPUT_TIMEOUT;
        let mut executed = false;
        while Instant::now() < deadline {
            app.test_poll_once();
            pump_main_loop(30);
            app.test_flush_feeds();
            if app.test_pane_vte_text(pane).contains(output_token)
                && !app.test_search_all(output_token).is_empty()
            {
                executed = true;
                break;
            }
        }
        assert!(
            executed,
            "SSH Herdr VTE 逐字输入 + Enter 必须执行远端命令。vte={:?} search={:?}",
            app.test_pane_vte_text(pane),
            app.test_search_all(output_token)
        );
    });
}
