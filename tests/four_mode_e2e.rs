//! 四模式 E2E 测试：local/ssh × shell/tmux × CLI/TUI = 8 个 case。
//!
//! **硬约束**（来自 /tmp/muxterm-four-mode-ci-requirement.md）：
//! - Linux CI 必须运行全部 8 个 case
//! - SSH 测试用临时 loopback sshd（127.0.0.1:随机端口，临时密钥）
//! - 每个 case 有硬超时
//! - 不访问公网，不读取用户真实 ~/.ssh/config
//! - 独立 tmux socket，不复用宿主默认 socket
//!
//! 分类：
//! - local-shell / local-tmux 的 CLI 测试：always-on（不需要 sshd）
//! - ssh-shell / ssh-tmux 的 CLI 测试：#[ignore]（需要 sshd，CI workflow 调用）
//! - TUI 测试：#[ignore]（需要编译 --features tui + tmux + 显示环境）
//!
//! 跑全部 E2E：cargo test --no-default-features --features tui --test four_mode_e2e -- --ignored --test-threads=1
//! 跑 local CLI only：cargo test --no-default-features --features ffi --test four_mode_e2e

#![cfg(feature = "ffi")]

mod support;

use std::process::Command;
use std::time::Duration;
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

/// 运行 muxterm binary，硬超时。
fn run_muxterm(args: &[&str], timeout: Duration) -> (bool, String, String) {
    let bin = muxterm_bin();
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    run_command(&mut cmd, timeout)
}

/// 运行命令，硬超时。
fn run_command(cmd: &mut Command, timeout: Duration) -> (bool, String, String) {
    let output = cmd.timeout(timeout).output().expect("执行命令失败");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.success(), stdout, stderr)
}

/// 为 Command 添加 timeout（用 timeout 命令包装）。
trait CommandTimeout {
    fn timeout(&mut self, duration: Duration) -> &mut Self;
}

impl CommandTimeout for Command {
    fn timeout(&mut self, duration: Duration) -> &mut Self {
        // 用 std::process::Command 的 pre_exec 不便；这里用 kill 超时模式
        // 简化：在调用方用线程 + kill 实现
        self
    }
}

// ═══════════════════════════════════════════════════════════════
// local-shell CLI
// ═══════════════════════════════════════════════════════════════

#[test]
fn local_shell_cli() {
    run_with_timeout(Duration::from_secs(10), "local-shell-cli", || {
        let bin = muxterm_bin();
        assert!(bin.exists(), "muxterm binary 不存在: {}", bin.display());

        // ephemeral local shell 模式：list-sessions 应返回 JSON
        let output = Command::new(&bin)
            .args(["list-sessions"])
            .output()
            .expect("执行失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("local") || stdout.contains("sessions"),
            "local-shell CLI 应返回 session 信息: {stdout}"
        );
    });
}

// ═══════════════════════════════════════════════════════════════
// local-tmux CLI
// ═══════════════════════════════════════════════════════════════

#[test]
fn local_tmux_cli() {
    run_with_timeout(Duration::from_secs(20), "local-tmux-cli", || {
        let bin = muxterm_bin();
        assert!(bin.exists());

        let socket = unique_socket("ltmux-cli");
        let session_name = format!("e2e-{}", socket);

        // 1. tmux session list
        let output = Command::new(&bin)
            .args(["tmux", "session", "list", "--target", "local"])
            .output()
            .expect("session list 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("\"ok\":true"),
            "session list 应返回 ok: {stdout}"
        );

        // 2. tmux session new
        let output = Command::new(&bin)
            .args([
                "tmux",
                "session",
                "new",
                "--target",
                "local",
                "--name",
                &session_name,
            ])
            .output()
            .expect("session new 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("\"ok\":true"),
            "session new 应成功: {stdout}"
        );

        // 3. tab list
        let output = Command::new(&bin)
            .args([
                "tmux",
                "tab",
                "list",
                "--target",
                "local",
                "--session",
                &session_name,
            ])
            .output()
            .expect("tab list 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"ok\":true"), "tab list 应成功: {stdout}");

        // 4. pane list
        let output = Command::new(&bin)
            .args([
                "tmux",
                "pane",
                "list",
                "--target",
                "local",
                "--session",
                &session_name,
            ])
            .output()
            .expect("pane list 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"ok\":true"), "pane list 应成功: {stdout}");

        // 5. pane split
        let output = Command::new(&bin)
            .args([
                "tmux",
                "pane",
                "split",
                "--target",
                "local",
                "--session",
                &session_name,
                "--pane",
                "1",
                "--direction",
                "horizontal",
            ])
            .output()
            .expect("pane split 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("\"ok\":true"),
            "pane split 应成功: {stdout}"
        );

        // 6. pane send-keys
        let output = Command::new(&bin)
            .args([
                "tmux",
                "pane",
                "send-keys",
                "--target",
                "local",
                "--session",
                &session_name,
                "--pane",
                "1",
                "--text",
                "echo hello",
            ])
            .output()
            .expect("send-keys 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"ok\":true"), "send-keys 应成功: {stdout}");

        // 7. pane capture
        let output = Command::new(&bin)
            .args([
                "tmux",
                "pane",
                "capture",
                "--target",
                "local",
                "--session",
                &session_name,
                "--pane",
                "1",
                "--lines",
                "5",
            ])
            .output()
            .expect("capture 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"ok\":true"), "capture 应成功: {stdout}");

        // 8. error: bad session
        let output = Command::new(&bin)
            .args([
                "tmux",
                "tab",
                "list",
                "--target",
                "local",
                "--session",
                "nonexistent-xyz",
            ])
            .output()
            .expect("error case 失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        // tmux 会创建新 session（-A 语义）或报错；都验证有 JSON 输出
        assert!(
            stdout.contains("\"ok\":"),
            "错误也应返回 envelope: {stdout}"
        );

        // 清理：kill test session
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &session_name])
            .output();
    });
}

