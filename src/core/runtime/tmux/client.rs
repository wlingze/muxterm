#![allow(clippy::while_let_loop)]
//! 异步 tmux `-CC` 客户端。
//!
//! 封装与 `tmux -CC` 子进程的通信：
//!
//! - [`TmuxClient::spawn`]：spawn tmux 子进程到一对 pty（tmux 在 `-CC` 模式下
//!   仍需要 tty，否则 `tcgetattr failed` 立即退出）。
//! - 后台 task 读 pty master 端，按**真换行**切行（`%output` content 里的 `\n`
//!   是 C 转义后的两个字符，不是真换行），逐行喂给
//!   [`parse_line_bytes`](super::protocol::parse_line_bytes)，产出 `Message` 事件流。
//! - [`TmuxClientHandle::send_command`]：把命令字符串写到 tmux stdin（pty）。
//! - 通过 [`tokio::sync::mpsc`] 输出 `TmuxEvent` 事件，命令响应正文行（夹在
//!   `%begin`/`%end` 之间的普通行）以 `ResponseLine` 形式分发。
//! - 优雅关闭：`detach` / `kill`。
//!
//! 半行 buffer 处理：tmux 一次 write 到 pty 可能只写半行，必须按真换行符
//! （`\n`，tmux 实际用 `\r\n`）切包，把不完整的尾段留到下次。

use super::command::TmuxCommand;
use super::protocol::{parse_line_bytes, Message, NotificationKind};
use super::pty::{self, split_master, PtyChild, PtyReader, PtyWriter};
use crate::core::buffer_cap::{trim_incomplete_line, MAX_INCOMPLETE_LINE_BYTES};
use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use tokio::process::{Child, ChildStdout};
use tokio::sync::mpsc;

/// 客户端连接模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectMode {
    /// `tmux -CC new-session`（创建新 session）。
    NewSession {
        name: Option<String>,
        /// 起始工作目录（`-c <dir>`）；None 用 tmux 默认（当前目录）。
        start_directory: Option<String>,
    },
    /// `tmux -CC attach -t <target>`（附加已有 session）。
    Attach { target: Option<String> },
}

impl Default for ConnectMode {
    fn default() -> Self {
        ConnectMode::NewSession {
            name: None,
            start_directory: None,
        }
    }
}

/// 启动参数。
#[derive(Debug, Clone, Default)]
pub struct TmuxClientConfig {
    /// 连接模式，默认 NewSession。
    pub mode: Option<ConnectMode>,
    /// 额外传给 tmux 的参数（如 `-L socket_name`）。
    pub extra_args: Vec<String>,
    /// tmux 可执行路径，默认 `tmux`。
    pub tmux_bin: Option<String>,
    /// 初始窗口几何 -x。
    pub cols: Option<u32>,
    /// 初始窗口几何 -y。
    pub rows: Option<u32>,
    /// 事件通道容量。
    pub event_buffer: usize,
    /// SSH alias：非空时通过 SSH transport 在远端启动 tmux -CC。
    pub ssh_alias: Option<String>,
}

/// tmux `-CC` 客户端工厂。
///
/// 仅提供静态构造方法，真正的句柄是 [`TmuxClientHandle`]。
pub struct TmuxClient;

/// tmux -CC 客户端句柄。
pub struct TmuxClientHandle {
    /// pty 写端（pty 模式）。
    pty_writer: Option<PtyWriter>,
    /// 直 spawn 模式的 stdin。
    stdin: Option<tokio::process::ChildStdin>,
    /// 直 spawn 模式的子进程。
    child: Option<Child>,
    /// pty 模式的子进程。
    pty_child: Option<PtyChild>,
    /// SSH 读写字节计数（本地 pty 模式为 None）。
    pub traffic: Option<crate::core::transport::TrafficCounters>,
}

/// 事件：tmux → 客户端。
#[derive(Debug, Clone)]
pub enum TmuxEvent {
    /// 一条已解析的通知消息。
    Message(Message),
    /// 命令响应正文行（夹在 %begin/%end 或 %begin/%error 之间，不带 % 前缀）。
    ResponseLine {
        number: i64,
        is_error: bool,
        line: String,
    },
    /// tmux 子进程退出。
    Exit { code: Option<i32> },
}

impl TmuxClient {
    /// spawn 一个 tmux -CC 进程并启动后台读循环。
    ///
    /// 默认走 pty 模式（tmux -CC 需要 tty）。
    pub async fn spawn(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, mpsc::Receiver<TmuxEvent>)> {
        if config.ssh_alias.is_some() {
            Self::spawn_ssh(config).await
        } else {
            Self::spawn_pty(config).await
        }
    }

