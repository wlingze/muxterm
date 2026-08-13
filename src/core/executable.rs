//! 可执行文件解析。
//!
//! macOS GUI 应用（Finder / `open`）的 PATH 通常只有系统目录，不含
//! Homebrew（`/opt/homebrew/bin`）；直接用 `tmux` 会得到
//! `No such file or directory (os error 2)`。这里统一做 PATH 查找 +
//! 常见安装位置回退。

use std::path::{Path, PathBuf};

/// 返回可用的 tmux 可执行路径：PATH 命中返回 `tmux`，否则回退常见位置。
pub fn resolve_tmux_binary() -> String {
    if which("tmux").is_some() {
        return "tmux".to_string();
    }
    for candidate in [
        "/opt/homebrew/bin/tmux",
        "/usr/local/bin/tmux",
        "/usr/bin/tmux",
        "/opt/local/bin/tmux",
        "/run/current-system/sw/bin/tmux",
    ] {
        if Path::new(candidate).is_file() {
            return (*candidate).to_string();
        }
    }
    "tmux".to_string()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_tmux_returns_nonempty_path() {
        let bin = resolve_tmux_binary();
        assert!(!bin.is_empty());
    }
}