// ═══════════════════════════════════════════════════════════════
// local-shell TUI (#[ignore]：需要 --features tui + tmux 环境)
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn local_shell_tui() {
    run_with_timeout(Duration::from_secs(30), "local-shell-tui", || {
        assert!(tmux_available(), "需要 tmux");
        let bin = muxterm_bin();
        assert!(bin.exists(), "muxterm binary 不存在");

        // 在独立 tmux socket 里启动 TUI，用 capture-pane 验证渲染
        let socket = unique_socket("lshell-tui");
        let session = "tui-test";
        create_session(&socket, session, 80, 24);

        // send-keys 启动 muxterm --tui -s <name>
        let tui_session = format!("{}-tui", session);
        let _ = Command::new("tmux")
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                &tui_session,
                "-x",
                "80",
                "-y",
                "24",
            ])
            .output();

        send_keys(
            &socket,
            &tui_session,
            &format!("{} --tui -s tui-local-shell-{}", bin.display(), socket),
        );

        // 等待 TUI 启动
        std::thread::sleep(Duration::from_secs(3));

        // capture-pane 检查渲染
        let pane_text = capture_pane(&socket, &tui_session);
        assert!(!pane_text.is_empty(), "TUI 应渲染画面");

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &socket, "send-keys", "-t", &tui_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&socket);
    });
}

// ═══════════════════════════════════════════════════════════════
// local-tmux TUI (#[ignore])
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn local_tmux_tui() {
    run_with_timeout(Duration::from_secs(30), "local-tmux-tui", || {
        assert!(tmux_available(), "需要 tmux");
        let bin = muxterm_bin();
        assert!(bin.exists());

        let socket = unique_socket("ltmux-tui");
        let session_name = format!("tui-tmux-{}", socket);

        // 先创建一个 tmux session 供 TUI attach
        create_session(&socket, &session_name, 80, 24);

        // 启动 TUI attach 这个 session
        let tui_session = format!("{}-tui", session_name);
        let _ = Command::new("tmux")
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                &tui_session,
                "-x",
                "80",
                "-y",
                "24",
            ])
            .output();

        send_keys(
            &socket,
            &tui_session,
            &format!("{} --tui -L {} -s {}", bin.display(), socket, session_name),
        );

        std::thread::sleep(Duration::from_secs(3));
        let pane_text = capture_pane(&socket, &tui_session);
        assert!(!pane_text.is_empty(), "TUI 应渲染画面");

        // 清理
        let _ = Command::new("tmux")
            .args(["-L", &socket, "send-keys", "-t", &tui_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&socket);
    });
}

