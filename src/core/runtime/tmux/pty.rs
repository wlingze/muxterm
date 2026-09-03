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
use std::io::{ErrorKind, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// 已 spawn 的 tmux pty 子进程。
pub struct PtyChild {
    /// master 端（用于 take_writer 写命令、管理 child wait）。
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    /// 子进程句柄（用于 kill / wait）。
    pub child: Box<dyn portable_pty::Child + Send>,
}

impl PtyChild {
    /// 杀 control client 并在时限内回收。禁止 `child.wait()`：portable-pty
    /// 在 tmux -CC 未随 SIGTERM 退出时会无限堵死 tokio worker，进而让
    /// PersistDetach 之后的重新 attach 永远等不到空闲运行时。
    pub fn kill_and_wait(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let pid = self.child.process_id();
        let _ = self.child.kill();
        if wait_exited(&mut *self.child, Duration::from_millis(400)) {
            return;
        }
        if let Some(pid) = pid {
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        let _ = wait_exited(&mut *self.child, Duration::from_millis(200));
    }
}

fn wait_exited(child: &mut (dyn portable_pty::Child + Send), budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
    }
}

/// 本 GUI 客户端对 tmux/ssh pty 声明的终端环境。
///
/// 桌面启动和 `ssh mac` 经常没有 TERM，或 LANG=C：tmux 会把 24-bit
/// 颜色收成 16 色、把中文收成 `_`。这些变量只作用在控制客户端进程，
/// 不改用户已有 pane 里的 shell。
pub const CLIENT_TERM: &str = "xterm-256color";
pub const CLIENT_COLORTERM: &str = "truecolor";
pub const CLIENT_UTF8_LOCALE: &str = "en_US.UTF-8";

pub fn client_terminal_env() -> [(&'static str, &'static str); 5] {
    [
        ("LANG", CLIENT_UTF8_LOCALE),
        ("LC_CTYPE", CLIENT_UTF8_LOCALE),
        ("LC_ALL", CLIENT_UTF8_LOCALE),
        ("TERM", CLIENT_TERM),
        ("COLORTERM", CLIENT_COLORTERM),
    ]
}

pub fn apply_client_terminal_env(cmd: &mut CommandBuilder) {
    for (key, value) in client_terminal_env() {
        cmd.env(key, value);
    }
}

/// 兼容旧名。
pub fn apply_truecolor_env(cmd: &mut CommandBuilder) {
    apply_client_terminal_env(cmd);
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
    apply_truecolor_env(&mut cmd);
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
    /// 由已有的 mpsc Receiver 构造（用于 SSH transport 桥接）。
    pub fn from_channel(rx: mpsc::Receiver<std::io::Result<Vec<u8>>>) -> Self {
        Self { rx }
    }

    /// 由 master 的 reader 构造，立即启动后台阻塞读线程。
    pub fn new(mut reader: Box<dyn Read + Send>) -> Self {
        let (tx, rx) = mpsc::channel::<std::io::Result<Vec<u8>>>(4096);
        std::thread::Builder::new()
            .name("muxterm-pty-read".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    if tx.is_closed() {
                        break;
                    }
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.blocking_send(Ok(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
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
    /// 上行字节计数（SSH 模式与 transport 共享）。
    traffic: Option<crate::core::transport::TrafficCounters>,
}

impl PtyWriter {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
            traffic: None,
        }
    }

    /// 带共享流量计数构造（SSH 模式）。
    pub fn with_traffic(
        writer: Box<dyn Write + Send>,
        traffic: crate::core::transport::TrafficCounters,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
            traffic: Some(traffic),
        }
    }

    /// 异步写：把数据克隆后丢到阻塞线程池写；超时返回 TimedOut，便于 shutdown 推进。
    pub async fn write_all(&self, data: Vec<u8>) -> std::io::Result<()> {
        let inner = self.inner.clone();
        let traffic = self.traffic.clone();
        let write_fut = tokio::task::spawn_blocking(move || {
            let mut w = inner.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut written = 0;
            while written < data.len() {
                match w.write(&data[written..]) {
                    Ok(0) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::WriteZero,
                            "pty writer returned zero bytes",
                        ));
                    }
                    Ok(n) => written += n,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "pty write timeout (2s)",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => return Err(e),
                }
            }
            if let Some(t) = &traffic {
                t.add_up(written as u64);
            }
            Ok(())
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
    set_nonblocking(&**master)?;
    let reader = master.try_clone_reader().context("try_clone_reader 失败")?;
    let writer = master.take_writer().context("take_writer 失败")?;
    Ok((PtyReader::new(reader), PtyWriter::new(writer)))
}

/// 单独构造 reader（用 try_clone_reader），保留 master 自己持有 writer。
#[allow(dead_code)]
#[allow(clippy::borrowed_box)]
pub fn reader_only(master: &Box<dyn portable_pty::MasterPty + Send>) -> Result<PtyReader> {
    set_nonblocking(&**master)?;
    let reader = master.try_clone_reader().context("try_clone_reader 失败")?;
    Ok(PtyReader::new(reader))
}

fn set_nonblocking(master: &(dyn portable_pty::MasterPty + Send)) -> Result<()> {
    #[cfg(unix)]
    {
        let Some(fd) = master.as_raw_fd() else {
            return Ok(());
        };
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error()).context("读取 pty flags 失败");
        }
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error()).context("设置 pty 非阻塞失败");
        }
    }
    #[cfg(not(unix))]
    let _ = master;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── spawn_pty ────────────────────────────────────────────────────────

    #[test]
    fn spawn_pty_advertises_truecolor_terminal() {
        let mut child = spawn_pty(
            "sh",
            &[
                "-c",
                "printf '%s\\n' \"$TERM\" \"$COLORTERM\" \"$LANG\" \"$LC_ALL\" \"$LC_CTYPE\"",
            ],
            40,
            12,
        )
        .expect("spawn shell");
        set_nonblocking(&*child.master).expect("set_nonblocking");
        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let mut reader = reader;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    let out = String::from_utf8_lossy(&buf);
                    if out.contains("xterm-256color")
                        && out.contains("truecolor")
                        && out.contains("en_US.UTF-8")
                    {
                        break;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
            if std::time::Instant::now() > deadline {
                break;
            }
        }
        let out = String::from_utf8_lossy(&buf);
        let values: Vec<_> = out.lines().map(str::trim).collect();
        assert_eq!(
            values,
            vec![
                CLIENT_TERM,
                CLIENT_COLORTERM,
                CLIENT_UTF8_LOCALE,
                CLIENT_UTF8_LOCALE,
                CLIENT_UTF8_LOCALE,
            ],
            "pty 子进程环境变量应由控制客户端显式设置, 实际: {out:?}"
        );
        let _ = child.child.try_wait();
    }

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
    fn kill_and_wait_returns_before_child_natural_exit() {
        let mut child = spawn_pty("sleep", &["30"], 10, 5).expect("spawn sleep");
        let start = std::time::Instant::now();
        child.kill_and_wait();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "kill_and_wait 不得阻塞到 sleep 自然结束，实际 {:?}",
            start.elapsed()
        );
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
        // 让 slave 短暂保持打开，避免 macOS 在子进程退出与 reader 线程
        // 尚未开始读取之间把最后一个 PTY 缓冲块表现为 EIO，导致测试偶发
        // 读到空串；真实 tmux 连接本身是长生命周期的。
        let mut child =
            spawn_pty("sh", &["-c", "printf stream-test; sleep 0.1"], 40, 12).expect("spawn shell");
        let reader = child.master.try_clone_reader().expect("try_clone_reader");
        let mut reader = PtyReader::new(reader);

        // 收集所有块直到 EOF 或超时。
        // 默认并行下多个测试同时 spawn 子进程，进程调度/pty 投递可能变慢；
        // 用宽松的有界超时（10s）避免 flaky，同时保证不会无限等待。
        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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
        // 兜底回收子进程，避免残留 echo 进程占用资源
        let _ = child.child.try_wait();
        if child.child.try_wait().ok().flatten().is_none() {
            let _ = child.child.kill();
        }
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

    /// E4：PtyWriter 带共享计数时，write_all 累加 up 字节。
    #[tokio::test]
    async fn pty_writer_with_traffic_counts_up_bytes() {
        let mut child = spawn_pty("cat", &[], 40, 12).expect("spawn cat");
        let writer = child.master.take_writer().expect("take_writer");
        let traffic = crate::core::transport::TrafficCounters::new();
        let writer = PtyWriter::with_traffic(writer, traffic.clone());

        writer.write_all(b"abc".to_vec()).await.expect("write_all");
        assert_eq!(traffic.snapshot(), (0, 3), "up 应累加 3 字节");
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
        // 让 slave 短暂保持打开，避免子进程在 reader 线程开始读取前就退出，
        // 把最后一个 PTY 缓冲块表现为 EIO/空读（默认并行下偶发
        // `reader_only 应能读到输出, 实际: ""`）。与 pty_reader_streams_chunks 同模式。
        let mut child =
            spawn_pty("sh", &["-c", "printf only-reader; sleep 0.1"], 40, 12).expect("spawn shell");
        let mut reader = reader_only(&child.master).expect("reader_only");

        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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
        // 兜底回收子进程，避免残留 shell 占用资源。
        let _ = child.child.try_wait();
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
