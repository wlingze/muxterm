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
        "#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}",
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
            let parts: Vec<&str> = line.split('\t').collect();
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
