//! attach 修复 session 环境，只影响后续 pane，不修改其他 session 或注入输入。
mod support;
use muxterm::test_support::core::{
    runtime::tmux::client::{ConnectMode, TmuxClient, TmuxClientConfig},
    transport::connection::Connect,
};
use std::{
    process::Command,
    time::{Duration, Instant},
};
use support::sshd_test_support::LoopbackSshd;
use support::tmux_test_support::TmuxServerGuard;

#[test]
fn attach_repairs_only_target_session_environment() -> anyhow::Result<()> {
    let guard = TmuxServerGuard::new("attach-locale");
    let run = |args: &[&str]| -> anyhow::Result<String> {
        let out = Command::new("tmux")
            .args(["-L", guard.socket()])
            .args(args)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "tmux: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(String::from_utf8(out.stdout)?)
    };
    for name in ["attached", "untouched"] {
        run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            name,
            "-e",
            "LC_ALL=C",
            "-e",
            "LANG=C",
            "-e",
            "LC_CTYPE=C",
        ])?;
    }
    run(&["set-environment", "-g", "LC_ALL", "C"])?;
    run(&[
        "set-option",
        "-g",
        "default-command",
        "locale charmap; sleep 30",
    ])?;
    let sshd = LoopbackSshd::start("tmux-locale")?;
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("MUXTERM_SSH_CONFIG_PATH", value);
            } else {
                std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
            }
        }
    }
    let _restore = Restore(std::env::var_os("MUXTERM_SSH_CONFIG_PATH"));
    sshd.apply_ssh_config_env();
    let rt = tokio::runtime::Runtime::new()?;
    for (transport, target) in [("local", ""), ("ssh", sshd.alias.as_str())] {
        run(&["set-environment", "-t", "attached", "LC_ALL", "C"])?;
        let (mut client, _events) = rt.block_on(TmuxClient::spawn_channel(
            Connect::new(transport, target),
            TmuxClientConfig {
                mode: Some(ConnectMode::Attach {
                    target: Some("attached".into()),
                }),
                tmux_bin: Some("tmux".into()),
                extra_args: vec!["-L".into(), guard.socket().into()],
                ..Default::default()
            },
        ))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let value = loop {
            let value = run(&["show-environment", "-t", "attached", "LC_ALL"])?;
            if value.trim() != "LC_ALL=C" {
                break value;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "{transport}: attach left LC_ALL=C"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            value.to_ascii_lowercase().replace('-', "").contains("utf8"),
            "{value}"
        );
        assert_eq!(
            run(&["show-environment", "-t", "untouched", "LC_ALL"])?.trim(),
            "LC_ALL=C"
        );
        // 新 pane 必须获得真实可用的 UTF-8，而不仅是 session 配置里出现了一个字符串。
        let pane = run(&[
            "new-window",
            "-d",
            "-t",
            "attached",
            "-P",
            "-F",
            "#{pane_id}",
            "locale charmap; sleep 10",
        ])?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let screen = run(&["capture-pane", "-p", "-t", pane.trim()])?;
            if screen.contains("UTF-8") {
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "new pane locale invalid: {screen:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        rt.block_on(client.kill())?;
        // 已有 ASCII server 上新 session 的第一个 pane 也必须正确。
        let fresh_name = format!("fresh-{transport}");
        let (mut fresh, _events) = rt.block_on(TmuxClient::spawn_channel(
            Connect::new(transport, target),
            TmuxClientConfig {
                mode: Some(ConnectMode::NewSession {
                    name: Some(fresh_name.clone()),
                    start_directory: None,
                }),
                tmux_bin: Some("tmux".into()),
                extra_args: vec!["-L".into(), guard.socket().into()],
                ..Default::default()
            },
        ))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let screen = run(&["capture-pane", "-p", "-t", &fresh_name]).unwrap_or_default();
            if screen.contains("UTF-8") {
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "first pane locale invalid: {screen:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            run(&["show-environment", "-g", "LC_ALL"])?.trim(),
            "LC_ALL=C"
        );
        rt.block_on(fresh.kill())?;
    }
    Ok(())
}