    /// SSH 模式：通过 SSH transport 在远端启动 `tmux -CC`。
    ///
    /// 用 `SshProcessTransport` spawn `ssh <alias> -- tmux -CC ...`，
    /// 取其 reader/writer 替代本地 pty。读循环与本地 pty 模式相同。
    pub async fn spawn_ssh(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, mpsc::Receiver<TmuxEvent>)> {
        use crate::core::transport::ssh::{build_ssh_command, SshProcessTransport};
        use crate::core::transport::Transport;

        let alias = config
            .ssh_alias
            .as_ref()
            .ok_or_else(|| anyhow!("spawn_ssh 需要 ssh_alias"))?;

        // 构造远端 tmux 命令字符串。
        // 注意 argv 开头是 `-L socket` 等二进制级选项；远端经 shell 执行，必须
        // 以 `tmux` 开头（否则 shell 会把 `-L ...` 当成 shell 自身选项报错）。
        let argv = build_argv(&config);
        let mut full = vec!["tmux".to_string()];
        full.extend_from_slice(&argv);
        let remote_tmux = full.join(" ");
        // 复用 CLI 的 MUXTERM_SSH_CONFIG_PATH 约定：显式 -F 指定 ssh config
        // （测试/CI 用生成 config，不读用户真实 ~/.ssh/config）。
        let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
        let (program, ssh_args) = build_ssh_command(alias, &remote_tmux, ssh_config.as_deref());
        let arg_refs: Vec<&str> = ssh_args.iter().map(|s| s.as_str()).collect();

        let pty_size = crate::core::transport::PtySize::new(
            config.cols.unwrap_or(80) as u16,
            config.rows.unwrap_or(24) as u16,
        );

        tracing::info!(
            target = "muxterm::client",
            alias = %alias,
            remote = %remote_tmux,
            "spawn tmux -CC via SSH"
        );

        let traffic = crate::core::transport::TrafficCounters::new();
        let mut transport = SshProcessTransport::new();
        transport.set_traffic(traffic.clone());
        transport
            .spawn_exec(&program, &arg_refs, pty_size)
            .context("SSH transport spawn 失败")?;

        // 先取 writer（take_pty_writer 消费 master 的 writer 端）
        let writer = transport
            .take_pty_writer()
            .context("SSH transport take_writer 失败")?;
        let writer = PtyWriter::with_traffic(writer, traffic.clone());

        // 再把 transport 移入读线程（read 是非阻塞，用后台线程桥接到 mpsc）
        let (read_tx, read_rx) = mpsc::channel::<std::io::Result<Vec<u8>>>(4096);
        let mut read_transport = transport;
        std::thread::Builder::new()
            .name("muxterm-ssh-read".into())
            .spawn(move || loop {
                match read_transport.read() {
                    Ok(Some(data)) => {
                        if read_tx.blocking_send(Ok(data)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            })
            .expect("spawn ssh read thread");

        // 用 PtyReader::from_channel 包装 read_rx，复用 read_pty_loop
        let reader = PtyReader::from_channel(read_rx);
        let (tx, rx) = mpsc::channel(config.event_buffer.max(32));
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            read_pty_loop(reader, tx_clone).await;
        });

        let handle = TmuxClientHandle {
            pty_writer: Some(writer),
            stdin: None,
            child: None,
            pty_child: None,
            traffic: Some(traffic),
        };
        Ok((handle, rx))
    }

    /// pty 模式 spawn（推荐，tmux -CC 需要 tty）。
    pub async fn spawn_pty(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, mpsc::Receiver<TmuxEvent>)> {
        let bin = config
            .tmux_bin
            .clone()
            .unwrap_or_else(crate::core::executable::resolve_tmux_binary);
        let argv = build_argv(&config);
        let arg_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let cols = config.cols.unwrap_or(80) as u16;
        let rows = config.rows.unwrap_or(24) as u16;
        tracing::info!(target = "muxterm::client", bin = %bin, args = ?argv, "spawn tmux -CC (pty)");
        let mut pty_child = pty::spawn_pty(&bin, &arg_refs, cols, rows)
            .with_context(|| format!("spawn tmux 失败: {bin}"))?;

        let (reader, writer) = split_master(&mut pty_child.master)?;
        let (tx, rx) = mpsc::channel(config.event_buffer.max(32));
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            read_pty_loop(reader, tx_clone).await;
        });

        let handle = TmuxClientHandle {
            pty_writer: Some(writer),
            stdin: None,
            child: None,
            pty_child: Some(pty_child),
            traffic: None,
        };
        // 配置消费标记（避免未使用）
        let _ = config.extra_args.len();
        Ok((handle, rx))
    }

    /// 直 spawn 模式（不用 pty）。tmux 在无 tty 下通常会立即退出，仅作兜底/测试。
    pub async fn spawn_direct(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, mpsc::Receiver<TmuxEvent>)> {
        let bin = config
            .tmux_bin
            .clone()
            .unwrap_or_else(crate::core::executable::resolve_tmux_binary);
        let mut cmd = tokio::process::Command::new(&bin);
        let argv = build_argv(&config);
        for a in &argv {
            cmd.arg(a);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        tracing::info!(target = "muxterm::client", bin = %bin, "spawn tmux -CC (direct)");
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn tmux 失败: {bin}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("tmux 没有 stdout"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("tmux 没有 stdin"))?;

        let (tx, rx) = mpsc::channel(config.event_buffer.max(32));
        let tx_clone = tx.clone();
        tokio::spawn(read_stream_loop(stdout, tx_clone));

        let handle = TmuxClientHandle {
            pty_writer: None,
            stdin: Some(stdin),
            child: Some(child),
            pty_child: None,
            traffic: None,
        };
        Ok((handle, rx))
    }

    /// 便捷：new-session。
    pub async fn new_session(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, mpsc::Receiver<TmuxEvent>)> {
        Self::spawn(config).await
    }

    /// 便捷：attach。
    pub async fn attach(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, mpsc::Receiver<TmuxEvent>)> {
        Self::spawn(config).await
    }
}

/// 构造 tmux 命令参数（不含 bin）。
///
/// 构造传给 tmux 可执行文件的 argv（不含二进制名本身）。
///
/// 顺序：`[extra_args] -CC <command> <command args>`。extra_args（如 `-L socket`）
/// 是 tmux **二进制级**选项，必须放在 `-CC` 之前；`-CC` 之后的第一个 token 才是
/// tmux 命令（new-session/attach），否则会被 tmux 当成命令参数解析报错。
pub(crate) fn build_argv(config: &TmuxClientConfig) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    // 二进制级选项（如 -L socket_name）放在 -CC 前
    for a in &config.extra_args {
        argv.push(a.clone());
    }
    argv.push("-CC".into());
    match config.mode.clone().unwrap_or_default() {
        ConnectMode::NewSession {
            name,
            start_directory,
        } => {
            argv.push("new-session".into());
            if let Some(n) = &name {
                argv.push("-s".into());
                argv.push(n.clone());
            }
            if let Some(dir) = &start_directory {
                argv.push("-c".into());
                argv.push(dir.clone());
            }
            if let Some(x) = config.cols {
                argv.push("-x".into());
                argv.push(x.to_string());
            }
            if let Some(y) = config.rows {
                argv.push("-y".into());
                argv.push(y.to_string());
            }
        }
        ConnectMode::Attach { target } => {
            argv.push("attach".into());
            if let Some(t) = &target {
                argv.push("-t".into());
                argv.push(t.clone());
            }
        }
    }
    argv
}

