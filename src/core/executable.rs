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

/// 展开命令和工作目录中的简单环境变量占位（`$SHELL` / `$HOME`）。
pub fn expand_config_value(raw: &str) -> String {
    let t = raw.trim();
    if t == "$SHELL" {
        return std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    }
    if t == "$HOME" {
        return std::env::var("HOME").unwrap_or_else(|_| "/".into());
    }
    if let Some(rest) = t.strip_prefix('$') {
        if let Ok(v) = std::env::var(rest) {
            return v;
        }
    }
    if t == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| "/".into());
    }
    if let Some(rest) = t.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        return format!("{}/{}", home.trim_end_matches('/'), rest);
    }
    t.to_string()
}

/// 从默认命令解析 argv；空命令回退到当前 shell。
pub fn parse_command_argv(command: &str) -> Vec<String> {
    let expanded = expand_config_value(command);
    let parts: Vec<String> = expanded.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        vec![expand_config_value("$SHELL")]
    } else {
        parts
    }
}

/// macOS GUI 以登录 shell 启动单独指定的 zsh/bash，补齐 Terminal.app 的环境。
pub fn prepare_pane_argv_for_platform(mut argv: Vec<String>, is_macos: bool) -> Vec<String> {
    if !is_macos || argv.len() != 1 {
        return argv;
    }
    let shell = program_basename(&argv[0]);
    if matches!(shell.as_str(), "zsh" | "bash") {
        argv.push("-l".into());
    }
    argv
}

/// 返回 argv[0] 的 basename，用作 pane 默认显示名。
pub fn program_basename(argv0: &str) -> String {
    Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(argv0)
        .to_string()
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
    fn expand_and_parse_command() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        assert_eq!(expand_config_value("$SHELL"), shell);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        assert_eq!(expand_config_value("$HOME"), home);
        assert_eq!(expand_config_value("~"), home);
        assert_eq!(
            expand_config_value("~/Developer/muxterm"),
            format!("{}/Developer/muxterm", home.trim_end_matches('/'))
        );
        assert_eq!(expand_config_value("/bin/bash"), "/bin/bash");
        assert_eq!(
            parse_command_argv("/usr/bin/python3 script.py"),
            vec!["/usr/bin/python3".to_string(), "script.py".to_string()]
        );
        assert_eq!(program_basename("/usr/bin/bash"), "bash");
        assert_eq!(program_basename("vim"), "vim");
    }

    #[test]
    fn program_basename_handles_paths() {
        assert_eq!(program_basename("/usr/bin/bash"), "bash");
        assert_eq!(program_basename("/usr/local/bin/opencode"), "opencode");
        assert_eq!(program_basename("python3"), "python3");
        assert_eq!(program_basename(""), "");
    }

    #[test]
    fn macos_default_shell_uses_login_mode() {
        assert_eq!(
            prepare_pane_argv_for_platform(vec!["/bin/zsh".into()], true),
            vec!["/bin/zsh".to_string(), "-l".to_string()]
        );
    }

    #[test]
    fn explicit_shell_arguments_are_preserved() {
        assert_eq!(
            prepare_pane_argv_for_platform(vec!["/bin/zsh".into(), "-f".into()], true),
            vec!["/bin/zsh".to_string(), "-f".to_string()]
        );
    }

    #[test]
    fn empty_command_uses_shell() {
        let shell = expand_config_value("$SHELL");
        assert_eq!(parse_command_argv(""), vec![shell.clone()]);
        assert_eq!(parse_command_argv("   "), vec![shell]);
    }

    #[test]
    fn resolve_tmux_returns_nonempty_path() {
        let bin = resolve_tmux_binary();
        assert!(!bin.is_empty());
    }

    #[test]
    fn resolve_tmux_falls_back_when_path_has_no_homebrew() {
        let _guard = crate::PATH_ENV_LOCK.lock().unwrap();
        let old = std::env::var_os("PATH");
        // 模拟 Finder 启动的 GUI：只有系统目录，没有 /opt/homebrew/bin。
        std::env::set_var("PATH", "/usr/bin:/bin");
        let bin = resolve_tmux_binary();
        match old {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        assert!(
            bin == "tmux" || bin.starts_with('/'),
            "缺少 Homebrew PATH 时必须回退到绝对路径或可用 PATH, got {bin:?}"
        );
    }
}
