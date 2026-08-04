//! Discovery 层：连接前的无状态查询能力。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §7。
//!
//! Discovery 不建立长连接，不画进主运行时层。
//! - SSH hosts：只读取 `~/.ssh/config` 的 Host alias，不做 DNS/认证
//! - tmux sessions：`tmux -L list-sessions` 或 `ssh <alias> tmux list-sessions`
//! - 目录列表：`std::fs::read_dir` 或 `ssh <alias> ls`
//!
//! v1：先建立最小 facade，不阻塞 local CLI。

/// SSH Host 条目（从 `~/.ssh/config` 读取）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshHostEntry {
    /// Host 别名（如 "myserver"）。
    pub alias: String,
    /// HostName（如 "server.example.com"）。
    pub hostname: String,
    /// Port（默认 22）。
    pub port: u16,
    /// User。
    pub user: String,
}

/// tmux session 信息。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TmuxSessionInfo {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
    pub created: u64,
}

/// 目录条目。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

/// 列出本地 tmux server 的 session。
///
/// 执行 `tmux -L <socket> list-sessions -F '...'`，解析 TSV 输出。
/// 不建立 tmux -CC 控制连接，只是一次性 exec。
pub fn list_local_tmux_sessions(socket: Option<&str>) -> Vec<TmuxSessionInfo> {
    let mut cmd = std::process::Command::new("tmux");
    if let Some(s) = socket {
        cmd.args(["-L", s]);
    }
    cmd.args([
        "list-sessions",
        "-F",
        "#{session_name},#{session_windows},#{session_attached},#{session_created}",
    ]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let output = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() != 4 {
                return None;
            }
            Some(TmuxSessionInfo {
                name: parts[0].to_string(),
                windows: parts[1].parse().unwrap_or(0),
                attached: parts[2] == "1",
                created: parts[3].parse().unwrap_or(0),
            })
        })
        .collect()
}

