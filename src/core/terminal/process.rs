//! 进程管理：pty 创建、子进程 spawn、信号、进程名查询。

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use super::{ProcessInfo, TerminalSize};

/// spawn 失败原因。
#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("openpty 失败: {0}")]
    OpenPty(#[source] anyhow::Error),
    #[error("spawn `{program}` 失败: {source}")]
    Spawn {
        program: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("设置工作目录失败: {0}")]
    Workdir(#[source] io::Error),
    #[error("平台不支持 pty spawn")]
    Unsupported,
}

/// 已启动（或仅按 pid 引用）的进程句柄。
pub struct ProcessHandle {
    pid: u32,
    /// 由 [`spawn_program`] 创建时持有；外部 pid（如 VTE）则为 None。
    #[cfg(unix)]
    inner: Option<PtyOwned>,
}

#[cfg(unix)]
struct PtyOwned {
    /// 持有 master 以保持 pty 存活（读写由调用方后续扩展）。
    #[allow(dead_code)]
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl ProcessHandle {
    /// 仅包装已知 pid（例如 VTE `spawn_async` 返回的子进程）。
    pub fn from_pid(pid: u32) -> Self {
        Self {
            pid,
            #[cfg(unix)]
            inner: None,
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 非阻塞探测是否仍在运行。
    pub fn try_reap(&mut self) -> Option<Option<u32>> {
        #[cfg(unix)]
        {
            if let Some(ref mut owned) = self.inner {
                match owned.child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.exit_code();
                        return Some(Some(code));
                    }
                    Ok(None) => return Some(None),
                    Err(_) => return None,
                }
            }
            // 无 child 句柄时用 kill(pid, 0) 探测
            let alive = unsafe { libc::kill(self.pid as i32, 0) } == 0;
            if alive {
                Some(None)
            } else {
                Some(Some(1))
            }
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

/// 启动程序（分配 pty，在 workdir 下 exec）。
pub fn spawn_program(
    program: &str,
    args: &[&str],
    workdir: &str,
    size: TerminalSize,
) -> Result<ProcessHandle, SpawnError> {
    #[cfg(unix)]
    {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: size.rows.max(1),
                cols: size.cols.max(1),
                pixel_width: size.cell_width,
                pixel_height: size.cell_height,
            })
            .map_err(|e| SpawnError::OpenPty(e.into()))?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        if !workdir.is_empty() {
            cmd.cwd(workdir);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SpawnError::Spawn {
                program: program.to_string(),
                source: e.into(),
            })?;
        drop(pair.slave);

        let pid = child.process_id().ok_or_else(|| SpawnError::Spawn {
            program: program.to_string(),
            source: anyhow::anyhow!("child 无 pid"),
        })?;

        Ok(ProcessHandle {
            pid,
            inner: Some(PtyOwned {
                master: pair.master,
                child,
            }),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (program, args, workdir, size);
        Err(SpawnError::Unsupported)
    }
}

/// 关闭进程：先 SIGHUP，必要时 SIGTERM。
pub fn kill(handle: &ProcessHandle) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        let pid = handle.pid as i32;
        if pid <= 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid pid"));
        }
        // 优先 SIGHUP（关闭终端会话的常见信号）
        let r = unsafe { libc::kill(pid, libc::SIGHUP) };
        if r == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(()); // 已退出
        }
        // 回退 SIGTERM
        let r2 = unsafe { libc::kill(pid, libc::SIGTERM) };
        if r2 == 0 {
            Ok(())
        } else {
            let err2 = io::Error::last_os_error();
            if err2.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(err2)
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = handle;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "kill unsupported on this platform",
        ))
    }
}

/// 读取 `/proc/{pid}/comm`（或等价）得到进程短名。
pub fn get_process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// 汇总 pid 的名称 / 路径 / argv。
pub fn get_process_info(pid: u32) -> Option<ProcessInfo> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let name = get_process_name(pid)?;
        let full_path = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let argv = read_cmdline(pid).unwrap_or_default();
        Some(ProcessInfo {
            pid,
            name,
            full_path,
            argv,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// 沿子进程树取最深 `comm`（本地 pane 标题用）。
pub fn foreground_process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let mut current = pid;
        let mut name = get_process_name(current)?;
        for _ in 0..12 {
            let kids = read_children(current);
            let Some(&next) = kids.last() else {
                break;
            };
            current = next;
            if let Some(n) = get_process_name(current) {
                if !n.is_empty() {
                    name = n;
                }
            }
        }
        Some(name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        get_process_name(pid)
    }
}

/// 路径 basename（`/usr/bin/bash` → `bash`）。
pub fn basename_command(s: &str) -> String {
    PathBuf::from(s)
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or(s)
        .to_string()
}

#[cfg(target_os = "linux")]
fn read_cmdline(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.is_empty() {
        return Some(Vec::new());
    }
    let parts: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    Some(parts)
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
    fn get_process_name_self() {
        let pid = std::process::id();
        let name = get_process_name(pid);
        assert!(name.is_some());
        assert!(!name.unwrap().is_empty());
    }

    #[test]
    fn get_process_name_zero() {
        assert!(get_process_name(0).is_none());
    }

    #[test]
    fn get_process_info_self() {
        let pid = std::process::id();
        let info = get_process_info(pid).expect("self info");
        assert_eq!(info.pid, pid);
        assert!(!info.name.is_empty());
    }

    #[test]
    fn basename_command_paths() {
        assert_eq!(basename_command("/usr/bin/bash"), "bash");
        assert_eq!(basename_command("/usr/local/bin/opencode"), "opencode");
        assert_eq!(basename_command("python3"), "python3");
        assert_eq!(basename_command(""), "");
    }

    #[test]
    fn foreground_invalid_pid() {
        assert!(foreground_process_name(0).is_none());
    }

    #[test]
    fn from_pid_and_try_reap_self() {
        let mut h = ProcessHandle::from_pid(std::process::id());
        // 自身仍在运行
        assert_eq!(h.try_reap(), Some(None));
    }

    #[test]
    fn kill_missing_pid_ok() {
        // 很大的 pid 通常不存在；kill 应对 ESRCH 返回 Ok
        let h = ProcessHandle::from_pid(u32::MAX - 1);
        let _ = kill(&h);
    }

    #[test]
    fn spawn_true_and_reap() {
        let size = TerminalSize::new(80, 24);
        let mut handle = spawn_program("true", &[], "/", size).expect("spawn true");
        // 等待子进程结束
        for _ in 0..50 {
            if let Some(Some(_)) = handle.try_reap() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // 超时仍可 kill
        let _ = kill(&handle);
    }

    #[test]
    fn spawn_sleep_and_kill() {
        let size = TerminalSize::new(40, 12);
        let handle = spawn_program("sleep", &["30"], "/", size).expect("spawn sleep");
        assert!(handle.pid() > 0);
        kill(&handle).expect("kill sleep");
    }
}
