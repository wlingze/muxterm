//! 已有的连接发现（W20）：本地/SSH 的 tmux session + Herdr workspace。
//!
//! 只读、无状态、不建 Runtime。本地 Herdr 走 socket JSON（禁止
//! `Command::new("herdr")`）；SSH 走 `ssh … herdr session list`（discovery
//! 层，与 `ssh … tmux list-sessions` 同类，不是 Runtime）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::core::quickconnect::model::{TargetRuntime, TargetTransport};
use crate::core::runtime::herdr::session::HerdrSession;

use super::{list_local_tmux_sessions, list_ssh_tmux_sessions, TmuxSessionInfo};

/// 已有的连接面板里的一行（tmux session 或 Herdr workspace）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingEntry {
    pub title: String,
    pub runtime: TargetRuntime,
    pub transport: TargetTransport,
    pub tmux_session: Option<String>,
    /// Herdr named session 名（默认 socket 为 "default"）。
    pub herdr_session: Option<String>,
    pub herdr_workspace_id: Option<String>,
    /// 本机绝对路径；SSH 在 prepare 之后才是本地转发路径。
    pub herdr_socket: Option<String>,
}

impl ExistingEntry {
    /// 副标题：`runtime @ transport`（与 Project 行同款）。
    pub fn subtitle(&self) -> String {
        format!("{} @ {}", self.runtime.as_str(), self.transport.label())
    }
}

/// 本地 tmux：只读 `list-sessions`（测试传 `-L muxterm-test-*`）。
pub fn discover_local_tmux(socket: Option<&str>) -> Vec<ExistingEntry> {
    list_local_tmux_sessions(socket)
        .into_iter()
        .map(|s| tmux_entry(s, TargetTransport::Local))
        .collect()
}

fn tmux_entry(s: TmuxSessionInfo, transport: TargetTransport) -> ExistingEntry {
    ExistingEntry {
        title: s.name.clone(),
        runtime: TargetRuntime::Tmux,
        transport,
        tmux_session: Some(s.name),
        herdr_session: None,
        herdr_workspace_id: None,
        herdr_socket: None,
    }
}

/// 本地 Herdr：扫描 `HERDR_SOCKET_PATH`（测试）→ 默认 socket → named sessions。
///
/// `config_dir` 覆盖 `~/.config/herdr`（测试传临时目录，避免扫到用户默认）。
/// 连不上的 socket 跳过，不 panic。
pub fn discover_local_herdr(config_dir: Option<&Path>) -> Vec<ExistingEntry> {
    let mut sockets: Vec<PathBuf> = Vec::new();
    // 测试 override：设了 HERDR_SOCKET_PATH 就只扫它，禁止连用户默认。
    if let Ok(env) = std::env::var("HERDR_SOCKET_PATH") {
        if !env.trim().is_empty() {
            sockets.push(PathBuf::from(env));
            return scan_sockets(sockets, None);
        }
    }
    let base = config_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config/herdr")
    });
    let default = base.join("herdr.sock");
    if default.exists() {
        sockets.push(default.clone());
    }
    if let Ok(entries) = std::fs::read_dir(base.join("sessions")) {
        for entry in entries.flatten() {
            let socket = entry.path().join("herdr.sock");
            if socket.exists() {
                sockets.push(socket);
            }
        }
    }
    scan_sockets(sockets, Some(default))
}

/// 逐个 socket ping + workspace.list，产出 ExistingEntry。
fn scan_sockets(sockets: Vec<PathBuf>, default: Option<PathBuf>) -> Vec<ExistingEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for socket in sockets {
        let session_name = if default.as_deref() == Some(socket.as_path()) {
            "default".to_string()
        } else {
            socket
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let session = HerdrSession::new(&session_name, &socket);
        if session.ping().is_err() {
            continue;
        }
        let Ok(list) = session.workspace_list() else {
            continue;
        };
        for ws in list {
            let key = format!("{session_name}:{}", ws.workspace_id);
            if !seen.insert(key) {
                continue;
            }
            out.push(ExistingEntry {
                title: ws.label,
                runtime: TargetRuntime::Herdr,
                transport: TargetTransport::Local,
                tmux_session: None,
                herdr_session: Some(session_name.clone()),
                herdr_workspace_id: Some(ws.workspace_id),
                herdr_socket: Some(socket.to_string_lossy().to_string()),
            });
        }
    }
    out
}

