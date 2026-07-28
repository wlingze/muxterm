//! 四模式集成测试：local/ssh × shell/tmux × CLI = 4 个 case。
//!
//! 替代旧 four_mode_e2e.rs。使用硬超时 + 独立 tmux socket + 共享 sshd。
//! SSH 走 muxterm SSH transport（--remote <alias>），不用 raw ssh+tmux。
//!
//! 跑 local CLI（always-on）：
//!   cargo test --no-default-features --features ffi --test four_mode_integration -- local -- --test-threads=1
//! 跑 SSH（需 sshd + --ignored）：
//!   cargo test --no-default-features --features ffi --test four_mode_integration -- --ignored --test-threads=1

#![cfg(feature = "ffi")]

mod support;

use std::process::Command;
use std::time::Duration;
use support::behavior_driver::*;
use support::sshd_test_support::*;
use support::tmux_test_support::*;

/// 找到 muxterm binary 路径。
fn muxterm_bin() -> std::path::PathBuf {
    let target =
        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "../muxterm-target".to_string());
    let p = std::path::PathBuf::from(&target)
        .join("debug")
        .join("muxterm");
    if p.exists() {
        return p;
    }
    std::path::PathBuf::from("target/debug/muxterm")
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

// ═══════════════════════════════════════════════════════════════
// local-shell CLI — split/new-tab/send-keys/capture via daemon
// ═══════════════════════════════════════════════════════════════

#[test]
fn local_shell_cli() {
    run_with_timeout(Duration::from_secs(60), "local-shell-cli", || {
        let bin = muxterm_bin();
        assert!(bin.exists(), "muxterm binary 不存在: {}", bin.display());

        let name = format!("it-lshell-{}", rand_suffix());

        // 创建 daemon session（local shell）
        let output = Command::new(&bin)
            .args(["new-session", "-s", &name])
            .output()
            .expect("new-session 失败");
        assert!(output.status.success(), "new-session 应成功");

        // split-pane (horizontal) → 2 panes
        let output = Command::new(&bin)
            .args(["split-pane", "-h", "-s", &name])
            .output()
            .expect("split-pane 失败");
        assert!(output.status.success(), "split-pane 应成功");

        // nested split (vertical) → 3 panes
        let output = Command::new(&bin)
            .args(["split-pane", "-v", "-s", &name])
            .output()
            .expect("nested split 失败");
        assert!(output.status.success(), "nested split 应成功");

        // new-tab → 2 tabs
        let output = Command::new(&bin)
            .args(["new-tab", "-s", &name])
            .output()
            .expect("new-tab 失败");
        assert!(output.status.success(), "new-tab 应成功");

        // list-tabs → 应有 2 tabs
        let output = Command::new(&bin)
            .args(["list-tabs", "-s", &name, "--format", "json"])
            .output()
            .expect("list-tabs 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("t1") || stdout.contains("t2") || stdout.contains("tab"),
            "list-tabs 应返回 tab 信息: {stdout}"
        );

        // list-panes → 应有 panes
        let output = Command::new(&bin)
            .args(["list-panes", "-s", &name, "--format", "json"])
            .output()
            .expect("list-panes 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("@") || stdout.contains("pane"),
            "list-panes 应返回 pane 信息: {stdout}"
        );

        // send-keys + capture：向 active pane 发 echo marker
        let marker = unique_marker("lshell");
        let send_text = format!("echo {marker}\r");
        let output = Command::new(&bin)
            .args(["write-raw", "-s", &name, &send_text])
            .output()
            .expect("send-keys 失败");
        assert!(output.status.success(), "write-raw 应成功");

        // 等待 shell 执行 echo
        std::thread::sleep(Duration::from_millis(1500));

        // capture-pane — 返回 raw text
        let output = Command::new(&bin)
            .args(["capture-pane", "-s", &name])
            .output()
            .expect("capture-pane 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&marker),
            "capture-pane 应包含 echo marker '{marker}': {stdout}"
        );

        // 清理
        let _ = Command::new(&bin)
            .args(["kill-session", "-s", &name])
            .output();
    });
}

// ═══════════════════════════════════════════════════════════════
// local-tmux CLI — 2tab3pane via muxterm tmux CLI commands
// ═══════════════════════════════════════════════════════════════

