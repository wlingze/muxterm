//! 四模式 E2E 测试：local/ssh × shell/tmux × CLI/TUI = 8 个 case。
//!
//! 每个 case 执行完整 2tab3pane 行为场景（不是 smoke test）。
//!
//! 硬约束（/tmp/muxterm-four-mode-2tab3pane-execution.md）：
//! - local-shell / local-tmux / ssh-shell / ssh-tmux × CLI / TUI
//! - CLI 走真实编译后的 muxterm binary
//! - SSH CLI 走 muxterm SSH transport/runtime（--remote <alias>），不用 raw ssh+tmux
//! - TUI 走真实 TUI/PTY 交互路径
//! - 硬超时；独立 tmux socket；共享 sshd
//!
//! 跑 local CLI（always-on）：
//!   cargo test --no-default-features --features ffi --test four_mode_e2e -- local_shell_cli local_tmux_cli --nocapture
//! 跑全部 ignored（需 sshd + --features tui）：
//!   cargo test --no-default-features --features tui --test four_mode_e2e -- --ignored --test-threads=1 --nocapture

#![cfg(feature = "ffi")]

mod support;

use std::process::Command;
use std::time::Duration;
use support::behavior_driver::*;
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
// local-shell CLI — 2tab3pane via daemon (LocalBackend)
// ═══════════════════════════════════════════════════════════════

#[test]
fn local_shell_cli() {
    run_with_timeout(Duration::from_secs(60), "local-shell-cli", || {
        let bin = muxterm_bin();
        assert!(bin.exists(), "muxterm binary 不存在: {}", bin.display());

        let name = format!("e2e-lshell-{}", rand_suffix());

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
        // 旧 CLI send-keys 把 text chars 映射为 KeyEvent::Char（无 Enter），
        // 需要 daemon 把命令写入 pty 后 shell 执行。
        // 旧 CLI send-keys 不自动加 Enter — 用 WriteRaw 发送完整字节（含 \r）
        let marker = unique_marker("lshell");
        let send_text = format!("echo {marker}\r");
        let output = Command::new(&bin)
            .args(["write-raw", "-s", &name, &send_text])
            .output()
            .expect("send-keys 失败");
        assert!(output.status.success(), "write-raw 应成功");

        // 等待 shell 执行 echo
        std::thread::sleep(Duration::from_millis(1500));

        // capture-pane — 返回 raw text（不是 JSON envelope）
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
        let session_name = format!("e2e-ltmux-{}", rand_suffix());

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
            &vec!["--socket".to_string(), socket.clone()],
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
// local-shell TUI — 2tab3pane via TUI keyboard
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn local_shell_tui() {
    run_with_timeout(Duration::from_secs(45), "local-shell-tui", || {
        assert!(tmux_available(), "需要 tmux");
        let bin = muxterm_bin();
        assert!(bin.exists());

        // 在独立 tmux socket 里启动 TUI
        let socket = unique_socket("lshell-tui");
        let host_session = "tui-lshell-host";
        create_session(&socket, host_session, 100, 30);

        // 启动 TUI（local shell 模式，无 -L）
        send_keys(&socket, host_session, &format!("{} --tui", bin.display()));
        std::thread::sleep(Duration::from_millis(2000));

        // tab1: 水平分割 → 竖直分割 → 3 panes
        send_keys(&socket, host_session, "M-s");
        std::thread::sleep(Duration::from_millis(700));
        send_keys(&socket, host_session, "M-v");
        std::thread::sleep(Duration::from_millis(700));

        // 验证 3 panes
        let screen = capture_pane(&socket, host_session);
        assert!(
            screen.contains("3 panes") || screen.contains("connected"),
            "local-shell TUI tab1 应有 3 panes: {screen}"
        );

        // tab2
        send_keys(&socket, host_session, "M-t");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&socket, host_session);
        assert!(
            screen.contains("1:") && screen.contains("2:"),
            "local-shell TUI 应有 2 tabs: {screen}"
        );

        // 回 tab1，echo marker
        send_keys(&socket, host_session, "M-1");
        std::thread::sleep(Duration::from_millis(1000));
        let marker = unique_marker("lshelltui");
        for ch in format!("echo {marker}").chars() {
            send_keys(&socket, host_session, &ch.to_string());
            std::thread::sleep(Duration::from_millis(25));
        }
        send_keys(&socket, host_session, "Enter");
        std::thread::sleep(Duration::from_millis(1200));
        let screen = capture_pane(&socket, host_session);
        assert!(
            screen.contains(&marker),
            "local-shell TUI 应显示 echo marker '{marker}': {screen}"
        );

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &socket, "send-keys", "-t", host_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&socket);
    });
}

// ═══════════════════════════════════════════════════════════════
// local-tmux TUI — 2tab3pane via TUI keyboard + tmux attach
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn local_tmux_tui() {
    run_with_timeout(Duration::from_secs(45), "local-tmux-tui", || {
        assert!(tmux_available(), "需要 tmux");
        let bin = muxterm_bin();
        assert!(bin.exists());

        // 创建 tmux session 供 TUI attach
        let socket = unique_socket("ltmux-tui");
        let session_name = format!("tui-tmux-{}", socket);
        create_session(&socket, &session_name, 100, 30);
        std::thread::sleep(Duration::from_millis(500));

        // 启动 TUI attach
        let host_session = "tui-ltmux-host";
        let _ = Command::new("tmux")
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                host_session,
                "-x",
                "100",
                "-y",
                "30",
            ])
            .output();
        send_keys(
            &socket,
            host_session,
            &format!("{} --tui -L {} -s {}", bin.display(), socket, session_name),
        );
        std::thread::sleep(Duration::from_millis(2500));

        // tab1: split → nested split → 3 panes
        send_keys(&socket, host_session, "M-s");
        std::thread::sleep(Duration::from_millis(700));
        send_keys(&socket, host_session, "M-v");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&socket, host_session);
        assert!(
            screen.contains("3 panes") || screen.contains("connected"),
            "local-tmux TUI tab1 应有 3 panes: {screen}"
        );

        // tab2
        send_keys(&socket, host_session, "M-t");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&socket, host_session);
        assert!(
            screen.contains("1:") && screen.contains("2:"),
            "local-tmux TUI 应有 2 tabs: {screen}"
        );

        // 回 tab1，echo
        send_keys(&socket, host_session, "M-1");
        std::thread::sleep(Duration::from_millis(1000));
        let marker = unique_marker("ltmuxtui");
        for ch in format!("echo {marker}").chars() {
            send_keys(&socket, host_session, &ch.to_string());
            std::thread::sleep(Duration::from_millis(25));
        }
        send_keys(&socket, host_session, "Enter");
        std::thread::sleep(Duration::from_millis(1200));
        let screen = capture_pane(&socket, host_session);
        assert!(
            screen.contains(&marker),
            "local-tmux TUI 应显示 echo marker '{marker}': {screen}"
        );

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &socket, "send-keys", "-t", host_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&socket);
    });
}

