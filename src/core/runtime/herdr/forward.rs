//! SSH Herdr attach 支持：把远端 herdr.sock Unix socket 转发到本机临时路径。
//!
//! 兼容测试/诊断路径保留；生产 Runtime provider 使用
//! `TargetConnection::open_channel(UnixSocket)`，不再直接启动该 forward。

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use super::session::client_socket_path_from_api;

static FORWARD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn local_forward_socket_paths() -> (PathBuf, PathBuf) {
    // macOS 的 TMPDIR 路径本身很长；把 SSH alias 拼进去会超过 Unix socket
    // 地址上限，使 ssh -L 在两条转发建立前退出。
    let local_api = std::env::temp_dir().join(format!(
        "mt-hf-{}-{:x}.sock",
        std::process::id(),
        FORWARD_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let local_client = client_socket_path_from_api(&local_api);
    (local_api, local_client)
}

/// 启动一条 ssh Unix socket 转发，返回本机 socket 路径 + 子进程。
pub fn start_herdr_ssh_forward(
    alias: &str,
    remote_socket_path: &str,
    ssh_config_path: Option<&str>,
) -> Result<(PathBuf, Child)> {
    let (local_api, local_client) = local_forward_socket_paths();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_socket_names_are_short_and_unique() {
        let (first_api, first_client) = local_forward_socket_paths();
        let (second_api, second_client) = local_forward_socket_paths();
        assert_ne!(first_api, second_api);
        assert_ne!(first_client, second_client);
        assert!(first_api.file_name().unwrap().len() < 30);
        assert!(first_client.file_name().unwrap().len() < 40);
        assert_eq!(first_client, client_socket_path_from_api(&first_api));
    }
}