impl TmuxClientHandle {
    /// 发送一条已构造好的命令。
    pub async fn send_command(&mut self, cmd: &TmuxCommand) -> Result<()> {
        let line = cmd.to_line();
        self.send_raw(&line).await
    }

    /// 发送任意原始文本到 tmux stdin（应自带末尾换行）。
    pub async fn send_raw(&mut self, raw: &str) -> Result<()> {
        tracing::debug!(target = "muxterm::client", "send: {:?}", raw);
        if let Some(w) = &self.pty_writer {
            w.write_all(raw.as_bytes().to_vec())
                .await
                .map_err(|e| anyhow!("写 tmux pty 失败: {e}"))?;
            return Ok(());
        }
        if let Some(stdin) = &mut self.stdin {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(raw.as_bytes())
                .await
                .map_err(|e| anyhow!("写 tmux stdin 失败: {e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| anyhow!("flush tmux stdin 失败: {e}"))?;
            return Ok(());
        }
        Err(anyhow!("tmux 写端已关闭"))
    }

    /// 优雅 detach：发 `detach-client`，tmux 会输出 %exit 然后进程退出。
    pub async fn detach(&mut self) -> Result<()> {
        self.send_raw("detach-client\n").await
    }

    /// 强制 kill：先关写端并杀子进程。
    ///
    /// 不先 `detach`：pty 写可能阻塞，导致 sender task / `shutdown` 永远等不到。
    pub async fn kill(&mut self) -> Result<()> {
        self.close_writer();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Some(mut pty_child) = self.pty_child.take() {
            pty_child.kill_and_wait();
        }
        Ok(())
    }

    /// 等待子进程退出，返回退出码。
    pub async fn wait(mut self) -> Result<Option<i32>> {
        self.close_writer();
        if let Some(mut child) = self.child {
            let status = child.wait().await.context("等待 tmux 退出失败")?;
            Ok(status.code())
        } else if let Some(mut pty_child) = self.pty_child {
            pty_child
                .child
                .wait()
                .map(|s| s.success().then_some(0).or(None))
                .map_err(|e| anyhow!("等待 tmux 退出失败: {e}"))
        } else {
            Ok(None)
        }
    }

    /// 关闭写端（用于 detach 后让 tmux 自然退出）。
    pub fn close_writer(&mut self) {
        self.pty_writer.take();
        self.stdin.take();
    }
}

// ============================================================================
// 读循环
// ============================================================================

/// 把字节块渲染成可读的 debug 字符串：可打印 ASCII/UTF-8 保留，控制字节转义。
///
/// 用于 debug 模式把 tmux 原始收发数据落盘，方便排查渲染/输入问题；内容过大时
/// 截断，避免日志刷屏。
fn hex_debug(bytes: &[u8]) -> String {
    const MAX: usize = 1024;
    let show = if bytes.len() > MAX {
        &bytes[..MAX]
    } else {
        bytes
    };
    let mut out = String::with_capacity(show.len() + 16);
    for &b in show {
        match b {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x1b => out.push_str("\\e"),
            0x20..=0x7e => out.push(b as char),
            other => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{other:02x}");
            }
        }
    }
    if bytes.len() > MAX {
        out.push_str("...(truncated)");
    }
    out
}

/// DCS passthrough 前缀：tmux 3.3+ 在 CC 模式下用 `ESC P 1 0 0 0 p` 把第一条
/// `%begin` 包起来。我们识别并剥离它。
pub(crate) const DCS_PREFIX: &[u8] = b"\x1bP1000p";

/// 从 pty/stdout 读到的字节块中提取「完整行」。
///
/// 这是 read loop 的核心纯逻辑，抽出来便于单元测试（分包/拼包/长行/UTF-8 边界）。
/// 它把 `chunk` 追加进 `buf`，按**真换行** `\n` 切出完整行（去掉行尾 `\n` 与
/// `\r`，并剥离 DCS 前缀），返回提取出的行；未闭合的尾段留在 `buf` 等下次。
/// 若 `buf` 过长仍无换行，会丢弃最旧前缀（见 [`trim_incomplete_line`]）。
///
/// 注意：`%output` content 里的 `\n` 是 C 转义后的两个字符（`\\` + `n`），
/// 不是真换行符，所以这里只按真 `\n` 字节切，不会把 content 内部切碎。
pub(crate) fn feed_bytes_to_lines(buf: &mut Vec<u8>, chunk: &[u8]) -> Vec<Vec<u8>> {
    if chunk.is_empty() {
        return Vec::new();
    }
    buf.extend_from_slice(chunk);
    let mut lines = Vec::new();
    loop {
        let Some(nl) = buf.iter().position(|&b| b == b'\n') else {
            // 只限制仍未闭合的尾段；同一 chunk 中已经闭合的完整行不能被
            // 大小限制提前丢掉。
            trim_incomplete_line(buf, MAX_INCOMPLETE_LINE_BYTES);
            break;
        };
        let mut line_bytes: Vec<u8> = buf.drain(..=nl).collect();
        if line_bytes.last() == Some(&b'\n') {
            line_bytes.pop();
        }
        if line_bytes.last() == Some(&b'\r') {
            line_bytes.pop();
        }
        // 完整行也不能让单次输入绕过上限；保留最近的字节，明确表示这是一条
        // 被截断的行（通常会因此无法作为协议通知解析），但不会无限增长。
        if line_bytes.len() > MAX_INCOMPLETE_LINE_BYTES {
            let start = line_bytes.len() - MAX_INCOMPLETE_LINE_BYTES;
            line_bytes.drain(..start);
        }
        if line_bytes.starts_with(DCS_PREFIX) {
            line_bytes.drain(..DCS_PREFIX.len());
        }
        lines.push(line_bytes);
    }
    lines
}

/// pty 模式读循环：用 `PtyReader::read_chunk` 异步取字节块，按真换行切行。
async fn read_pty_loop(mut reader: PtyReader, tx: mpsc::Sender<TmuxEvent>) {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut in_response = false;
    let mut current_number: i64 = 0;
    let mut current_is_error = false;

    while let Some(chunk_res) = reader.read_chunk().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(target = "muxterm::client", "读 tmux pty 失败: {e}");
                break;
            }
        };
        if chunk.is_empty() {
            continue;
        }
        tracing::trace!(
            target = "muxterm::client",
            len = chunk.len(),
            hex = %hex_debug(&chunk),
            "recv tmux chunk"
        );
        for line in feed_bytes_to_lines(&mut buf, &chunk) {
            tracing::trace!(
                target = "muxterm::client",
                line = %String::from_utf8_lossy(&line),
                "recv line"
            );
            process_line(
                &line,
                &tx,
                &mut in_response,
                &mut current_number,
                &mut current_is_error,
            )
            .await;
        }
    }
    tracing::info!(target = "muxterm::client", "tmux pty EOF");
    let _ = tx.send(TmuxEvent::Exit { code: None }).await;
}

