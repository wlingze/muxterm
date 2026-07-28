//! Session 管理：unix socket 路径 + daemon 进程生命周期。
//!
//! 参考 tmux 的 `-L <socket-name>` 设计：
//! - session name 是用户友好名字
//! - socket 路径派生自 session name：`/tmp/muxterm-<name>.sock`
//! - daemon 进程监听该 socket，持有 LocalBackend + TerminalModel

use std::path::PathBuf;

/// socket 文件根目录。
fn socket_dir() -> PathBuf {
    // 优先用 XDG_RUNTIME_DIR（如 /run/user/1000），否则 /tmp
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// 把 session name 转成合法文件名（替换不安全字符）。
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// session name → unix socket 路径。
///
/// `/tmp/muxterm-<name>.sock` 或 `$XDG_RUNTIME_DIR/muxterm-<name>.sock`
pub fn session_socket_path(name: &str) -> PathBuf {
    socket_dir().join(format!("muxterm-{}.sock", sanitize_name(name)))
}

/// 列出所有活跃 session 的 socket 路径。
///
/// 扫描 socket 目录，找到所有 `muxterm-*.sock` 文件。
/// 只返回 socket 文件存在的（不验证是否真的在监听）。
pub fn list_session_sockets() -> Vec<(String, PathBuf)> {
    let dir = socket_dir();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(name) = fname
                    .strip_prefix("muxterm-")
                    .and_then(|s| s.strip_suffix(".sock"))
                {
                    // 验证是 unix socket（basic check: 文件存在即可）
                    out.push((name.to_string(), path));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// 从 socket 路径反推 session name。
pub fn session_name_from_path(path: &std::path::Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|s| s.strip_prefix("muxterm-"))
        .and_then(|s| s.strip_suffix(".sock"))
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_contains_name() {
        let p = session_socket_path("mywork");
        assert!(p.to_string_lossy().contains("muxterm-mywork.sock"));
    }

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        let p = session_socket_path("my/work space");
        let s = p.to_string_lossy();
        // 路径分隔符会出现在目录部分，只检查文件名部分
        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(!fname.contains('/'), "文件名不应含路径分隔符: {fname}");
        assert!(!fname.contains(' '), "文件名不应含空格: {fname}");
        assert!(fname.contains("my-work-space"), "应替换非法字符: {fname}");
    }

    #[test]
    fn name_from_path_roundtrip() {
        let p = session_socket_path("dev");
        let name = session_name_from_path(&p);
        assert_eq!(name, Some("dev".to_string()));
    }
}
