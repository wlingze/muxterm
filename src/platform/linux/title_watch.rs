//! Pane 标题跟踪：UI-local 辅助函数，不访问 core backend。
//!
//! - 本地 pane：沿 `/proc/<pid>` 子进程树取最深 `comm`
//! - tmux pane：调用外部 `tmux display-message`（不走 -CC 通道）

use std::path::PathBuf;
use std::process::Command;

/// 本地 pane：根据 vte 子进程 pid 推断当前应显示的程序名。
pub fn local_foreground_name(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let mut current = pid as u32;
        let mut name = process_name_from_proc(current)?;
        for _ in 0..12 {
            let kids = read_children(current);
            let Some(&next) = kids.last() else {
                break;
            };
            current = next;
            if let Some(n) = process_name_from_proc(current) {
                if !n.is_empty() {
                    name = n;
                }
            }
        }
        Some(name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// tmux pane：查询 `#{pane_current_command}`（调用外部 tmux，不经核心）。
///
/// `socket_args` 需与 -CC 客户端一致（如 `["-L", "muxterm"]`）。
pub fn tmux_pane_command(pane_id: u32, socket_args: &[String]) -> Option<String> {
    let mut cmd = Command::new("tmux");
    cmd.args(socket_args).args([
        "display-message",
        "-p",
        "-t",
        &format!("@{pane_id}"),
        "#{pane_current_command}",
    ]);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(basename_command(&s))
    }
}

/// 路径 basename（`/usr/bin/bash` → `bash`）。纯字符串工具。
pub fn basename_command(s: &str) -> String {
    PathBuf::from(s)
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or(s)
        .to_string()
}

#[cfg(target_os = "linux")]
fn process_name_from_proc(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let raw = std::fs::read(format!("/proc/{pid}/comm")).ok()?;
    let name = String::from_utf8_lossy(&raw).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(target_os = "linux")]
fn read_children(pid: u32) -> Vec<u32> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    s.split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_watch_basename_bash() {
        assert_eq!(basename_command("/usr/bin/bash"), "bash");
    }

    #[test]
    fn test_title_watch_basename_opencode() {
        assert_eq!(basename_command("/usr/local/bin/opencode"), "opencode");
    }

    #[test]
    fn test_title_watch_basename_python3() {
        assert_eq!(basename_command("python3"), "python3");
        assert_eq!(basename_command("/usr/bin/python3"), "python3");
    }

    #[test]
    fn test_title_watch_basename_fallback() {
        assert_eq!(basename_command(""), "");
        assert_eq!(basename_command("vim"), "vim");
    }

    #[test]
    fn test_title_watch_local_invalid_pid() {
        assert!(local_foreground_name(0).is_none());
        assert!(local_foreground_name(-1).is_none());
    }

    #[test]
    fn test_title_watch_local_self() {
        let pid = std::process::id() as i32;
        let name = local_foreground_name(pid);
        assert!(name.is_some());
    }
}
