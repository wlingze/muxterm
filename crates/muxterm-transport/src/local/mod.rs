//! LocalProcessTransport: allocate a PTY pair and spawn a local process.
//!
//! This module does not understand shell/tmux semantics; it owns only the
//! byte stream and PTY lifecycle.

pub mod provider;

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use crate::{Transport, TransportError, TransportResult, TransportSignal};

/// Local PTY process transport.
pub struct LocalProcessTransport {
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    pid: Option<u32>,
}

impl LocalProcessTransport {
    /// Create a transport that has not been spawned yet.
    pub fn new() -> Self {
        Self {
            master: None,
            writer: None,
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

impl LocalProcessTransport {
    fn spawn_command(
        &mut self,
        program: &str,
        cmd: CommandBuilder,
        pty_size: crate::PtySize,
    ) -> Result<()> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: pty_size.rows.max(1),
                cols: pty_size.cols.max(1),
                pixel_width: pty_size.pixel_width,
                pixel_height: pty_size.pixel_height,
            })
            .map_err(|e| anyhow::anyhow!(TransportError::Spawn(e.to_string())))?;

        let child = pair.slave.spawn_command(cmd).map_err(|e| {
            anyhow::anyhow!(TransportError::Spawn(format!("spawn {program} 失败: {e}")))
        })?;
        drop(pair.slave);
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| anyhow::anyhow!(TransportError::Spawn(format!("take writer: {e}"))))?;

        self.pid = Some(child.process_id().unwrap_or(0));
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
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        self.master = Some(pair.master);
        self.writer = Some(writer);
        self.child = Some(child);
        self.reader = Some(rx);
        Ok(())
    }
}

impl Transport for LocalProcessTransport {
    fn spawn_exec(
        &mut self,
        program: &str,
        args: &[&str],
        pty_size: crate::PtySize,
    ) -> TransportResult<()> {
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        Ok(self.spawn_command(program, cmd, pty_size)?)
    }

    fn spawn_exec_with_options(
        &mut self,
        program: &str,
        args: &[&str],
        pty_size: crate::PtySize,
        cwd: Option<&Path>,
        env: &[(String, String)],
    ) -> TransportResult<()> {
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        Ok(self.spawn_command(program, cmd, pty_size)?)
    }

    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let Some(rx) = self.reader.as_mut() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(data) => Ok(Some(data)),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                self.reader = None;
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "local transport EOF",
                ))
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "transport not started",
            ));
        };
        writer.write_all(data)?;
        writer.flush()?;
        Ok(data.len())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> TransportResult<()> {
        let Some(master) = self.master.as_mut() else {
            return Err(TransportError::NotStarted);
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

    fn kill(&mut self, signal: TransportSignal) -> TransportResult<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        let pid = child.process_id().unwrap_or(0) as i32;
        if pid <= 0 {
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
        if unsafe { libc::kill(pid, sig) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            child
                .kill()
                .map_err(|e| anyhow::anyhow!("kill fallback 失败: {e}"))?;
        }
        Ok(())
    }

    fn try_wait(&mut self) -> std::io::Result<Option<u32>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(Some(0));
        };
        match child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.exit_code())),
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn shutdown(&mut self) -> TransportResult<()> {
        self.master.take();
        self.writer.take();
        if let Some(child) = self.child.as_mut() {
            for _ in 0..60 {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
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
        let mut transport = LocalProcessTransport::new();
        transport
            .spawn_exec("true", &[], crate::PtySize::new(40, 12))
            .expect("spawn true");

        let mut exited_code = None;
        for _ in 0..100 {
            if let Ok(Some(code)) = transport.try_wait() {
                exited_code = Some(code);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(exited_code.is_some(), "true 应在 2 秒内退出");
    }

    #[test]
    fn local_transport_spawn_sleep_and_kill() {
        let mut transport = LocalProcessTransport::new();
        transport
            .spawn_exec("sleep", &["30"], crate::PtySize::new(40, 12))
            .expect("spawn sleep");
        assert!(transport.try_wait().unwrap().is_none(), "sleep 应仍在运行");
        transport.kill(TransportSignal::Term).expect("kill sleep");
        for _ in 0..100 {
            if transport.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("sleep 应在 kill 后退出");
    }

    #[test]
    fn local_transport_not_started_errors() {
        let mut transport = LocalProcessTransport::new();
        assert!(transport.write(b"hi").is_err());
        assert!(transport.resize(80, 24).is_err());
        assert!(transport.read().unwrap().is_none());
        assert!(transport.stderr().is_empty());
    }
}