/// 通过 SSH transport 在远端执行 `tmux list-sessions`，解析结果。
///
/// 使用 muxterm 自己的 `SshProcessTransport`（spawn `ssh <alias> -- tmux list-sessions`），
/// 不直接调用 `ssh` 子进程作为产品路径。transport 的 read 非阻塞，
/// 用后台线程收集输出，有硬超时。
pub fn list_ssh_tmux_sessions(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<TmuxSessionInfo>> {
    use crate::core::transport::ssh::{build_ssh_command, SshProcessTransport};
    use crate::core::transport::{PtySize, Transport, TransportSignal};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    let remote_tmux = if let Some(sk) = remote_socket {
        format!("tmux -L {} list-sessions -F '#{{session_name}},#{{session_windows}},#{{session_attached}},#{{session_created}}'", sk)
    } else {
        "tmux list-sessions -F '#{session_name},#{session_windows},#{session_attached},#{session_created}'".to_string()
    };
    let (program, args) = build_ssh_command(alias, &remote_tmux, ssh_config_path);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let mut transport = SshProcessTransport::new();
    transport
        .spawn_exec(&program, &arg_refs, PtySize::new(80, 24))
        .map_err(|e| anyhow::anyhow!("SSH transport spawn 失败: {e}"))?;

    let transport = Arc::new(Mutex::new(transport));

    // 后台线程读 transport 输出
    let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
    let read_transport = transport.clone();
    let read_handle = std::thread::spawn(move || loop {
        let mut t = read_transport.lock().unwrap();
        match t.read() {
            Ok(Some(data)) => {
                drop(t);
                if tx.send(data).is_err() {
                    break;
                }
            }
            Ok(None) => {
                drop(t);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => break,
        }
    });

    // 收集输出直到超时或子进程退出
    let deadline = Instant::now() + timeout;
    let mut all_output = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(data) => all_output.extend_from_slice(&data),
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                // 检查子进程是否已退出
                let mut t = transport.lock().unwrap();
                if let Ok(Some(_)) = t.try_wait() {
                    drop(t);
                    break;
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // kill 子进程，等读线程结束（会把剩余 pty 输出投递到 channel），再一次性收尾。
    {
        let mut t = transport.lock().unwrap();
        let _ = t.kill(TransportSignal::Term);
    }
    let _ = read_handle.join();
    // 读线程已退出并关闭 channel：此刻的 rx.try_recv 能取到「最后一块」输出，
    // 避免旧实现只 try_recv 一次就 break 而丢掉尾部数据（CI 负载下偶发空列表）。
    while let Ok(data) = rx.try_recv() {
        all_output.extend_from_slice(&data);
    }

    // Check child exit code: nonzero = SSH or remote command failed
    let exit_code = {
        let mut t = transport.lock().unwrap();
        t.try_wait().ok().flatten()
    };
    if let Some(code) = exit_code {
        if code != 0 {
            let text = String::from_utf8_lossy(&all_output);
            let stderr = text.to_string();
            // tmux list-sessions returns exit 1 when no server running — that's "no sessions"
            // ssh connection failures return 255
            if code == 255 {
                return Err(anyhow::anyhow!(
                    "SSH connection failed (exit {code}): {stderr}"
                ));
            }
            // exit 1 from tmux = no sessions, return empty list (not error)
        }
    }

    let text = String::from_utf8_lossy(&all_output);
    let sessions: Vec<TmuxSessionInfo> = text
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() != 4 {
                return None;
            }
            Some(TmuxSessionInfo {
                name: parts[0].to_string(),
                windows: parts[1].parse().unwrap_or(0),
                attached: parts[2] == "1",
                created: parts[3].parse().unwrap_or(0),
            })
        })
        .collect();

    Ok(sessions)
}

/// 通过 SSH transport 在远端执行 `tmux list-panes`，解析结果。
///
/// 使用 muxterm 自己的 `SshProcessTransport`，不直接调用 raw ssh。
/// SSH 远端 pane 信息：(pane_id, active, cols, rows, title)
pub type SshPaneInfo = (u32, bool, u16, u16, String);

pub fn list_ssh_tmux_panes(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    session: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<SshPaneInfo>> {
    use crate::core::transport::ssh::{build_ssh_command, SshProcessTransport};
    use crate::core::transport::{PtySize, Transport, TransportSignal};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    let remote_tmux = if let Some(sk) = remote_socket {
        format!(
            "tmux -L {} list-panes -t {} -F '#{{pane_id}},#{{pane_active}},#{{pane_width}},#{{pane_height}},#{{pane_title}}'",
            sk, session
        )
    } else {
        format!(
            "tmux list-panes -t {} -F '#{{pane_id}},#{{pane_active}},#{{pane_width}},#{{pane_height}},#{{pane_title}}'",
            session
        )
    };
    let (program, args) = build_ssh_command(alias, &remote_tmux, ssh_config_path);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let mut transport = SshProcessTransport::new();
    transport
        .spawn_exec(&program, &arg_refs, PtySize::new(80, 24))
        .map_err(|e| anyhow::anyhow!("SSH transport spawn 失败: {e}"))?;

    let transport = Arc::new(Mutex::new(transport));
    let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
    let rt = transport.clone();
    let read_handle = std::thread::spawn(move || loop {
        let mut t = rt.lock().unwrap();
        match t.read() {
            Ok(Some(data)) => {
                drop(t);
                if tx.send(data).is_err() {
                    break;
                }
            }
            Ok(None) => {
                drop(t);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => break,
        }
    });

    let deadline = Instant::now() + timeout;
    let mut all_output = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(data) => all_output.extend_from_slice(&data),
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let mut t = transport.lock().unwrap();
                if let Ok(Some(code)) = t.try_wait() {
                    if code != 0 && code != 1 {
                        let text = String::from_utf8_lossy(&all_output);
                        return Err(anyhow::anyhow!(
                            "SSH remote pane list failed (exit {code}): {text}"
                        ));
                    }
                    drop(t);
                    break;
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    {
        let mut t = transport.lock().unwrap();
        let _ = t.kill(TransportSignal::Term);
    }
    let _ = read_handle.join();
    // 读线程结束后再 drain 尾部数据（见 list_ssh_tmux_sessions 的说明）。
    while let Ok(d) = rx.try_recv() {
        all_output.extend_from_slice(&d);
    }

    // Check child exit code: nonzero (except 1 = tmux no server) = error
    let exit_code = {
        let mut t = transport.lock().unwrap();
        t.try_wait().ok().flatten()
    };
    if let Some(code) = exit_code {
        if code != 0 && code != 1 {
            let text = String::from_utf8_lossy(&all_output);
            if code == 255 {
                return Err(anyhow::anyhow!(
                    "SSH connection failed (exit {code}): {text}"
                ));
            }
            // Other nonzero = remote command failed
            return Err(anyhow::anyhow!(
                "SSH remote pane list failed (exit {code}): {text}"
            ));
        }
    }

    let text = String::from_utf8_lossy(&all_output);
    let panes: Vec<SshPaneInfo> = text
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() < 5 {
                return None;
            }
            let pane_id = parts[0].strip_prefix('%')?.parse().ok()?;
            let active = parts[1] == "1";
            let cols: u16 = parts[2].parse().unwrap_or(80);
            let rows: u16 = parts[3].parse().unwrap_or(24);
            let title = parts[4].to_string();
            Some((pane_id, active, cols, rows, title))
        })
        .collect();

    Ok(panes)
}

/// 列出本地 SSH 配置里的 Host alias（从 `~/.ssh/config` 读取）。
///
/// 只解析 `Host` 条目，忽略通配符 `Host *`（不作为可选机器）。不做 DNS/认证。
/// 解析失败（无配置 / 无法读取）返回空列表而非错误。
pub fn list_local_ssh_hosts(ssh_config_path: Option<&str>) -> Vec<String> {
    let path = ssh_config_path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
                .join(".ssh")
                .join("config")
        });
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };

    let mut hosts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("Match ") {
            continue;
        }
        if line.len() < 5 || !line[..5].eq_ignore_ascii_case("Host ") {
            continue;
        }
        let rest = line[5..].trim();
        if rest.is_empty() || rest == "*" {
            continue;
        }
        for alias in rest.split_whitespace() {
            hosts.push(alias.to_string());
        }
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