/// SSH tmux：`ssh … tmux list-sessions`（超时 2s，失败 = 没有 tmux）。
pub fn discover_ssh_tmux(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    timeout: Duration,
) -> Vec<ExistingEntry> {
    match list_ssh_tmux_sessions(alias, ssh_config_path, remote_socket, timeout) {
        Ok(sessions) => sessions
            .into_iter()
            .map(|s| {
                tmux_entry(
                    s,
                    TargetTransport::Ssh {
                        name: alias.to_string(),
                    },
                )
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// SSH Herdr：`ssh … env -u HERDR_ENV -u HERDR_SESSION herdr session list --json`，
/// 再对每个 running session 拉 `workspace list`。失败 = 该 host 无 Herdr。
pub fn discover_ssh_herdr(
    alias: &str,
    ssh_config_path: Option<&str>,
    timeout: Duration,
) -> Vec<ExistingEntry> {
    let Some(sessions) = ssh_herdr_sessions(alias, ssh_config_path, timeout) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (session_name, socket_path) in sessions {
        let cmd = if session_name == "default" {
            "env -u HERDR_ENV -u HERDR_SESSION PATH=\"$HOME/.local/bin:$PATH\" herdr workspace list"
                .to_string()
        } else {
            format!(
                "env -u HERDR_ENV -u HERDR_SESSION PATH=\"$HOME/.local/bin:$PATH\" herdr --session {session_name} workspace list"
            )
        };
        let Some(stdout) = ssh_run(alias, ssh_config_path, &cmd, timeout) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) else {
            continue;
        };
        let Some(workspaces) = v
            .get("result")
            .and_then(|r| r.get("workspaces"))
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for ws in workspaces {
            let Some(ws_id) = ws.get("workspace_id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let label = ws
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(ws_id);
            out.push(ExistingEntry {
                title: label.to_string(),
                runtime: TargetRuntime::Herdr,
                transport: TargetTransport::Ssh {
                    name: alias.to_string(),
                },
                tmux_session: None,
                herdr_session: Some(session_name.clone()),
                herdr_workspace_id: Some(ws_id.to_string()),
                herdr_socket: Some(socket_path.clone()),
            });
        }
    }
    out
}

/// `ssh … herdr session list --json` → `(session_name, socket_path)`（running 的）。
fn ssh_herdr_sessions(
    alias: &str,
    ssh_config_path: Option<&str>,
    timeout: Duration,
) -> Option<Vec<(String, String)>> {
    let cmd = "env -u HERDR_ENV -u HERDR_SESSION PATH=\"$HOME/.local/bin:$PATH\" herdr session list --json";
    let stdout = ssh_run(alias, ssh_config_path, cmd, timeout)?;
    let v: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let sessions = v.get("sessions").or_else(|| v.get("result"))?;
    let sessions = sessions
        .as_array()
        .cloned()
        .or_else(|| {
            sessions
                .get("sessions")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for s in sessions {
        let status = s
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let running = s
            .get("running")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if status != "running" && !running {
            continue;
        }
        let name = s
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            .to_string();
        let socket = s
            .get("socket_path")
            .or_else(|| s.get("socket"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push((name, socket));
    }
    Some(out)
}

/// 跑一条 ssh 只读命令，硬超时；失败返回 None。
///
/// 用读线程收 output，主线程只等 channel：`try_wait` 会 reap 子进程，
/// 之后再 `wait_with_output` 拿不到 stdout。
fn ssh_run(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_cmd: &str,
    timeout: Duration,
) -> Option<String> {
    let mut cmd = Command::new("ssh");
    cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=2"]);
    if let Some(cfg) = ssh_config_path {
        cmd.args(["-F", cfg]);
    }
    cmd.arg(alias).arg("--").arg(remote_cmd);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<Vec<u8>>>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let result = std::io::Read::read_to_end(&mut stdout, &mut buf).map(|_| buf);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + timeout;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(bytes)) => {
                let status = child.wait().ok()?;
                if !status.success() {
                    return None;
                }
                return Some(String::from_utf8_lossy(&bytes).to_string());
            }
            Ok(Err(_)) => return None,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W20d 纯逻辑：空目录 + 无 env → 空列表，不 panic。
    #[test]
    fn discover_local_herdr_empty_without_sockets() {
        let tmp = std::env::temp_dir().join(format!(
            "muxterm-test-herdr-disc-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HERDR_SOCKET_PATH", "");
        let entries = discover_local_herdr(Some(&tmp));
        assert!(entries.is_empty(), "空目录 + 无 env 必须为空: {entries:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
