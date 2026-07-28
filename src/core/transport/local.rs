//! LocalProcessTransport：在本地用 portable-pty 分配 PTY 对，spawn 子进程。
//!
//! 复用现有 `core::tmux::pty` 和 `core::terminal::process` 的模式。
//! 不理解 shell/tmux 语义，只管字节流 + PTY 控制。

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use super::{Transport, TransportError, TransportSignal};

/// 本地 PTY 进程 Transport。
///
/// 内部用 portable-pty 分配 PTY 对，后台读线程把 master 端字节喂进 mpsc channel，
/// `read()` 非阻塞从 channel 取。
pub struct LocalProcessTransport {
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    pid: Option<u32>,
}

impl LocalProcessTransport {
    /// 创建尚未 spawn 的 Transport。
    pub fn new() -> Self {
        Self {
            master: None,
            child: None,
            reader: None,
            stderr_buf: Arc::new(Mutex::new(Vec::new())),
            pid: None,
        }
    }
}

impl Default for LocalProcessTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for LocalProcessTransport {
    fn spawn_exec(&mut self, program: &str, args: &[&str], pty_size: super::PtySize) -> Result<()> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: pty_size.rows.max(1),
                cols: pty_size.cols.max(1),
                pixel_width: pty_size.pixel_width,
                pixel_height: pty_size.pixel_height,
            })
            .map_err(|e| anyhow::anyhow!(TransportError::Spawn(e.to_string())))?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| {
            anyhow::anyhow!(TransportError::Spawn(format!("spawn {program} 失败: {e}")))
        })?;
        drop(pair.slave);

        let pid = child.process_id().unwrap_or(0);
        self.pid = Some(pid);

        // 后台读线程
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!(TransportError::Spawn(format!("clone reader: {e}"))))?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        self.master = Some(pair.master);
        self.child = Some(child);
        self.reader = Some(rx);
        Ok(())
    }

    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let Some(rx) = self.reader.as_mut() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(data) => Ok(Some(data)),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => Ok(None),
        }
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let Some(master) = self.master.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "transport not started",
            ));
        };
        let mut writer = master
            .take_writer()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        writer.write_all(data)?;
        writer.flush()?;
        Ok(data.len())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let Some(master) = self.master.as_mut() else {
            return Err(anyhow::anyhow!(TransportError::NotStarted));
        };
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize pty 失败")?;
        Ok(())
    }

    fn kill(&mut self, signal: TransportSignal) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(()); // 未启动，nothing to kill
        };
        let pid = child.process_id().unwrap_or(0) as i32;
        if pid <= 0 {
            // fallback: portable-pty Child::kill
            child
                .kill()
                .map_err(|e| anyhow::anyhow!("kill 失败: {e}"))?;
            return Ok(());
        }
        let sig = match signal {
            TransportSignal::Hangup => libc::SIGHUP,
            TransportSignal::Term => libc::SIGTERM,
            TransportSignal::Kill => libc::SIGKILL,
        };
        let r = unsafe { libc::kill(pid, sig) };
        if r != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                return Ok(()); // 已退出
            }
            child
                .kill()
                .map_err(|e| anyhow::anyhow!("kill fallback 失败: {e}"))?;
        }
        Ok(())
    }

    fn try_wait(&mut self) -> std::io::Result<Option<u32>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(Some(0)); // 未启动视为已退出
        };
        match child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.exit_code())),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn shutdown(&mut self) -> Result<()> {
        // 关闭写端
        self.master.take();
        // 等待子进程退出（最多 3 秒）
        if let Some(child) = self.child.as_mut() {
            for _ in 0..60 {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
            // 强制 kill 如果仍在运行
            let _ = child.kill();
        }
        self.child = None;
        self.reader = None;
        self.pid = None;
        Ok(())
    }

    fn stderr(&self) -> Vec<u8> {
        self.stderr_buf.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_transport_spawn_true_and_read_exit() {
        let mut t = LocalProcessTransport::new();
        t.spawn_exec("true", &[], super::super::PtySize::new(40, 12))
            .expect("spawn true");

        // true 很快退出；等最多 2 秒
        let mut exited_code = None;
        for _ in 0..100 {
            if let Ok(Some(code)) = t.try_wait() {
                exited_code = Some(code);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(exited_code.is_some(), "true 应在 2 秒内退出");
    }

    #[test]
    fn local_transport_spawn_sleep_and_kill() {
        let mut t = LocalProcessTransport::new();
        t.spawn_exec("sleep", &["30"], super::super::PtySize::new(40, 12))
            .expect("spawn sleep");
        assert!(t.try_wait().unwrap().is_none(), "sleep 应仍在运行");
        t.kill(TransportSignal::Term).expect("kill sleep");
        // 等退出
        for _ in 0..100 {
            if t.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("sleep 应在 kill 后退出");
    }

    #[test]
    fn local_transport_not_started_errors() {
        let mut t = LocalProcessTransport::new();
        assert!(t.write(b"hi").is_err());
        assert!(t.resize(80, 24).is_err());
        assert!(t.read().unwrap().is_none());
        assert!(t.stderr().is_empty());
    }
}