/// 列出本地目录条目（名字 + 是否目录），用于「创建新 session 时选目录」。
///
/// 非目录 / 无权限返回空列表。
pub fn list_local_dir(path: &std::path::Path) -> Vec<FsEntry> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(FsEntry {
            name,
            is_dir,
            size: 0,
            modified: 0,
        });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_host_entry_serializable() {
        let e = SshHostEntry {
            alias: "myserver".into(),
            hostname: "example.com".into(),
            port: 22,
            user: "alice".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"alias\":\"myserver\""));
        let back: SshHostEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn tmux_session_info_serializable() {
        let s = TmuxSessionInfo {
            name: "dev".into(),
            windows: 3,
            attached: true,
            created: 1234567890,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"name\":\"dev\""));
    }

    #[test]
    fn fs_entry_serializable() {
        let e = FsEntry {
            name: "test.txt".into(),
            is_dir: false,
            size: 1024,
            modified: 1234567890,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"is_dir\":false"));
    }

    #[test]
    fn list_local_tmux_sessions_nonexistent_socket() {
        // 使用一个不存在的 socket 名，应返回空而非 panic
        let sessions = list_local_tmux_sessions(Some("muxterm-test-nonexistent-xyz"));
        assert!(sessions.is_empty(), "不存在的 socket 应返回空列表");
    }
}