/// 直 spawn 模式读循环：ChildStdout 是 AsyncRead。
async fn read_stream_loop(stdout: ChildStdout, tx: mpsc::Sender<TmuxEvent>) {
    use tokio::io::AsyncReadExt;
    let mut reader = stdout;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let mut in_response = false;
    let mut current_number: i64 = 0;
    let mut current_is_error = false;

    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                for line in feed_bytes_to_lines(&mut buf, &chunk[..n]) {
                    process_line(
                        &line,
                        &tx,
                        &mut in_response,
                        &mut current_number,
                        &mut current_is_error,
                    )
                    .await;
                }
            }
            Err(e) => {
                tracing::error!(target = "muxterm::client", "读 tmux stdout 失败: {e}");
                break;
            }
        }
    }
    tracing::info!(target = "muxterm::client", "tmux stdout EOF");
    let _ = tx.send(TmuxEvent::Exit { code: None }).await;
}

/// 处理单行：解析为 Message 或 ResponseLine，并维护响应状态机。
///
/// `pub(crate)`：SSH 远程 client 复用同一套行状态机。
pub(crate) async fn process_line(
    line: &[u8],
    tx: &mpsc::Sender<TmuxEvent>,
    in_response: &mut bool,
    current_number: &mut i64,
    current_is_error: &mut bool,
) {
    if let Some(Message::Unknown { .. }) = parse_line_bytes(line) {
        // 命令响应正文可以合法地以 `%` 开头（例如 list-panes 的 `%0 @0 0`），
        // 只有在响应边界内才把这种未知通知形状还原为 ResponseLine。
        if *in_response {
            let line = String::from_utf8_lossy(line).into_owned();
            let _ = tx
                .send(TmuxEvent::ResponseLine {
                    number: *current_number,
                    is_error: *current_is_error,
                    line,
                })
                .await;
            return;
        }
    }

    if let Some(msg) = parse_line_bytes(line) {
        if let Message::ResponseBoundary(b) = &msg {
            match b.kind {
                NotificationKind::Begin => {
                    *in_response = true;
                    *current_number = b.number;
                    *current_is_error = false;
                }
                NotificationKind::End => {
                    *in_response = false;
                }
                NotificationKind::Error => {
                    *current_is_error = true;
                    *in_response = false;
                }
            }
        }
        let _ = tx.send(TmuxEvent::Message(msg)).await;
    } else if line.is_empty() {
        if *in_response {
            let _ = tx
                .send(TmuxEvent::ResponseLine {
                    number: *current_number,
                    is_error: *current_is_error,
                    line: String::new(),
                })
                .await;
        }
    } else if *in_response {
        let line = String::from_utf8_lossy(line).into_owned();
        let _ = tx
            .send(TmuxEvent::ResponseLine {
                number: *current_number,
                is_error: *current_is_error,
                line,
            })
            .await;
    } else {
        tracing::trace!(
            target = "muxterm::client",
            line = %String::from_utf8_lossy(line),
            "响应外普通行被忽略"
        );
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 每次调用递增的全局计数器，保证同一进程内并行线程拿到的测试 socket 名唯一。
    /// 旧实现只用了 `std::process::id()`，在默认并行下多个真实 tmux E2E 会共用
    /// 同一个 `-L` socket，互相 kill-server 导致 CI 卡死（end_to_end_real_tmux 30m 超时）。
    static TEST_SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_socket(prefix: &str) -> String {
        let n = TEST_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{}-{n}", std::process::id())
    }

    /// 清理指定 tmux server；与测试体解耦，保证即使测试体在 15s 超时被取消，
    /// 残留的 tmux server 也能被回收，避免污染下一次 CI 运行。
    /// 接收 owned `String`，使返回的 future 拥有其数据，可被 `Fn` 闭包安全返回。
    async fn kill_tmux_server(socket: String) {
        let _ = tokio::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output()
            .await;
    }

    /// 在一个 15s 有界 tokio timeout 内运行闭包；无论闭包成功、panic 还是超时，
    /// 返回前都会调用 cleanup 清理资源。用于把 `spawn/send/kill` 包进有界窗口，
    /// 防止 `handle.kill().await` 在 Linux PTY 上无限阻塞导致整个测试挂死。
    /// `BodyFut` 与 `CleanupFut` 是独立泛型，避免二者 future 类型耦合。
    async fn run_bounded<B, Cleanup, BodyFut, CleanupFut, T>(
        timeout_s: u64,
        fut: B,
        cleanup: &Cleanup,
    ) -> std::result::Result<T, ()>
    where
        B: FnOnce() -> BodyFut,
        BodyFut: std::future::Future<Output = T>,
        Cleanup: Fn() -> CleanupFut,
        CleanupFut: std::future::Future<Output = ()>,
    {
        let res =
            match tokio::time::timeout(tokio::time::Duration::from_secs(timeout_s), fut()).await {
                Ok(t) => Ok(t),
                Err(_) => Err(()),
            };
        cleanup().await;
        res
    }

    #[tokio::test]
    async fn process_line_dcs_and_response() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut in_resp = false;
        let mut num = 0i64;
        let mut is_err = false;

        process_line(
            b"\x1bP1000p%begin 1784356613 286 1",
            &tx,
            &mut in_resp,
            &mut num,
            &mut is_err,
        )
        .await;
        assert!(in_resp);
        assert_eq!(num, 286);

        process_line(
            b"cmd: 1 windows (created ...)",
            &tx,
            &mut in_resp,
            &mut num,
            &mut is_err,
        )
        .await;
        process_line(
            b"%end 1784356613 286 1",
            &tx,
            &mut in_resp,
            &mut num,
            &mut is_err,
        )
        .await;
        assert!(!in_resp);

        let mut msgs = Vec::new();
        let mut lines = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                TmuxEvent::Message(m) => msgs.push(m),
                TmuxEvent::ResponseLine { line, .. } => lines.push(line),
                _ => {}
            }
        }
        assert!(msgs
            .iter()
            .any(|m| matches!(m, Message::ResponseBoundary(_))));
        assert!(lines.contains(&"cmd: 1 windows (created ...)".to_string()));
    }

    #[tokio::test]
    async fn process_line_output_with_escaped_newline() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut in_resp = false;
        let mut num = 0;
        let mut is_err = false;
        process_line(
            br#"%output %0 a\nb"#,
            &tx,
            &mut in_resp,
            &mut num,
            &mut is_err,
        )
        .await;
        let ev = rx.recv().await.unwrap();
        match ev {
            TmuxEvent::Message(Message::Output { content, .. }) => {
                assert_eq!(&content, b"a\nb");
            }
            _ => panic!("应为 Output"),
        }
    }

    #[tokio::test]
    async fn process_line_error_boundary_closes_response() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut in_resp = false;
        let mut num = 0;
        let mut is_err = false;
        process_line(b"%begin 1 5 0", &tx, &mut in_resp, &mut num, &mut is_err).await;
        assert!(in_resp);
        process_line(b"some error text", &tx, &mut in_resp, &mut num, &mut is_err).await;
        process_line(b"%error 1 5 0", &tx, &mut in_resp, &mut num, &mut is_err).await;
        assert!(!in_resp);
        let mut any_lines = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let TmuxEvent::ResponseLine { line, .. } = ev {
                any_lines.push(line);
            }
        }
        assert!(any_lines.contains(&"some error text".to_string()));
    }

    #[tokio::test]
    async fn process_line_keeps_percent_prefixed_response_rows_as_response_lines() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut in_resp = false;
        let mut number = 0;
        let mut is_err = false;

        process_line(b"%begin 1 7 0", &tx, &mut in_resp, &mut number, &mut is_err).await;
        process_line(b"%0 @0 0", &tx, &mut in_resp, &mut number, &mut is_err).await;
        process_line(b"%end 1 7 0", &tx, &mut in_resp, &mut number, &mut is_err).await;

        let mut response_rows = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let TmuxEvent::ResponseLine {
                number: 7, line, ..
            } = event
            {
                response_rows.push(line);
            }
        }
        assert_eq!(response_rows, vec!["%0 @0 0"]);
    }

    /// 端到端：真实 spawn tmux -CC（pty），收事件，发命令，验证响应。
    ///
    /// 隔离回归：socket 名用 PID+Atomic 计数器保证唯一（旧实现只用 PID，
    /// 默认并行下两个真实 tmux E2E 共用同一 `-L` socket，互相 kill-server
    /// 会卡死 CI——end_to_end_real_tmux 曾 30 分钟超时）。整个测试体包在
    /// 15s 有界 timeout 内；无论成功/panic/超时，timeout 之外都执行
    /// `tmux -L <socket> kill-server` 回收残留，杜绝 `handle.kill().await`
    /// 在 Linux PTY 上无限阻塞导致 kill-server 永远执行不到。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_real_tmux() {
        let socket = unique_test_socket("muxterm-test");
        let cleanup_socket = socket.clone();
        let result = run_bounded(
            15,
            || async {
                let config = TmuxClientConfig {
                    mode: Some(ConnectMode::NewSession {
                        name: Some("mxtest".into()),
                        start_directory: None,
                    }),
                    extra_args: vec!["-L".into(), socket.clone()],
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                };
                let (mut handle, mut rx) = TmuxClient::spawn(config)
                    .await
                    .expect("end_to_end_real_tmux 应能启动 tmux");

                let mut got_window_add = false;
                let mut got_session_changed = false;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(4);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::Message(m)) => match &m {
                                Message::WindowAdd { .. } => got_window_add = true,
                                Message::SessionChanged { .. } => got_session_changed = true,
                                _ => {}
                            },
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                    if got_window_add && got_session_changed {
                        break;
                    }
                }
                assert!(got_window_add, "应收到 window-add");

                let cmd = super::super::command::display_message(
                    super::super::command::PaneId(0),
                    "#{session_name}",
                );
                handle.send_command(&cmd).await.unwrap();

                let mut got_response = false;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::ResponseLine { line, .. }) => {
                                if line.trim() == "mxtest" { got_response = true; break; }
                            }
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                }
                assert!(got_response, "display-message 应返回 mxtest");

                let _ = handle.kill().await;
                let _ = got_session_changed;
            },
            &move || kill_tmux_server(cleanup_socket.clone()),
        )
        .await;
        // 无论成功或超时，都已完成 kill-server 清理。
        assert!(result.is_ok(), "end_to_end_real_tmux 应在 15s 内完成");
    }

    /// 端到端（P0）：detach 后 tmux 应输出 `%exit`，验证程序退出 → %exit 顺序。
    ///
    /// 与其他真实 tmux E2E 一样使用唯一 socket + 15s 有界 timeout +
    /// timeout 之外 kill-server，防止并行 socket 冲突与 PTY kill 阻塞。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_detach_yields_exit_event() {
        let socket = unique_test_socket("muxterm-exit");
        let cleanup_socket = socket.clone();
        let result = run_bounded(
            15,
            || async {
                let config = TmuxClientConfig {
                    mode: Some(ConnectMode::NewSession {
                        name: Some("mexit".into()),
                        start_directory: None,
                    }),
                    extra_args: vec!["-L".into(), socket.clone()],
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                };
                let (mut handle, mut rx) = TmuxClient::spawn(config).await.expect("应能启动 tmux");

                // 等 tmux 就绪（window-add）
                let ready = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                    while let Some(ev) = rx.recv().await {
                        if matches!(ev, TmuxEvent::Message(Message::WindowAdd { .. })) {
                            return true;
                        }
                    }
                    false
                })
                .await
                .unwrap_or(false);
                assert!(ready, "应收到 window-add 表示就绪");

                // detach → tmux 输出 %exit 后进程退出
                let _ = handle.detach().await;
                let got_exit = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                    while let Some(ev) = rx.recv().await {
                        if matches!(ev, TmuxEvent::Exit { .. }) {
                            return true;
                        }
                    }
                    false
                })
                .await
                .unwrap_or(false);
                assert!(got_exit, "detach 后应收到 %exit 事件");

                let _ = handle.kill().await;
            },
            &move || kill_tmux_server(cleanup_socket.clone()),
        )
        .await;
        assert!(
            result.is_ok(),
            "end_to_end_detach_yields_exit_event 应在 15s 内完成"
        );
    }

    /// 端到端：验证半行 buffer 正确拼包——发一个会被 tmux 分多次输出的命令
    /// （list-windows 的多行响应），确认所有响应行都被收到。
    ///
    /// 隔离回归：与 end_to_end_real_tmux 相同，使用唯一 socket + 15s 有界
    /// timeout + timeout 之外 kill-server，防止并行 socket 冲突与 PTY kill 阻塞。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_multi_line_response() {
        let socket = unique_test_socket("muxterm-ml");
        let cleanup_socket = socket.clone();
        let result = run_bounded(
            15,
            || async {
                let config = TmuxClientConfig {
                    mode: Some(ConnectMode::NewSession {
                        name: Some("mxml".into()),
                        start_directory: None,
                    }),
                    extra_args: vec!["-L".into(), socket.clone()],
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                };
                let (mut handle, mut rx) = TmuxClient::spawn(config)
                    .await
                    .expect("end_to_end_multi_line_response 应能启动 tmux");

                // 等启动
                let mut started = false;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
                while !started {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::Message(Message::WindowAdd { .. })) => started = true,
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                }
                assert!(started, "end_to_end_multi_line_response 应收到 window-add");

                // 创建第二个窗口，再 list-windows，应得到 2 行响应
                handle
                    .send_command(&super::super::command::new_window(
                        super::super::command::TmuxSessionId(0),
                        Some("second"),
                    ))
                    .await
                    .unwrap();
                // 给一点时间让 window-add 到达
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                handle
                    .send_command(&super::super::command::list_windows(
                        super::super::command::TmuxSessionId(0),
                    ))
                    .await
                    .unwrap();

                let mut lines = Vec::new();
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::ResponseLine { line, .. }) => lines.push(line),
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                    if lines.len() >= 2 {
                        break;
                    }
                }
                assert!(lines.len() >= 2, "list-windows 应返回至少 2 行: {lines:?}");
                // 每行应是 `<n>: ...` 形式的窗口列表行。窗口序号受 tmux `base-index`
                // 全局设置影响（用户 `~/.tmux.conf` 可能设为 1），所以不硬编码 0/1，
                // 只校验至少出现两个**不同**的窗口序号。
                let window_idxs: Vec<&str> =
                    lines.iter().filter_map(|l| l.split(':').next()).collect();
                let distinct: std::collections::HashSet<&str> =
                    window_idxs.iter().copied().collect();
                assert!(
                    distinct.len() >= 2,
                    "应至少出现 2 个不同窗口序号: {lines:?}"
                );

                let _ = handle.kill().await;
            },
            &move || kill_tmux_server(cleanup_socket.clone()),
        )
        .await;
        assert!(
            result.is_ok(),
            "end_to_end_multi_line_response 应在 15s 内完成"
        );
    }

    #[test]
    fn unique_test_socket_is_unique_within_process() {
        let a = unique_test_socket("muxterm-test");
        let b = unique_test_socket("muxterm-test");
        let c = unique_test_socket("muxterm-ml");
        // 同一进程内每次调用必须唯一，且不同前缀不冲突。
        assert_ne!(a, b, "同一前缀两次调用应不同");
        assert_ne!(a, c);
        assert_ne!(b, c);
        // 都含进程 id 且不为空。
        assert!(!a.is_empty());
        assert!(a.starts_with("muxterm-test-"));
    }

    #[tokio::test]
    async fn run_bounded_cleans_up_on_success() {
        let cleaned = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = cleaned.clone();
        let res = run_bounded(2, || async { 42u32 }, &move || {
            flag.set(true);
            std::future::ready(())
        })
        .await;
        assert_eq!(res, Ok(42));
        assert!(cleaned.get(), "cleanup 应在成功后执行");
    }

    #[tokio::test]
    async fn run_bounded_times_out_and_still_cleans_up() {
        let cleaned = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = cleaned.clone();
        let res = run_bounded(
            1,
            || async {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                0u32
            },
            &move || {
                flag.set(true);
                std::future::ready(())
            },
        )
        .await;
        assert!(res.is_err(), "应超时返回 Err");
        assert!(cleaned.get(), "超时后 cleanup 也应执行");
    }

    #[test]
    fn config_default_and_mode() {
        let _c = TmuxClientConfig::default();
        assert_eq!(
            ConnectMode::default(),
            ConnectMode::NewSession {
                name: None,
                start_directory: None,
            }
        );
    }

    // ---------- feed_bytes_to_lines：分包/拼包/长行/UTF-8 边界 ----------

    #[test]
    fn feed_bytes_one_full_line() {
        let mut buf = Vec::new();
        let lines = feed_bytes_to_lines(&mut buf, b"%window-add @0\r\n");
        assert_eq!(lines, vec![b"%window-add @0".to_vec()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_split_across_chunks() {
        // 一条 %output 被多次 read 拆开
        let mut buf = Vec::new();
        // 三次 read 才拼出完整一行：前两段是半行，第三段带换行收尾
        assert_eq!(
            feed_bytes_to_lines(&mut buf, b"%output %0 \"ab"),
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            feed_bytes_to_lines(&mut buf, b"cd\""),
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            feed_bytes_to_lines(&mut buf, b"\r\n"),
            vec![br#"%output %0 "abcd""#.to_vec()]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_many_lines_in_one_chunk() {
        // 一次 read 含多条 %output
        let mut buf = Vec::new();
        let lines = feed_bytes_to_lines(
            &mut buf,
            b"%output %0 a\r\n%output %1 b\r\n%window-add @1\n",
        );
        assert_eq!(
            lines,
            vec![
                br#"%output %0 a"#.to_vec(),
                br#"%output %1 b"#.to_vec(),
                b"%window-add @1".to_vec(),
            ]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_long_line_across_buffers() {
        // 单条很长的 %output 跨多个内部 buffer 拼起来
        let mut buf = Vec::new();
        let long = format!("%output %0 \"{}x\"\r\n", "a".repeat(8192));
        let bytes = long.as_bytes();
        let mut got = Vec::new();
        for chunk in bytes.chunks(1000) {
            got.extend(feed_bytes_to_lines(&mut buf, chunk));
        }
        assert_eq!(got.len(), 1);
        assert!(got[0].starts_with(b"%output %0".as_slice()));
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_incomplete_utf8_kept_in_buf() {
        // 非完整 UTF-8 chunk：多字节字符被拆在两个 chunk 里
        let mut buf = Vec::new();
        assert_eq!(
            feed_bytes_to_lines(&mut buf, "中文".as_bytes()[..3].to_vec().as_slice()),
            Vec::<Vec<u8>>::new()
        );
        let lines = feed_bytes_to_lines(&mut buf, &"中文".as_bytes()[3..]);
        // 无换行时不产出行
        assert_eq!(lines, Vec::<Vec<u8>>::new());
        assert!(!buf.is_empty());
    }

    #[test]
    fn feed_bytes_incomplete_utf8_flushed_on_newline() {
        let mut buf = Vec::new();
        assert_eq!(
            feed_bytes_to_lines(&mut buf, "中文".as_bytes()[..3].to_vec().as_slice()),
            Vec::<Vec<u8>>::new()
        );
        let lines = feed_bytes_to_lines(&mut buf, "文\r\n".as_bytes());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "中文".as_bytes().to_vec());
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_dcs_prefix_stripped() {
        let mut buf = Vec::new();
        let dcs = b"\x1bP1000p%begin 123 1 0\r\n";
        let lines = feed_bytes_to_lines(&mut buf, dcs);
        assert_eq!(lines, vec![b"%begin 123 1 0".to_vec()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_crlf_and_lf() {
        let mut buf = Vec::new();
        // 同时处理 CRLF 与 LF
        let lines = feed_bytes_to_lines(&mut buf, b"%begin 1 2 0\r\n%end 1 2 0\n");
        assert_eq!(
            lines,
            vec![b"%begin 1 2 0".to_vec(), b"%end 1 2 0".to_vec()]
        );
        assert!(buf.is_empty());
    }

    /// RED 回归（来自 b.log 真实 chunk）：`%output` 的 content 可能含原始
    /// 非 UTF-8 字节（如 0x94 0x80），行切分必须逐字节保留，不能先经过
    /// `String::from_utf8_lossy` 替换成 U+FFFD（0xEF 0xBF 0xBD）。
    #[test]
    fn feed_bytes_preserves_raw_non_utf8_output_content() {
        let mut buf = Vec::new();
        // b.log 中真实出现的字节序列：%output content 以原始字节内嵌，
        // 不是 C 转义文本 `\x94\x80`。
        let chunk: &[u8] = b"%output %25 \x94\x80\r\n";
        let lines = feed_bytes_to_lines(&mut buf, chunk);
        assert_eq!(lines.len(), 1, "应切出 1 行");
        assert_eq!(lines[0], b"%output %25 \x94\x80".to_vec());
        assert!(
            !lines[0].windows(3).any(|w| w == b"\xef\xbf\xbd"),
            "行切分不得提前用 lossy 替换非 UTF-8 字节: {:?}",
            lines[0]
        );
        assert!(buf.is_empty());
    }

    /// 真实样例（用户 SSH a.log）：把 htop / git-lg 的 `%output` 行按小 chunk 喂给
    /// `feed_bytes_to_lines`（模拟 pty 分包），再 `parse_line_bytes`，验证控制字节
    /// （SO \017、backspace \010、CRLF、UTF-8）在分包 + 拼接 + 解析全程不丢、不产生
    /// replacement char。这覆盖渲染前的字节保真。
    #[test]
    fn feed_real_samples_byte_fidelity_across_chunks() {
        let chunk_sizes = [1usize, 2, 3, 7, 16, 31, 64, 256];
        for name in ["real-htop", "real-git_lg", "real-ls_la", "real-codex"] {
            let bytes = std::fs::read(format!("tests/samples/{name}.txt"))
                .unwrap_or_else(|e| panic!("读取样例 {name} 失败: {e}"));
            let mut baseline = None;
            for chunk_size in chunk_sizes {
                let mut buf: Vec<u8> = Vec::new();
                let mut outputs = Vec::new();
                for chunk in bytes.chunks(chunk_size) {
                    for line in feed_bytes_to_lines(&mut buf, chunk) {
                        if let Some(crate::core::runtime::tmux::protocol::Message::Output {
                            content,
                            ..
                        }) = parse_line_bytes(&line)
                        {
                            outputs.push(content);
                        }
                    }
                }
                if !buf.is_empty() {
                    let tail = std::mem::take(&mut buf);
                    if let Some(crate::core::runtime::tmux::protocol::Message::Output {
                        content,
                        ..
                    }) = parse_line_bytes(&tail)
                    {
                        outputs.push(content);
                    }
                }
                assert!(
                    !outputs.is_empty(),
                    "{name} chunk_size={chunk_size}: no outputs"
                );
                let combined: Vec<u8> = outputs.into_iter().flatten().collect();
                assert!(
                    !combined.windows(3).any(|w| w == b"\xef\xbf\xbd"),
                    "{name} chunk_size={chunk_size}: replacement char"
                );
                if let Some(expected) = &baseline {
                    assert_eq!(
                        &combined, expected,
                        "{name} chunk_size={chunk_size}: chunking changed output bytes"
                    );
                } else {
                    baseline = Some(combined.clone());
                }
                assert!(
                    combined.contains(&0x1b),
                    "{name} chunk_size={chunk_size}: ESC was lost"
                );
                match name {
                    "real-htop" => assert!(
                        combined.contains(&0x0f),
                        "{name} chunk_size={chunk_size}: SO was lost"
                    ),
                    "real-git_lg" => assert!(
                        combined.contains(&0x0d),
                        "{name} chunk_size={chunk_size}: CR was lost"
                    ),
                    "real-codex" => assert!(
                        combined.windows(3).any(|w| w == b"\xe2\x9d\xaf"),
                        "{name} chunk_size={chunk_size}: UTF-8 prompt was lost"
                    ),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn feed_bytes_incomplete_line_is_capped_at_maximum() {
        let mut buf = Vec::new();
        let prefix = vec![b'a'; MAX_INCOMPLETE_LINE_BYTES];
        assert!(feed_bytes_to_lines(&mut buf, &prefix).is_empty());
        assert_eq!(buf.len(), MAX_INCOMPLETE_LINE_BYTES);

        assert!(feed_bytes_to_lines(&mut buf, b"b").is_empty());
        assert_eq!(buf.len(), MAX_INCOMPLETE_LINE_BYTES);
        assert_eq!(buf.last(), Some(&b'b'));

        let mut with_newline = vec![b'c'; MAX_INCOMPLETE_LINE_BYTES];
        with_newline.push(b'\n');
        let lines = feed_bytes_to_lines(&mut buf, &with_newline);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_INCOMPLETE_LINE_BYTES);
        assert!(lines[0].iter().all(|&b| b == b'c'));
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_preserves_empty_lines_and_truncated_tail() {
        let mut buf = Vec::new();
        let lines = feed_bytes_to_lines(&mut buf, b"\r\n\n%output %1 \"tail");
        assert_eq!(lines, vec![Vec::<u8>::new(), Vec::<u8>::new()]);
        assert_eq!(buf, br#"%output %1 "tail"#.to_vec());

        let lines = feed_bytes_to_lines(&mut buf, b"\"\r\n");
        assert_eq!(lines, vec![br#"%output %1 "tail""#.to_vec()]);
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn process_line_preserves_raw_output_and_response_boundary() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut in_resp = false;
        let mut number = 0;
        let mut is_error = false;

        process_line(
            b"%begin 10 99 0",
            &tx,
            &mut in_resp,
            &mut number,
            &mut is_error,
        )
        .await;
        process_line(
            b"response \xff",
            &tx,
            &mut in_resp,
            &mut number,
            &mut is_error,
        )
        .await;
        process_line(
            b"%output %9 \"\x94\x80\"",
            &tx,
            &mut in_resp,
            &mut number,
            &mut is_error,
        )
        .await;
        process_line(
            b"%end 10 99 0",
            &tx,
            &mut in_resp,
            &mut number,
            &mut is_error,
        )
        .await;

        assert!(!in_resp);
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|event| matches!(
            event,
            TmuxEvent::Message(Message::Output {
                pane: crate::core::types::PaneId(9),
                content,
                ..
            }) if content == b"\x94\x80"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TmuxEvent::ResponseLine { number: 99, line, .. } if line.contains('\u{fffd}')
        )));
    }
}