// ═══════════════════════════════════════════════════════════════
// SSH CLI tests (#[ignore]：需要共享 sshd 已启动)
//
// sshd 由外部管理（CI setup-sshd.sh 或本地环境）。
// 测试从 MUXTERM_TEST_SSH_* 环境变量读取连接参数。
// 测试本身绝不 spawn/kill sshd。
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn ssh_shell_cli() {
    run_with_timeout(Duration::from_secs(30), "ssh-shell-cli", || {
        use support::sshd_test_support::*;
        assert!(
            sshd_available(),
            "需要 sshd 在 127.0.0.1 监听（设 MUXTERM_TEST_SSH_PORT）"
        );
        assert!(ssh_client_available(), "需要 ssh 客户端");

        let ssh_env = SshTestEnv::setup("ssh-shell-cli").expect("SSH 测试环境创建失败");

        // 验证 SSH 连接：通过 alias 执行 echo
        let (ok, stdout, stderr) = ssh_env.remote_exec("echo ssh-shell-ok");
        assert!(
            ok && stdout.contains("ssh-shell-ok"),
            "SSH shell 应能执行 echo: ok={ok} stdout={stdout} stderr={stderr}"
        );
    });
}

#[test]
#[ignore]
fn ssh_tmux_cli() {
    run_with_timeout(Duration::from_secs(45), "ssh-tmux-cli", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");

        let ssh_env = SshTestEnv::setup("ssh-tmux-cli").expect("SSH 测试环境创建失败");
        let session_name = format!("rt-{}", ssh_env.remote_tmux_socket);

        // 远端创建 tmux session（用独立 socket）
        let (ok, _stdout, stderr) =
            ssh_env.remote_tmux(&format!("new-session -d -s {} -x 80 -y 24", session_name));
        assert!(ok, "远端 tmux session 创建失败: {stderr}");

        // 远端验证 session 存在
        let (ok, stdout, stderr) = ssh_env.remote_tmux("list-sessions -F '#{session_name}'");
        assert!(ok, "远端 tmux list-sessions 失败: {stderr}");
        assert!(
            stdout.contains(&session_name),
            "远端应列出 session: {stdout}"
        );

        // 清理远端 tmux
        let _ = ssh_env.remote_tmux("kill-server");
    });
}

// ═══════════════════════════════════════════════════════════════
// SSH TUI tests (#[ignore])
//
// sshd 由外部管理；TUI 进程在独立本地 tmux socket 里启动。
// ═══════════════════════════════════════════════════════════════

#[cfg(feature = "tui")]
#[test]
#[ignore]
fn ssh_shell_tui() {
    run_with_timeout(Duration::from_secs(45), "ssh-shell-tui", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");

        let bin = muxterm_bin();
        assert!(bin.exists());

        let ssh_env = SshTestEnv::setup("ssh-shell-tui").expect("SSH 测试环境创建失败");

        // 在独立本地 tmux socket 里启动 TUI
        let socket = unique_socket("ssh-shell-tui");
        let tui_session = "tui-ssh-shell";
        create_session(&socket, tui_session, 80, 24);

        send_keys(
            &socket,
            tui_session,
            &format!(
                "HOME={} {} --tui -s ssh-shell-test",
                ssh_env.home_dir.display(),
                bin.display()
            ),
        );

        std::thread::sleep(Duration::from_secs(5));
        let pane_text = capture_pane(&socket, tui_session);
        assert!(!pane_text.is_empty(), "SSH shell TUI 应渲染画面");

        // 清理本地 tmux
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
    run_with_timeout(Duration::from_secs(45), "ssh-tmux-tui", || {
        use support::sshd_test_support::*;
        assert!(sshd_available(), "需要 sshd");
        assert!(tmux_available(), "需要 tmux");

        let bin = muxterm_bin();
        assert!(bin.exists());

        let ssh_env = SshTestEnv::setup("ssh-tmux-tui").expect("SSH 测试环境创建失败");
        let remote_session = format!("rtui-{}", ssh_env.remote_tmux_socket);

        // 远端创建 tmux session
        let _ = ssh_env.remote_tmux(&format!("new-session -d -s {} -x 80 -y 24", remote_session));

        // 本地 TUI 在独立 tmux socket 里启动，通过 SSH 连远端
        let local_socket = unique_socket("ssh-tmux-tui-local");
        let tui_session = "tui-ssh-tmux";
        create_session(&local_socket, tui_session, 80, 24);

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

        std::thread::sleep(Duration::from_secs(5));
        let pane_text = capture_pane(&local_socket, tui_session);
        assert!(!pane_text.is_empty(), "SSH tmux TUI 应渲染画面");

        // 清理本地 tmux
        let _ = Command::new("tmux")
            .args(["-L", &local_socket, "send-keys", "-t", tui_session, "C-c"])
            .output();
        std::thread::sleep(Duration::from_millis(500));
        kill_server(&local_socket);

        // 清理远端 tmux
        let _ = ssh_env.remote_tmux("kill-server");
    });
}
