//! Pane 标题跟踪：进程名变化时更新显示名。
//!
//! - 本地 pane：沿 `/proc/<pid>` 子进程树取最深 `comm`（`core::terminal::process`）
//! - tmux pane：调用 `tmux display-message -p '#{pane_current_command}'`
//!   （走独立 tmux 客户端，不经过 -CC 通道）

use std::process::Command;

use crate::core::terminal::process::{basename_command, foreground_process_name};
use crate::core::tmux::protocol::PaneId;

/// 本地 pane：根据 vte 子进程 pid 推断当前应显示的程序名。
pub fn local_foreground_name(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    foreground_process_name(pid as u32)
}

/// tmux pane：查询 `#{pane_current_command}`。
pub fn tmux_pane_command(pane: PaneId) -> Option<String> {
    let out = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "-t",
            &pane.as_str(),
            "#{pane_current_command}",
        ])
        .output()
        .ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 对应：进程名提取 /usr/bin/bash → bash。
    #[test]
    fn test_title_watch_basename_bash() {
        assert_eq!(basename_command("/usr/bin/bash"), "bash");
    }

    #[test]
    fn test_title_watch_basename_opencode() {
        assert_eq!(basename_command("/usr/local/bin/opencode"), "opencode");
    }

    /// 对应：`python3 /path/to/script.py` 取 argv0 basename。
    #[test]
    fn test_title_watch_basename_python3() {
        assert_eq!(basename_command("python3"), "python3");
        assert_eq!(basename_command("/usr/bin/python3"), "python3");
    }

    /// 对应：未知/空路径回退。
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