#[test]
fn local_tmux_cli() {
    run_with_timeout(Duration::from_secs(120), "local-tmux-cli", || {
        let bin = muxterm_bin();
        assert!(bin.exists());

        let socket = unique_socket("ltmux-cli");
        let session_name = format!("it-ltmux-{}", rand_suffix());

        // 用 tmux 在独立 socket 上创建 detached session
        Command::new("tmux")
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                &session_name,
                "-x",
                "80",
                "-y",
                "24",
            ])
            .output()
            .expect("创建 tmux session 失败");
        std::thread::sleep(Duration::from_millis(500));

        // 运行 2tab3pane 场景（传 --socket <socket>）
        let failures = cli_2tab3pane_scenario(
            &bin,
            &session_name,
            &["--socket".to_string(), socket.clone()],
            Duration::from_secs(20),
        );

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &socket, "kill-session", "-t", &session_name])
            .output();
        kill_server(&socket);

        assert!(
            failures.is_empty(),
            "local-tmux CLI 2tab3pane 有失败:\n{}",
            failures.join("\n")
        );
    });
}

// ═══════════════════════════════════════════════════════════════
// SSH shell CLI — 通过 muxterm 产品路径（--target <alias>）走 SSH transport
// ═══════════════════════════════════════════════════════════════
//
// 测试 muxterm CLI 的 SSH session list 路径：
//   muxterm tmux session list --target <alias>
// 这会通过 SshProcessTransport 在远端执行 tmux 命令，
// 而不是直接用 raw ssh + tmux。
//
// 前置条件：共享 loopback sshd（由 CI 或本地环境提供）。

#[test]
#[ignore]
fn ssh_shell_cli() {
    run_with_timeout(Duration::from_secs(45), "ssh-shell-cli", || {
        assert!(sshd_available(), "需要 sshd 在 127.0.0.1 监听");
        let ssh_env = SshTestEnv::setup("ssh-shell-cli").expect("SSH 测试环境创建失败");

        let bin = muxterm_bin();
        assert!(bin.exists());

        // 先用 raw ssh 验证 sshd 可用（测试基础设施，不是产品路径）
        let (ok, stdout, stderr) = ssh_env.remote_exec("echo sshd-ok");
        assert!(
            ok && stdout.contains("sshd-ok"),
            "sshd 连通性验证失败: ok={ok} stdout={stdout} stderr={stderr}"
        );

        // 产品路径：muxterm tmux session list --target <alias>
        // 这应该通过 SSH transport 在远端列出 tmux sessions
        let alias = ssh_env.alias.clone();
        let output = Command::new(&bin)
            .args(["tmux", "session", "list", "--target", &alias])
            .env("HOME", &ssh_env.home_dir)
            .env("MUXTERM_TEST_SSH_HOST", &ssh_env.host)
            .env("MUXTERM_TEST_SSH_PORT", ssh_env.port.to_string())
            .env("MUXTERM_TEST_SSH_USER", &ssh_env.user)
            .env("MUXTERM_TEST_SSH_KEY", &ssh_env.client_key_path)
            .env("MUXTERM_SSH_CONFIG_PATH", &ssh_env.ssh_config_path)
            .output()
            .expect("muxterm tmux session list 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // 必须返回 JSON envelope（ok 或 error），不能是空输出或 panic
        assert!(
            stdout.contains("ok") || stderr.contains("SSH") || stderr.contains("remote"),
            "muxterm SSH session list 应返回 JSON envelope: stdout={stdout} stderr={stderr}"
        );

        // SSH session list 应返回 ok:true（即使远端没有 tmux session，也应返回空列表）
        assert!(
            stdout.contains(r#""ok":true"#),
            "SSH session list 应成功: stdout={stdout} stderr={stderr}"
        );
    });
}

// ═══════════════════════════════════════════════════════════════
// SSH tmux CLI — 走 muxterm SSH transport（--remote <alias>）
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn ssh_tmux_cli() {
    run_with_timeout(Duration::from_secs(60), "ssh-tmux-cli", || {
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");
        let ssh_env = SshTestEnv::setup("ssh-tmux-cli").expect("SSH 测试环境创建失败");

        let bin = muxterm_bin();
        assert!(bin.exists());

        let session_name = format!("it-ssh-tmux-{}", rand_suffix());

        // 在远端创建 tmux session
        let (ok, _, stderr) =
            ssh_env.remote_tmux(&format!("new-session -d -s {} -x 80 -y 24", session_name));
        assert!(ok, "远端 tmux session 创建失败: {stderr}");
        std::thread::sleep(Duration::from_millis(500));

        // 运行 2tab3pane 场景（通过 --remote alias 让 muxterm 走 SSH transport）
        let alias = ssh_env.alias.clone();
        let failures = cli_2tab3pane_scenario(
            &bin,
            &session_name,
            &["--remote".to_string(), alias.clone()],
            Duration::from_secs(30),
        );

        // 清理远端 tmux
        let _ = ssh_env.remote_tmux("kill-server");

        assert!(
            failures.is_empty(),
            "ssh-tmux CLI 2tab3pane 有失败:\n{}",
            failures.join("\n")
        );
    });
}
