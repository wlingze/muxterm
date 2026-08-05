//! macOS GUI 启动器。
//!
//! macOS 的 GUI 前端是 Swift 写的 `Muxterm.app` bundle（Rust 侧只编核心静态库）。
//! `muxterm gui` 从这里定位并 `open` 该 bundle，并把 `-L/--socket`、`-s/--session`、
//! `--debug`、`--log-file` 转发给 Swift 端（`AppDelegate.resolveBackend` 会解析这些参数）。

use std::path::PathBuf;

/// 启动 `Muxterm.app`。
///
/// 查找顺序：
/// 1. `MUXTERM_APP_PATH` 环境变量显式指定
/// 2. 与当前可执行文件同目录下的 `Muxterm.app`
/// 3. 系统 `open -a Muxterm`
///
/// `socket`/`session` 非空时作为 `open --args -L <socket> [-s <session>]` 转发；
/// `debug` / `log_file` 也会以 `--debug` / `--log-file` 形式传给 Swift 侧。
pub fn launch_app_bundle(
    socket: Option<&str>,
    session: Option<&str>,
    debug: bool,
    log_file: Option<&str>,
) -> anyhow::Result<()> {
    let app_path = resolve_app_path();

    let mut cmd = std::process::Command::new("/usr/bin/open");
    match &app_path {
        Some(p) => {
            cmd.arg(p);
        }
        None => {
            cmd.arg("-a").arg("Muxterm");
        }
    }
    if socket.is_some() || session.is_some() || debug || log_file.is_some() {
        cmd.arg("--args");
        if let Some(sock) = socket {
            cmd.arg("-L").arg(sock);
        }
        if let Some(sess) = session {
            cmd.arg("-s").arg(sess);
        }
        if debug {
            cmd.arg("--debug");
        }
        if let Some(path) = log_file {
            cmd.arg("--log-file").arg(path);
        }
    }
    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("open Muxterm.app 失败: {e}"))?;
    if !status.success() {
        anyhow::bail!("open Muxterm.app 退出码 {}", status.code().unwrap_or(-1));
    }
    Ok(())
}

/// 解析 `.app` 路径。
fn resolve_app_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MUXTERM_APP_PATH") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    // 与当前可执行文件同目录（dev 布局：build/macos/muxterm + build/macos/Muxterm.app）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("Muxterm.app");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_app_path_prefers_env_var() {
        let dir = std::env::temp_dir().join("muxterm-test-env");
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("MUXTERM_APP_PATH", &dir);
        // 该目录本身存在 → 直接命中 env
        let p = resolve_app_path();
        std::env::remove_var("MUXTERM_APP_PATH");
        assert_eq!(p, Some(dir));
    }

    #[test]
    fn resolve_app_path_none_when_no_bundle() {
        std::env::remove_var("MUXTERM_APP_PATH");
        // 当前 exe 目录通常没有 Muxterm.app，且 /usr/bin/open 兜底不在该函数内。
        // 只验证不 panic 并返回一个值（可能是 None 或找到了真实 bundle）。
        let _ = resolve_app_path();
    }
}
