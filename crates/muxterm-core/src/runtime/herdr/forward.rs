//! SSH Herdr attach 支持：把远端 herdr.sock Unix socket 转发到本机临时路径。
//!
//! 生产 attach 禁止 `herdr --remote`（那会在远端装/启 server）。这里用
//! `ssh -nNT -L <local.sock>:<remote_socket_path> <alias>`，转发进程随
//! HerdrRuntime Drop/shutdown 杀掉。

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use super::session::client_socket_path_from_api;

/// 启动一条 ssh Unix socket 转发，返回本机 socket 路径 + 子进程。
pub fn start_herdr_ssh_forward(
    alias: &str,
    remote_socket_path: &str,
    ssh_config_path: Option<&str>,
) -> Result<(PathBuf, Child)> {
    let local_api = std::env::temp_dir().join(format!(
        "muxterm-herdr-fwd-{}-{}.sock",
        alias.replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    let local_client = client_socket_path_from_api(&local_api);
    let remote_api = Path::new(remote_socket_path);
    let remote_client = client_socket_path_from_api(remote_api);
    let _ = std::fs::remove_file(&local_api);
    let _ = std::fs::remove_file(&local_client);
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-nNT",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ConnectTimeout=2",
    ]);
    if let Some(cfg) = ssh_config_path {
        cmd.args(["-F", cfg]);
    }
    cmd.arg("-L")
        .arg(format!("{}:{}", local_api.display(), remote_api.display()))
        .arg("-L")
        .arg(format!(
            "{}:{}",
            local_client.display(),
            remote_client.display()
        ))
        .arg(alias);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn ssh 转发失败（alias={alias}）"))?;

    // 等本地 socket 出现（最多 5s）；失败则杀进程。
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if local_api.exists() && local_client.exists() {
            return Ok((local_api, child));
        }
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&local_api);
    let _ = std::fs::remove_file(&local_client);
    Err(anyhow!(
        "SSH Herdr 双 socket 转发未就绪（alias={alias} api={} client={}）",
        remote_api.display(),
        remote_client.display()
    ))
}
