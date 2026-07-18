//! PTY 辅助：为 tmux -CC 子进程分配伪终端。
//!
//! tmux 在 `-CC` 模式下仍需要一个 tty 作为「真实终端」；若 stdout 不是 tty，
//! tmux 会 `tcgetattr failed` 并立即退出。本模块用 `portable-pty` 分配一对
//! pty，把子进程的 stdin/stdout/stderr 都接到 slave 端，master 端留给 client
//! 读写——这样对 tmux 而言它跑在一个「正常终端」里，对 client 而言则是字节流。
//!
//! 因为 `portable-pty` 的 reader/writer 是 `Box<dyn std::io::Read/Write>`，
//! 无法直接用 tokio 的 `AsyncFd`（需要 `AsRawFd`），这里用「阻塞读线程 + mpsc
//! 字节块」桥接到 async 世界；写端用 `spawn_blocking` 即可（写量小）。

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// 已 spawn 的 tmux pty 子进程。
pub struct PtyChild {
    /// master 端（用于 take_writer 写命令、管理 child wait）。
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    /// 子进程句柄（用于 kill / wait）。
    pub child: Box<dyn portable_pty::Child + Send>,
}

/// spawn 一个命令到一对 pty。
pub fn spawn_pty(bin: &str, args: &[&str], cols: u16, rows: u16) -> Result<PtyChild> {
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty 失败")?;
    let mut cmd = CommandBuilder::new(bin);
    for a in args {
        cmd.arg(a);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawn {bin} 失败"))?;
    drop(pair.slave); // 关闭 slave，避免 master 读到 EOF 之前一直挂着
    Ok(PtyChild {
        master: pair.master,
        child,
    })
}

/// pty 读端：后台阻塞读线程把字节块喂进 mpsc channel。
///
/// 构造时 spawn 一个 `spawn_blocking` 线程循环 `read`，每读到一块就 send。
/// `rx.recv()` 即可异步拿到字节块；`rx` 返回 None 表示读线程退出（EOF）。
pub struct PtyReader {
    rx: mpsc::Receiver<std::io::Result<Vec<u8>>>,
}

impl PtyReader {
    /// 由 master 的 reader 构造，立即启动后台阻塞读线程。
    pub fn new(mut reader: Box<dyn Read + Send>) -> Self {
        let (tx, rx) = mpsc::channel::<std::io::Result<Vec<u8>>>(4096);
        std::thread::Builder::new()
            .name("muxterm-pty-read".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.blocking_send(Ok(buf[..n].to_vec())).is_err() {
                                // 接收端关闭，退出
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.blocking_send(Err(e));
                            break;
                        }
                    }
                }
            })
            .expect("spawn pty read thread");
        Self { rx }
    }

    /// 异步读一块字节；`Ok(Some(buf))` 表示数据，`Ok(None)` 表示 EOF。
    pub async fn read_chunk(&mut self) -> Option<std::io::Result<Vec<u8>>> {
        self.rx.recv().await
    }
}

/// pty 写端：用 `spawn_blocking` 把同步 `write` 放到阻塞线程池。
///
/// 内部用 `Arc<Mutex<Box<dyn Write>>>` 共享，写量小、串行化足够。
pub struct PtyWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl PtyWriter {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    /// 异步写：把数据克隆后丢到阻塞线程池写。
    pub async fn write_all(&self, data: Vec<u8>) -> std::io::Result<()> {
        let inner = self.inner.clone();
        // 用长超时的 spawn_blocking，避免 tokio 限流阻塞
        let h = tokio::task::spawn_blocking(move || {
            let mut w = inner.lock().unwrap();
            w.write_all(&data)
        });
        h.await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
    }
}

/// 由 master 同时构造 reader 与 writer。
///
/// 注意：`try_clone_reader` 只是 dup fd，并未保证 reader 与 master 各自独立 EOF
/// 语义；这里额外 take 一个独立 writer。调用后 master 仍可作 wait/kill。
pub fn split_master(
    master: &mut Box<dyn portable_pty::MasterPty + Send>,
) -> Result<(PtyReader, PtyWriter)> {
    let reader = master.try_clone_reader().context("try_clone_reader 失败")?;
    let writer = master.take_writer().context("take_writer 失败")?;
    Ok((PtyReader::new(reader), PtyWriter::new(writer)))
}

/// 单独构造 reader（用 try_clone_reader），保留 master 自己持有 writer。
#[allow(dead_code)]
pub fn reader_only(master: &Box<dyn portable_pty::MasterPty + Send>) -> Result<PtyReader> {
    let reader = master.try_clone_reader().context("try_clone_reader 失败")?;
    Ok(PtyReader::new(reader))
}
