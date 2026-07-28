//! SSH loopback sshd test support：为 SSH E2E 测试提供临时 sshd。
//!
//! **安全约束**：
//! - 只监听 127.0.0.1 的随机端口
//! - 动态生成临时 host key、client key、authorized_keys
//! - 不读取用户真实 ~/.ssh/config 或使用真实密钥
//! - 不访问公网
//! - 测试结束自动清理 sshd 进程和临时目录

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// 临时 SSH 测试环境。
pub struct SshTestEnv {
    pub home_dir: PathBuf,
    pub ssh_dir: PathBuf,
    pub ssh_config_path: PathBuf,
    pub alias: String,
    pub port: u16,
    pub sshd: Option<Child>,
    pub sshd_config_path: PathBuf,
    pub host_key_path: PathBuf,
    pub authorized_keys_path: PathBuf,
    pub client_key_path: PathBuf,
}

impl SshTestEnv {
    /// 创建完整的 SSH 测试环境：生成密钥、写 ssh_config、启动 sshd。
    ///
    /// 需要系统安装 `sshd` 和 `ssh-keygen`。
    pub fn setup(label: &str) -> anyhow::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("muxterm-ssh-test-{}-{}", label, nanos));
        fs::create_dir_all(&tmp)?;

        let home_dir = tmp.join("home");
        let ssh_dir = home_dir.join(".ssh");
        fs::create_dir_all(&ssh_dir)?;
        fs::set_permissions(&ssh_dir, PermissionsExt::from_mode(0o700))?;

        // 生成 host key
        let host_key_path = tmp.join("host_key");
        run_ssh_keygen(&host_key_path)?;

        // 生成 client key
        let client_key_path = ssh_dir.join("id_ed25519");
        run_ssh_keygen(&client_key_path)?;

        // authorized_keys
        let authorized_keys_path = ssh_dir.join("authorized_keys");
        let pub_key = fs::read_to_string(format!("{}.pub", client_key_path.display()))?;
        let mut auth_file = fs::File::create(&authorized_keys_path)?;
        auth_file.write_all(pub_key.as_bytes())?;
        fs::set_permissions(&authorized_keys_path, PermissionsExt::from_mode(0o600))?;

        // 找一个空闲端口
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        // sshd_config
        let sshd_run_dir = tmp.join("sshd_run");
        fs::create_dir_all(&sshd_run_dir)?;

        let sshd_config_path = tmp.join("sshd_config");
        let sshd_config = format!(
            r#"Port {port}
ListenAddress 127.0.0.1
HostKey {host_key}
PidFile {tmp}/sshd.pid
AuthorizedKeysFile {authorized_keys}
PasswordAuthentication no
PubkeyAuthentication yes
PermitRootLogin no
UsePAM no
StrictModes no
Subsystem sftp internal-sftp
"#,
            port = port,
            host_key = host_key_path.display(),
            tmp = tmp.display(),
            authorized_keys = authorized_keys_path.display(),
        );
        fs::write(&sshd_config_path, &sshd_config)?;

        // 启动 sshd
        let sshd_child = Command::new("sshd")
            .args([
                "-D",
                "-f",
                &sshd_config_path.to_string_lossy(),
                "-E",
                &tmp.join("sshd.log").to_string_lossy(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("启动 sshd 失败（需安装 openssh-server）：{e}"))?;

        // 等待 sshd 就绪（最多 5 秒）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // 写 ssh_config
        let ssh_config_path = ssh_dir.join("config");
        let alias = format!("test-{}", label);
        let config_content = format!(
            r#"Host {alias}
    HostName 127.0.0.1
    Port {port}
    User {user}
    IdentityFile {client_key}
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
"#,
            alias = alias,
            port = port,
            user = whoami(),
            client_key = client_key_path.display(),
        );
        fs::write(&ssh_config_path, &config_content)?;
        fs::set_permissions(&ssh_config_path, PermissionsExt::from_mode(0o600))?;

        Ok(Self {
            home_dir,
            ssh_dir,
            ssh_config_path,
            alias,
            port,
            sshd: Some(sshd_child),
            sshd_config_path,
            host_key_path,
            authorized_keys_path,
            client_key_path,
        })
    }

    /// 设置 HOME 环境变量（供子进程使用）。
    pub fn env_for_command<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("HOME", &self.home_dir);
        cmd
    }

    /// sshd 日志路径（失败时查看）。
    pub fn sshd_log(&self) -> PathBuf {
        self.home_dir.parent().unwrap().join("sshd.log")
    }
}

impl Drop for SshTestEnv {
    fn drop(&mut self) {
        // 杀 sshd
        if let Some(mut child) = self.sshd.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // 清理临时目录
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

fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".to_string())
}

/// 检查 sshd 是否可用。
pub fn sshd_available() -> bool {
    Command::new("sshd")
        .arg("-V")
        .output()
        .map(|_| true)
        .unwrap_or(false)
        || Command::new("which")
            .arg("sshd")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}
