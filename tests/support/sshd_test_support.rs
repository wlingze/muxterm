#![allow(dead_code)]
//! SSH 测试支持：为 SSH long-chain 集成测试提供临时 HOME + 显式 ssh config。
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

use std::env;
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

        // client key：优先使用显式 env；其次本机现有 key（测试环境 sshd
        // 通常已授权它）；最后才生成临时 key（需外部 sshd 已预授权）。
        let client_key_path = if let Some(ref k) = key_from_env {
            PathBuf::from(k)
        } else {
            let user_key = env::var_os("HOME")
                .map(PathBuf::from)
                .map(|h| h.join(".ssh").join("id_ed25519"))
                .filter(|p| p.exists());
            match user_key {
                Some(k) => k,
                None => {
                    let k = ssh_dir.join("id_ed25519");
                    run_ssh_keygen(&k)?;
                    k
                }
            }
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
    SendEnv ZDOTDIR
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

/// 本机 `sshd` 二进制（W18：测试自己拉起隔离 daemon，不连用户 22 端口）。
pub fn sshd_binary() -> Option<PathBuf> {
    for candidate in ["sshd", "/usr/sbin/sshd", "/usr/bin/sshd"] {
        if candidate.starts_with('/') {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return Some(p);
            }
        } else if let Ok(out) = Command::new("command").args(["-v", candidate]).output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
        if let Ok(out) = Command::new("which").arg(candidate).output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !s.is_empty() {
                let p = PathBuf::from(s);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// 能自启 loopback sshd + ssh 客户端。
pub fn loopback_sshd_available() -> bool {
    sshd_binary().is_some() && ssh_client_available()
}

fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// 测试进程内隔离 sshd：随机端口、自签密钥、Drop 只杀这个 daemon。
///
/// **禁止**连用户 22 端口 / 真实 `~/.ssh/config`。远端 tmux 一律另用 `-L muxterm-test-*`。
pub struct LoopbackSshd {
    tmp: PathBuf,
    pid: u32,
    pub port: u16,
    pub user: String,
    pub client_key: PathBuf,
    pub config_path: PathBuf,
    pub alias: String,
}

impl LoopbackSshd {
    /// 复用已启动的隔离 sshd（W7 child 不重复拉起 sshd，避免进程堆积）。
    ///
    /// 从 ssh config 解析 alias/port/user；pid=0 表示不拥有 daemon，
    /// Drop 只清理自己的 tmp 目录。
    pub fn attach(alias: String, config_path: PathBuf) -> anyhow::Result<Self> {
        let text = fs::read_to_string(&config_path)?;
        let mut port = 0u16;
        let mut user = String::new();
        let mut client_key = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Port ") {
                port = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("User ") {
                user = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("IdentityFile ") {
                client_key = Some(PathBuf::from(v.trim()));
            }
        }
        if port == 0 || user.is_empty() {
            anyhow::bail!(
                "attach 的 ssh config 缺 Port/User: {}",
                config_path.display()
            );
        }
        let client_key = client_key.unwrap_or_else(dflt_key);
        Ok(Self {
            tmp: PathBuf::new(),
            pid: 0,
            port,
            user,
            client_key,
            config_path: config_path.to_path_buf(),
            alias,
        })
    }

    /// 启动隔离 sshd，并做一次 `echo ok` smoke。
    ///
    /// Host alias 为 `muxterm-loop-{label}-{nanos}`。要固定名叫 `local` 时用
    /// [`Self::start_with_alias`]。
    pub fn start(label: &str) -> anyhow::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        Self::start_with_alias(label, &format!("muxterm-loop-{label}-{nanos}"))
    }

    /// 启动隔离 sshd，SSH config 的 `Host` 用调用方给的 alias（可叫 `local`）。
    ///
    /// 仍听 `127.0.0.1` 随机端口、自签密钥，**不**读用户 `~/.ssh/config`。
    /// tmp 目录仍带 nanos，同 alias 并发不会撞。
    pub fn start_with_alias(label: &str, alias: &str) -> anyhow::Result<Self> {
        let sshd = sshd_binary().ok_or_else(|| anyhow::anyhow!("无 sshd 二进制"))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("muxterm-sshd-{}-{}", label, nanos));
        fs::create_dir_all(&tmp)?;
        let host_ed = tmp.join("host_ed25519");
        let client = tmp.join("client_ed25519");
        run_ssh_keygen(&host_ed)?;
        run_ssh_keygen(&client)?;
        let client_pub = PathBuf::from(format!("{}.pub", client.display()));
        let pub_bytes = fs::read(&client_pub)?;
        let authorized = tmp.join("authorized_keys");
        fs::write(&authorized, pub_bytes)?;
        fs::set_permissions(&authorized, PermissionsExt::from_mode(0o600))?;

        let port = free_loopback_port();
        let user = env::var("USER").unwrap_or_else(|_| "wlz".into());
        let pid_file = tmp.join("sshd.pid");
        let log_file = tmp.join("sshd.log");
        let cfg_path = tmp.join("sshd_config");
        let config = format!(
            "Port {port}\n\
             ListenAddress 127.0.0.1\n\
             HostKey {host}\n\
             PidFile {pid}\n\
             AuthorizedKeysFile {auth}\n\
             PasswordAuthentication no\n\
             PubkeyAuthentication yes\n\
             PermitRootLogin no\n\
             UsePAM no\n\
             StrictModes no\n\
             AcceptEnv ZDOTDIR\n\
             Subsystem sftp internal-sftp\n",
            host = host_ed.display(),
            pid = pid_file.display(),
            auth = authorized.display(),
        );
        fs::write(&cfg_path, config)?;

        let output = Command::new(&sshd)
            .args([
                "-f",
                &cfg_path.to_string_lossy(),
                "-E",
                &log_file.to_string_lossy(),
            ])
            .output()?;
        if !output.status.success() {
            let log = fs::read_to_string(&log_file).unwrap_or_default();
            anyhow::bail!(
                "sshd 启动失败: status={:?} stderr={} log={log}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let mut pid = 0u32;
        for _ in 0..50 {
            if let Ok(s) = fs::read_to_string(&pid_file) {
                if let Ok(p) = s.trim().parse() {
                    pid = p;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if pid == 0 {
            let log = fs::read_to_string(&log_file).unwrap_or_default();
            anyhow::bail!("sshd 未写 pid 文件。log={log}");
        }

        let ssh_dir = tmp.join("client-ssh");
        fs::create_dir_all(&ssh_dir)?;
        fs::set_permissions(&ssh_dir, PermissionsExt::from_mode(0o700))?;
        let alias = alias.to_string();
        let ssh_config_path = ssh_dir.join("config");
        fs::write(
            &ssh_config_path,
            format!(
                "Host {alias}\n\
                 HostName 127.0.0.1\n\
                 Port {port}\n\
                 User {user}\n\
                 IdentityFile {key}\n\
                 IdentitiesOnly yes\n\
                 BatchMode yes\n\
                 StrictHostKeyChecking no\n\
                 UserKnownHostsFile /dev/null\n\
                 SendEnv ZDOTDIR\n\
                 LogLevel ERROR\n",
                key = client.display(),
            ),
        )?;
        fs::set_permissions(&ssh_config_path, PermissionsExt::from_mode(0o600))?;

        let mut smoke = Command::new("ssh");
        smoke
            .arg("-F")
            .arg(&ssh_config_path)
            .arg(&alias)
            .arg("echo ok");
        let smoke_out = smoke.output()?;
        if !smoke_out.status.success() || !String::from_utf8_lossy(&smoke_out.stdout).contains("ok")
        {
            let log = fs::read_to_string(&log_file).unwrap_or_default();
            anyhow::bail!(
                "loopback ssh smoke 失败: stderr={} log={log}",
                String::from_utf8_lossy(&smoke_out.stderr)
            );
        }

        Ok(Self {
            tmp,
            pid,
            port,
            user,
            client_key: client,
            config_path: ssh_config_path,
            alias,
        })
    }

    /// 让 `TmuxRuntime` SSH 路径读这份 `-F` config（禁止用户真实 ~/.ssh/config）。
    pub fn apply_ssh_config_env(&self) {
        std::env::set_var("MUXTERM_SSH_CONFIG_PATH", &self.config_path);
    }

    pub fn ssh_cmd(&self) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.arg("-F").arg(&self.config_path).arg(&self.alias);
        cmd
    }

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

    pub fn remote_tmux(&self, socket: &str, args: &str) -> (bool, String, String) {
        self.remote_exec(&format!("tmux -L {socket} {args}"))
    }
}

impl Drop for LoopbackSshd {
    fn drop(&mut self) {
        if self.pid != 0 {
            let _ = Command::new("kill").arg(self.pid.to_string()).output();
        }
        if !self.tmp.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.tmp);
        }
    }
}

fn dflt_key() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".ssh").join("id_ed25519"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("/dev/null"))
}