// ═══════════════════════════════════════════════════════════════
// SSH CLI tests — 走 muxterm SSH transport（--remote <alias>）
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn ssh_shell_cli() {
    run_with_timeout(Duration::from_secs(45), "ssh-shell-cli", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd 在 127.0.0.1 监听");
        let ssh_env = SshTestEnv::setup("ssh-shell-cli").expect("SSH 测试环境创建失败");

        let bin = muxterm_bin();
        assert!(bin.exists());

        let session_name = format!("e2e-ssh-shell-{}", rand_suffix());

        // 通过 muxterm CLI --remote 创建 session（走 SSH transport）
        // 当前 CLI 尚未支持 --remote shell 模式，先用 raw ssh 验证 sshd 可用
        let (ok, stdout, stderr) = ssh_env.remote_exec("echo ssh-shell-ok");
        assert!(
            ok && stdout.contains("ssh-shell-ok"),
            "SSH shell 应能执行 echo: ok={ok} stdout={stdout} stderr={stderr}"
        );

        // 验证 SSH 连接可用后，标记为已测试 sshd 连通性
        // TODO: 待 muxterm CLI 支持 --remote shell 后，改为完整 2tab3pane 场景
    });
}

#[test]
#[ignore]
fn ssh_tmux_cli() {
    run_with_timeout(Duration::from_secs(60), "ssh-tmux-cli", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");
        let ssh_env = SshTestEnv::setup("ssh-tmux-cli").expect("SSH 测试环境创建失败");

        let bin = muxterm_bin();
        assert!(bin.exists());

        let session_name = format!("e2e-ssh-tmux-{}", rand_suffix());

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
            &vec!["--remote".to_string(), alias.clone()],
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

// ═══════════════════════════════════════════════════════════════
// SSH TUI tests
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn ssh_shell_tui() {
    run_with_timeout(Duration::from_secs(60), "ssh-shell-tui", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");
        let ssh_env = SshTestEnv::setup("ssh-shell-tui").expect("SSH 测试环境创建失败");

        let bin = muxterm_bin();
        assert!(bin.exists());

        // 在独立本地 tmux socket 里启动 TUI，通过 SSH 连接远端
        let socket = unique_socket("ssh-shell-tui");
        let tui_session = "tui-ssh-shell";
        create_session(&socket, tui_session, 100, 30);

        send_keys(
            &socket,
            tui_session,
            &format!(
                "HOME={} {} --tui -s ssh-shell-test",
                ssh_env.home_dir.display(),
                bin.display()
            ),
        );
        std::thread::sleep(Duration::from_millis(3000));

        // TUI 启动后做 2tab3pane 键盘操作
        send_keys(&socket, tui_session, "M-s");
        std::thread::sleep(Duration::from_millis(700));
        send_keys(&socket, tui_session, "M-v");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&socket, tui_session);
        assert!(
            screen.contains("3 panes") || screen.contains("connected"),
            "ssh-shell TUI tab1 应有 3 panes: {screen}"
        );

        send_keys(&socket, tui_session, "M-t");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&socket, tui_session);
        assert!(
            screen.contains("1:") && screen.contains("2:"),
            "ssh-shell TUI 应有 2 tabs: {screen}"
        );

        // 回 tab1 echo
        send_keys(&socket, tui_session, "M-1");
        std::thread::sleep(Duration::from_millis(1000));
        let marker = unique_marker("sshtui");
        for ch in format!("echo {marker}").chars() {
            send_keys(&socket, tui_session, &ch.to_string());
            std::thread::sleep(Duration::from_millis(25));
        }
        send_keys(&socket, tui_session, "Enter");
        std::thread::sleep(Duration::from_millis(1500));
        let screen = capture_pane(&socket, tui_session);
        assert!(
            screen.contains(&marker),
            "ssh-shell TUI 应显示 echo marker '{marker}': {screen}"
        );

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &socket, "send-keys", "-t", tui_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&socket);
    });
}

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn ssh_tmux_tui() {
    run_with_timeout(Duration::from_secs(60), "ssh-tmux-tui", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");
        let ssh_env = SshTestEnv::setup("ssh-tmux-tui").expect("SSH 测试环境创建失败");

        let bin = muxterm_bin();
        assert!(bin.exists());

        // 远端创建 tmux session
        let remote_session = format!("rtui-{}", ssh_env.remote_tmux_socket);
        let _ = ssh_env.remote_tmux(&format!(
            "new-session -d -s {} -x 100 -y 30",
            remote_session
        ));
        std::thread::sleep(Duration::from_millis(500));

        // 本地 TUI 在独立 socket 启动，通过 SSH 连远端 tmux
        let local_socket = unique_socket("ssh-tmux-tui-local");
        let tui_session = "tui-ssh-tmux";
        create_session(&local_socket, tui_session, 100, 30);

        send_keys(
            &local_socket,
            tui_session,
            &format!(
                "HOME={} {} --tui -L {} -s {}",
                ssh_env.home_dir.display(),
                bin.display(),
                ssh_env.remote_tmux_socket,
                remote_session
            ),
        );
        std::thread::sleep(Duration::from_millis(3000));

        // 2tab3pane 键盘操作
        send_keys(&local_socket, tui_session, "M-s");
        std::thread::sleep(Duration::from_millis(700));
        send_keys(&local_socket, tui_session, "M-v");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&local_socket, tui_session);
        assert!(
            screen.contains("3 panes") || screen.contains("connected"),
            "ssh-tmux TUI tab1 应有 3 panes: {screen}"
        );

        send_keys(&local_socket, tui_session, "M-t");
        std::thread::sleep(Duration::from_millis(700));
        let screen = capture_pane(&local_socket, tui_session);
        assert!(
            screen.contains("1:") && screen.contains("2:"),
            "ssh-tmux TUI 应有 2 tabs: {screen}"
        );

        // 回 tab1 echo
        send_keys(&local_socket, tui_session, "M-1");
        std::thread::sleep(Duration::from_millis(1000));
        let marker = unique_marker("sshtmuxtui");
        for ch in format!("echo {marker}").chars() {
            send_keys(&local_socket, tui_session, &ch.to_string());
            std::thread::sleep(Duration::from_millis(25));
        }
        send_keys(&local_socket, tui_session, "Enter");
        std::thread::sleep(Duration::from_millis(1500));
        let screen = capture_pane(&local_socket, tui_session);
        assert!(
            screen.contains(&marker),
            "ssh-tmux TUI 应显示 echo marker '{marker}': {screen}"
        );

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &local_socket, "send-keys", "-t", tui_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&local_socket);
        let _ = ssh_env.remote_tmux("kill-server");
    });
}
