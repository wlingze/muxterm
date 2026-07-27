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

    /// 异步写：把数据克隆后丢到阻塞线程池写；超时返回 TimedOut，便于 shutdown 推进。
    pub async fn write_all(&self, data: Vec<u8>) -> std::io::Result<()> {
        let inner = self.inner.clone();
        let write_fut = tokio::task::spawn_blocking(move || {
            let mut w = inner.lock().unwrap();
            w.write_all(&data)
        });
        match tokio::time::timeout(std::time::Duration::from_secs(2), write_fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(join_err)) => Err(std::io::Error::other(join_err)),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "pty write timeout (2s)",
            )),
        }
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
#[allow(clippy::borrowed_box)]
pub fn reader_only(master: &Box<dyn portable_pty::MasterPty + Send>) -> Result<PtyReader> {
    let reader = master.try_clone_reader().context("try_clone_reader 失败")?;
    Ok(PtyReader::new(reader))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── spawn_pty ────────────────────────────────────────────────────────

    #[test]
    fn spawn_pty_echo_and_read_output() {
        // /bin/echo 立即输出后退出，master reader 应能读到它的 stdout
        let mut child = spawn_pty("echo", &["hello-pty"], 40, 12).expect("spawn echo");

        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let mut reader = reader;

        // 阻塞读直到拿到包含 "hello-pty" 的块或 EOF
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if String::from_utf8_lossy(&buf).contains("hello-pty") {
                        break;
                    }
                }
                Err(_) => break,
            }
            if std::time::Instant::now() > deadline {
                break;
            }
        }
        let out = String::from_utf8_lossy(&buf);
        assert!(out.contains("hello-pty"), "应读到 echo 输出, 实际: {out:?}");

        // 等待子进程退出
        let _ = child.child.try_wait();
    }

    #[test]
    fn spawn_pty_child_has_pid() {
        let child = spawn_pty("true", &[], 10, 5).expect("spawn true");
        let pid = child.child.process_id();
        assert!(pid.is_some(), "子进程应有 pid");
        assert!(pid.unwrap() > 0);
    }

    #[test]
    fn spawn_pty_missing_binary_errors() {
        let err = spawn_pty("/nonexistent/binary/xyz", &[], 10, 5);
        assert!(err.is_err(), "不存在的二进制应返回 Err");
    }

    #[test]
    fn spawn_pty_cat_echoes_stdin() {
        // /bin/cat 把 stdin 原样输出；写什么读回什么
        let mut child = spawn_pty("cat", &[], 40, 12).expect("spawn cat");

        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let writer = child.master.take_writer().expect("take_writer");
        let mut reader = reader;
        let mut writer = writer;

        // 写一行
        let payload = b"pty-test-line\n";
        writer.write_all(payload).expect("write to cat");

        // 读回（cat 会回显 stdin；终端模式可能 echo，但至少应包含我们写的内容）
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    // 终端 echo + cat 回显，可能读到多次；只要含 payload 即可
                    if String::from_utf8_lossy(&buf).contains("pty-test-line") {
                        break;
                    }
                }
                Err(_) => break,
            }
            if std::time::Instant::now() > deadline {
                break;
            }
        }
        let out = String::from_utf8_lossy(&buf);
        assert!(
            out.contains("pty-test-line"),
            "cat 应回显 stdin, 实际: {out:?}"
        );

        // 关闭 writer（EOF）让 cat 退出
        drop(writer);
        let _ = child.child.try_wait();
    }

    // ── PtyReader ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn pty_reader_streams_chunks() {
        // echo 输出后退出；PtyReader 把字节块通过 mpsc 异步投递
        let mut child = spawn_pty("echo", &["stream-test"], 40, 12).expect("spawn echo");
        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let mut reader = PtyReader::new(reader);

        // 收集所有块直到 EOF 或超时
        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                chunk = reader.read_chunk() => match chunk {
                    Some(Ok(data)) => all.extend_from_slice(&data),
                    Some(Err(_)) | None => break,
                }
            }
            if String::from_utf8_lossy(&all).contains("stream-test") {
                break;
            }
        }
        let out = String::from_utf8_lossy(&all);
        assert!(
            out.contains("stream-test"),
            "PtyReader 应流式投递输出, 实际: {out:?}"
        );
        let _ = child.child.try_wait();
    }

    #[tokio::test]
    async fn pty_reader_eof_returns_none() {
        // true 立即退出；PtyReader 读到 EOF 后 read_chunk 返回 None
        let mut child = spawn_pty("true", &[], 10, 5).expect("spawn true");
        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let mut reader = PtyReader::new(reader);

        // 持续读直到 None（EOF）
        let mut got_none = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                chunk = reader.read_chunk() => match chunk {
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => {
                        got_none = true;
                        break;
                    }
                }
            }
        }
        assert!(got_none, "EOF 后 read_chunk 应返回 None");
        let _ = child.child.try_wait();
    }

    // ── PtyWriter ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn pty_writer_writes_to_cat() {
        // PtyWriter 异步写到 cat 的 stdin，cat 回显
        let mut child = spawn_pty("cat", &[], 40, 12).expect("spawn cat");

        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let writer = child.master.take_writer().expect("take_writer");
        let mut reader = PtyReader::new(reader);
        let writer = PtyWriter::new(writer);

        let payload = b"writer-test\n".to_vec();
        writer
            .write_all(payload)
            .await
            .expect("PtyWriter write_all");

        // 读回
        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                chunk = reader.read_chunk() => match chunk {
                    Some(Ok(data)) => all.extend_from_slice(&data),
                    Some(Err(_)) | None => break,
                }
            }
            if String::from_utf8_lossy(&all).contains("writer-test") {
                break;
            }
        }
        let out = String::from_utf8_lossy(&all);
        assert!(
            out.contains("writer-test"),
            "PtyWriter 写入后 cat 应回显, 实际: {out:?}"
        );
        let _ = child.child.try_wait();
    }

    #[tokio::test]
    async fn pty_writer_multiple_writes_concatenate() {
        let mut child = spawn_pty("cat", &[], 40, 12).expect("spawn cat");
        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let writer = child.master.take_writer().expect("take_writer");
        let mut reader = PtyReader::new(reader);
        let writer = PtyWriter::new(writer);

        // 连续写两块
        writer
            .write_all(b"part1-".to_vec())
            .await
            .expect("write part1");
        writer
            .write_all(b"part2\n".to_vec())
            .await
            .expect("write part2");

        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                chunk = reader.read_chunk() => match chunk {
                    Some(Ok(data)) => all.extend_from_slice(&data),
                    Some(Err(_)) | None => break,
                }
            }
            let s = String::from_utf8_lossy(&all);
            if s.contains("part1-") && s.contains("part2") {
                break;
            }
        }
        let out = String::from_utf8_lossy(&all);
        assert!(
            out.contains("part1-") && out.contains("part2"),
            "两次写入都应被 cat 回显, 实际: {out:?}"
        );
        let _ = child.child.try_wait();
    }

    // ── split_master ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn split_master_provides_reader_and_writer() {
        let mut child = spawn_pty("cat", &[], 40, 12).expect("spawn cat");
        let (mut reader, writer) = split_master(&mut child.master).expect("split_master");

        writer
            .write_all(b"split-test\n".to_vec())
            .await
            .expect("write via split writer");

        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                chunk = reader.read_chunk() => match chunk {
                    Some(Ok(data)) => all.extend_from_slice(&data),
                    Some(Err(_)) | None => break,
                }
            }
            if String::from_utf8_lossy(&all).contains("split-test") {
                break;
            }
        }
        let out = String::from_utf8_lossy(&all);
        assert!(
            out.contains("split-test"),
            "split_master 的 reader+writer 应可读写, 实际: {out:?}"
        );
        let _ = child.child.try_wait();
    }

    // ── reader_only ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn reader_only_streams_output() {
        let child = spawn_pty("echo", &["only-reader"], 40, 12).expect("spawn echo");
        let mut reader = reader_only(&child.master).expect("reader_only");

        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                chunk = reader.read_chunk() => match chunk {
                    Some(Ok(data)) => all.extend_from_slice(&data),
                    Some(Err(_)) | None => break,
                }
            }
            if String::from_utf8_lossy(&all).contains("only-reader") {
                break;
            }
        }
        let out = String::from_utf8_lossy(&all);
        assert!(
            out.contains("only-reader"),
            "reader_only 应能读到输出, 实际: {out:?}"
        );
    }

    /// 阻塞写必须在时限内失败返回，避免 sender/shutdown 永久挂起。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_all_times_out_when_blocked() {
        struct BlockingWrite;
        impl Write for BlockingWrite {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                // 略长于 write_all 的 2s 超时；勿睡太久以免拖慢整 suite 收尾
                std::thread::sleep(Duration::from_millis(3500));
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let writer = PtyWriter::new(Box::new(BlockingWrite));
        let start = std::time::Instant::now();
        let err = writer
            .write_all(b"block".to_vec())
            .await
            .expect_err("阻塞写应超时");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "超时应约 2s，实际 {:?}",
            start.elapsed()
        );
        // 等阻塞线程结束，避免 runtime drop 时再挂数秒
        tokio::time::sleep(Duration::from_millis(2000)).await;
    }
}
