//! OS 差异的唯一出口：进程终止、用户目录、本地 IPC 能力、默认 shell。
//!
//! 这个模块在三平台都**必须编译**。Unix 专有的东西（UnixSocket / fork /
//! POSIX 信号 / chmod）在这里收口：能跨平台的给实现，不能跨平台的给
//! 能力查询 + 可读错误，避免上层散落 `cfg(unix)` 与悬空符号。

use std::path::PathBuf;

/// 本平台支持 Unix domain socket 吗？
///
/// tmux `-CC` 控制 client、Herdr、shell daemon 都用它；Windows 上这些
/// 后端不可用（本地 shell 走 ConPTY，不经过 socket）。
pub const SUPPORTS_UNIX_SOCKET: bool = cfg!(unix);

/// 终止方式。Windows 没有 POSIX 信号，全部落到 `taskkill /T /F`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminate {
    /// SIGHUP：终端断开。
    Hangup,
    /// SIGTERM：优雅结束。
    Term,
    /// SIGKILL：强制结束。
    Kill,
}

/// 给进程发终止请求。`Ok(())` 包含「进程已经不在」。
#[cfg(unix)]
pub fn terminate_process(pid: i32, how: Terminate) -> std::io::Result<()> {
    let signal = match how {
        Terminate::Hangup => libc::SIGHUP,
        Terminate::Term => libc::SIGTERM,
        Terminate::Kill => libc::SIGKILL,
    };
    // SAFETY: `kill` 只读 pid 与信号编号，失败通过返回值报告。
    if unsafe { libc::kill(pid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        // 进程已经退出：对调用方来说目标达成。
        return Ok(());
    }
    Err(error)
}

/// Windows：用 `taskkill /PID <pid> /T /F` 结束进程树。
///
/// 没有 PID 级信号语义，三个 `Terminate` 都按强制结束处理：ConPTY 里杀掉
/// 子进程等价于关闭 pane，先 graceful 再 fallback 反而会留下半死的 shell。
#[cfg(windows)]
pub fn terminate_process(pid: i32, _how: Terminate) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`：不要让 taskkill 闪一个黑框。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    // 128 = 进程不存在（taskkill 的 "not found"）。
    if output.status.code() == Some(128) {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "taskkill {pid} 失败: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// 家目录：Unix 用 `HOME`，Windows 用 `USERPROFILE`。
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// 配置根目录（不含应用名）。
///
/// Unix：`$XDG_CONFIG_HOME` 或 `~/.config`；
/// Windows：`%APPDATA%`（Roaming）；
/// 都拿不到时返回 `None`，调用方决定退路。
pub fn config_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(home_dir)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".config")))
    }
}

/// 缓存根目录：自更新下载、临时产物。
///
/// Unix：`$XDG_CACHE_HOME` 或 `~/.cache`；Windows：`%LOCALAPPDATA%`。
pub fn cache_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(home_dir)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".cache")))
    }
}

/// 图片粘贴的本地落盘目录。
///
/// Unix 固定 `/tmp`：远程目标是同一套 `/tmp` 语义，本地路径也会写回 pane，
/// 用 macOS 的 per-user `TMPDIR` 会让同一个会话里出现两种路径风格。
/// Windows 没有 `/tmp`，用系统临时目录（`%TEMP%`）。
pub fn paste_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::temp_dir()
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/tmp")
    }
}

/// 默认 shell：Unix `$SHELL` → `/bin/sh`；Windows `%COMSPEC%` → `cmd.exe`。
pub fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

/// 本地 IPC 不可用时给用户看的话（错误信息里带上平台原因）。
pub fn unix_socket_unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{what} 需要 Unix domain socket，当前平台（{}）不支持：\
         请改用本机 shell 或 SSH transport",
        std::env::consts::OS
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_matches_the_target_family() {
        assert_eq!(SUPPORTS_UNIX_SOCKET, cfg!(unix));
    }

    #[test]
    fn home_and_config_dirs_are_available_on_every_platform() {
        // CI 三个平台都有 HOME/USERPROFILE/APPDATA，缺了就是环境异常。
        assert!(home_dir().is_some(), "家目录必须可解析");
        assert!(config_home().is_some(), "配置目录必须可解析");
        assert!(cache_home().is_some(), "缓存目录必须可解析");
    }

    #[test]
    fn paste_dir_is_absolute() {
        let dir = paste_dir();
        assert!(dir.is_absolute(), "粘贴目录必须是绝对路径: {dir:?}");
    }

    #[test]
    fn default_shell_is_non_empty() {
        assert!(!default_shell().trim().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn terminating_a_missing_process_does_not_panic() {
        // 一个几乎不可能存在的 pid：内核回 ESRCH，本函数映射成 Ok；
        // 其它 errno 允许原样返回 Err，但绝不能 panic。
        let _ = terminate_process(i32::MAX / 2, Terminate::Term);
    }

    #[test]
    fn unix_socket_error_mentions_the_platform() {
        let message = unix_socket_unsupported("tmux -CC").to_string();
        assert!(message.contains("tmux -CC"));
        assert!(message.contains(std::env::consts::OS));
    }
}
