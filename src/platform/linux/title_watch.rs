//! Pane 标题跟踪：进程名变化时更新显示名。
//!
//! - 本地 pane：沿 `/proc/<pid>` 子进程树取最深 `comm`
//! - tmux pane：调用 `tmux display-message -p '#{pane_current_command}'`
//!   （走独立 tmux 客户端，不经过 -CC 通道）

use std::path::PathBuf;
use std::process::Command;

use crate::core::tmux::protocol::PaneId;

/// 本地 pane：根据 vte 子进程 pid 推断当前应显示的程序名。
pub fn local_foreground_name(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let mut current = pid;
    let mut name = read_comm(current)?;
    // 最多向下走几层，取最深子进程名（bash 里跑 opencode 时通常是叶子）
    for _ in 0..12 {
        let kids = read_children(current);
        let Some(&next) = kids.last() else {
            break;
        };
        current = next;
        if let Some(n) = read_comm(current) {
            if !n.is_empty() {
                name = n;
            }
        }
    }
    Some(name)
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
        // basename：有时是路径
        let base = PathBuf::from(&s)
            .file_name()
            .and_then(|x| x.to_str())
            .unwrap_or(&s)
            .to_string();
        Some(base)
    }
}

fn read_comm(pid: i32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn read_children(pid: i32) -> Vec<i32> {
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
    fn read_comm_self() {
        let pid = std::process::id() as i32;
        let name = read_comm(pid);
        assert!(name.is_some());
    }
}
