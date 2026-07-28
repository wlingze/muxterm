#![allow(dead_code)]
//! SSH 测试支持：为 SSH E2E 测试提供临时 HOME + 显式 ssh config。
//!
//! **关键约束**：
//! - **不启动/停止 sshd**：sshd 由外部（CI setup 或本地环境）管理。
//! - 从环境变量读取共享 sshd 连接参数：
//!   - `MUXTERM_TEST_SSH_HOST`（默认 127.0.0.1）
//!   - `MUXTERM_TEST_SSH_PORT`（默认 22）
//!   - `MUXTERM_TEST_SSH_USER`（默认 $USER）
//!   - `MUXTERM_TEST_SSH_KEY`（可选，client key 路径）
//! - 生成的 ssh config 使用 `-F` 显式指定，绝不读取用户真实 `~/.ssh/config`。
//! - SshTestEnv 只创建临时 HOME + ssh config + cleanup guard。

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 每个 SSH 测试的临时环境：HOME + ssh config + 远端 tmux socket 名 + cleanup guard。
///
/// **不启动 sshd**。从环境变量读取共享 sshd 连接参数。
pub struct SshTestEnv {
    /// 临时 HOME 目录（含 .ssh/config）。
    pub home_dir: PathBuf,
    /// 临时 .ssh 目录。
    pub ssh_dir: PathBuf,
    /// ssh config 文件路径（-F 指定）。
    pub ssh_config_path: PathBuf,
    /// ssh alias（写入 config 的 Host 名）。
    pub alias: String,
    /// 连接参数。
    pub host: String,
    pub port: u16,
    pub user: String,
    /// client key 路径（可选；从 env 或自动生成临时 key）。
    pub client_key_path: PathBuf,
    /// 远端 tmux socket 名（每个测试独立）。
    pub remote_tmux_socket: String,
}

/// 读取共享 sshd 连接参数（从环境变量）。
fn read_ssh_params() -> (String, u16, String, Option<String>) {
    let host = std::env::var("MUXTERM_TEST_SSH_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("MUXTERM_TEST_SSH_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(22);
    let user = std::env::var("MUXTERM_TEST_SSH_USER")
        .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "root".into()));
    let key = std::env::var("MUXTERM_TEST_SSH_KEY").ok();
    (host, port, user, key)
}

impl SshTestEnv {
    /// 创建测试环境：临时 HOME + ssh config。
    ///
    /// **不启动 sshd**。从环境变量读取连接参数。
    /// 如果 `MUXTERM_TEST_SSH_KEY` 未设置，在临时 .ssh 下生成一对 ed25519 key。
    pub fn setup(label: &str) -> anyhow::Result<Self> {
        let (host, port, user, key_from_env) = read_ssh_params();

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("muxterm-ssh-test-{}-{}", label, nanos));
        let home_dir = tmp.join("home");
        let ssh_dir = home_dir.join(".ssh");
        fs::create_dir_all(&ssh_dir)?;
        fs::set_permissions(&ssh_dir, PermissionsExt::from_mode(0o700))?;

        // client key：从 env 或自动生成
        let client_key_path = if let Some(ref k) = key_from_env {
            PathBuf::from(k)
        } else {
            let k = ssh_dir.join("id_ed25519");
            run_ssh_keygen(&k)?;
            k
        };

        // ssh config（显式 -F，不读用户真实 config）
        let ssh_config_path = ssh_dir.join("config");
        let alias = format!("test-{}", label);
        let config_content = format!(
            r#"Host {alias}
    HostName {host}
    Port {port}
    User {user}
    IdentityFile {key}
    IdentitiesOnly yes
    BatchMode yes
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
"#,
            alias = alias,
            host = host,
            port = port,
            user = user,
            key = client_key_path.display(),
        );
        fs::write(&ssh_config_path, &config_content)?;
        fs::set_permissions(&ssh_config_path, PermissionsExt::from_mode(0o600))?;

        // 远端 tmux socket（每个测试独立，绝不复用）
        let remote_tmux_socket = format!("muxterm-test-remote-{}-{}", label, nanos);

        Ok(Self {
            home_dir,
            ssh_dir,
            ssh_config_path,
            alias,
            host,
            port,
            user,
            client_key_path,
            remote_tmux_socket,
        })
    }

    /// 设置 HOME 环境变量 + `-F` config 路径。
    pub fn env_for_command<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("HOME", &self.home_dir);
        cmd.arg("-F").arg(&self.ssh_config_path);
        cmd
    }

    /// 远端 ssh 命令前缀：`ssh -F config test-<label>`
    pub fn ssh_cmd(&self) -> Command {
        let mut cmd = Command::new("ssh");
        self.env_for_command(&mut cmd);
        cmd.arg(&self.alias);
        cmd
    }

    /// 在远端执行命令。
    pub fn remote_exec(&self, remote_cmd: &str) -> (bool, String, String) {
        let mut cmd = self.ssh_cmd();
        cmd.arg(remote_cmd);
        let output = cmd
            .output()
            .unwrap_or_else(|e| panic!("SSH 远端执行失败: {e}"));
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }

    /// 远端 tmux 命令（自动加 -L <socket> 前缀）。
    pub fn remote_tmux(&self, args: &str) -> (bool, String, String) {
        let cmd = format!("tmux -L {} {}", self.remote_tmux_socket, args);
        self.remote_exec(&cmd)
    }
}

impl Drop for SshTestEnv {
    fn drop(&mut self) {
        // 清理临时 HOME 目录（不 kill sshd）
        let _ = fs::remove_dir_all(self.home_dir.parent().unwrap_or(&self.home_dir));
    }
}

fn run_ssh_keygen(key_path: &Path) -> anyhow::Result<()> {
    let output = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            &key_path.to_string_lossy(),
            "-N",
            "",
            "-q",
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "ssh-keygen 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// 检查 sshd 是否在目标端口监听（TCP 连通性检查）。
pub fn sshd_available() -> bool {
    let (_, port, _, _) = read_ssh_params();
    std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok()
}

/// 检查 ssh 客户端可用。
pub fn ssh_client_available() -> bool {
    Command::new("ssh")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
