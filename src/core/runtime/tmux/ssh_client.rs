#![allow(clippy::while_let_loop)]
//! SSH 客户端与远程 tmux -CC。

use anyhow::{anyhow, Context, Result};
use async_ssh2_tokio::client::{AuthMethod, Client, ServerCheckMethod};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::runtime::tmux::client::{
    event_channel, feed_bytes_to_lines, process_line, OutputBatcher, ResponseBuffer, TmuxEvent,
    TmuxEventReceiver, TmuxEventSink, OUTPUT_COALESCE_IDLE,
};
use crate::core::runtime::tmux::command::TmuxCommand;

/// 把字节块渲染成可读的 debug 字符串（可打印字符保留，控制字节转义）。
/// 用于 debug 模式把 SSH 远端 tmux 的原始收发数据落盘。
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

/// SSH stdout -> tmux control protocol parser.
///
/// 独立成函数后，transport chunk 边界可以直接做回归测试；这些边界不是
/// `%output` 的语义边界，不能改变交付给 Surface 的字节流。
async fn process_tmux_stdout_chunk<S: TmuxEventSink>(
    chunk: Vec<u8>,
    buf: &mut Vec<u8>,
    parse_tx: &S,
    response: &mut Option<ResponseBuffer>,
) {
    if chunk.is_empty() {
        return;
    }
    tracing::debug!(
        target = "muxterm::ssh",
        len = chunk.len(),
        hex = %hex_debug(&chunk),
        "recv remote tmux chunk"
    );
    for line_bytes in feed_bytes_to_lines(buf, &chunk) {
        process_line(&line_bytes, parse_tx, response).await;
    }
}

async fn read_tmux_stdout<S: TmuxEventSink>(mut stdout_rx: mpsc::Receiver<Vec<u8>>, parse_tx: S) {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut response = None;
    'reader: loop {
        let Some(chunk) = stdout_rx.recv().await else {
            break;
        };
        process_tmux_stdout_chunk(chunk, &mut buf, &parse_tx, &mut response).await;

        // async-ssh2-tokio 的 channel chunk 只是 transport read 边界。
        // 在短 idle 窗内继续收，避免每个小 chunk 都把 OutputBatcher 强制
        // flush 成一个 lane event；控制消息仍会在 emit 时形成顺序边界。
        loop {
            match tokio::time::timeout(OUTPUT_COALESCE_IDLE, stdout_rx.recv()).await {
                Ok(Some(chunk)) => {
                    process_tmux_stdout_chunk(chunk, &mut buf, &parse_tx, &mut response).await;
                }
                Ok(None) => break 'reader,
                Err(_) => break,
            }
        }
        parse_tx.flush();
    }
    parse_tx.flush();
    parse_tx.emit(TmuxEvent::Exit { code: None });
}

/// SSH 连接配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            user: std::env::var("USER").unwrap_or_else(|_| "root".into()),
            auth: SshAuth::Agent,
        }
    }
}

impl SshConfig {
    /// 从配置文件字段构造（空 host 视为未配置）。
    pub fn from_file_fields(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        key_path: impl Into<String>,
    ) -> Self {
        let key_path = key_path.into();
        let auth = if key_path.is_empty() {
            SshAuth::Agent
        } else {
            SshAuth::Key {
                path: key_path,
                passphrase: None,
            }
        };
        Self {
            host: host.into(),
            port: if port == 0 { 22 } else { port },
            user: user.into(),
            auth,
        }
    }

    /// 构造远端 `tmux -CC` 命令行。
    pub fn tmux_cc_command(session_name: &str) -> String {
        build_tmux_cc_command(session_name)
    }
}

/// 认证方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshAuth {
    Key {
        path: String,
        passphrase: Option<String>,
    },
    Password(String),
    /// 使用本机 ssh-agent（Unix）。
    Agent,
}

/// SSH / 远程 tmux 错误。
#[derive(Debug, Error)]
pub enum SshError {
    #[error("SSH 连接失败: {0}")]
    Connect(String),
    #[error("SSH 执行失败: {0}")]
    Exec(String),
    #[error("SSH 未连接")]
    NotConnected,
    #[error("远程 tmux 通道已关闭")]
    ChannelClosed,
}

/// 已建立的 SSH 会话。
pub struct SshSession {
    client: Client,
    config: SshConfig,
}

impl SshSession {
    /// 连接远程服务器。
    pub async fn connect(config: SshConfig) -> Result<Self> {
        if config.host.trim().is_empty() {
            return Err(anyhow!("SSH host 为空"));
        }
        let auth = to_auth_method(&config.auth)?;
        let addr = (config.host.as_str(), config.port);
        tracing::info!(
            target = "muxterm::ssh",
            host = %config.host,
            port = config.port,
            user = %config.user,
            "connecting SSH"
        );
        let client = Client::connect(addr, &config.user, auth, ServerCheckMethod::NoCheck)
            .await
            .map_err(|e| anyhow!(SshError::Connect(e.to_string())))?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &SshConfig {
        &self.config
    }

    /// 在远程执行命令，返回 stdout 字节流（完成后可读到 EOF）。
    pub async fn exec(&self, cmd: &str) -> Result<CommandStream> {
        let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(64);
        let (stderr_tx, mut stderr_rx) = mpsc::channel::<Vec<u8>>(16);
        let client = self.client.clone();
        let cmd = cmd.to_string();
        let join = tokio::spawn(async move {
            // 合并 stderr 到日志，避免阻塞
            let drain_err = tokio::spawn(async move {
                while let Some(chunk) = stderr_rx.recv().await {
                    tracing::debug!(
                        target = "muxterm::ssh",
                        "remote stderr: {}",
                        String::from_utf8_lossy(&chunk)
                    );
                }
            });
            let code = client
                .execute_io(&cmd, stdout_tx, Some(stderr_tx), None, false, Some(1))
                .await
                .map_err(|e| anyhow!(SshError::Exec(e.to_string())))?;
            let _ = drain_err.await;
            Ok::<u32, anyhow::Error>(code)
        });
        Ok(CommandStream {
            rx: stdout_rx,
            join: Some(join),
        })
    }

    /// 在远程启动 `tmux -CC`，返回写端 + 与本地 client 同构的事件流。
    pub async fn spawn_tmux_cc(
        &self,
        session_name: &str,
    ) -> Result<(RemoteTmuxClient, TmuxEventReceiver)> {
        let cmd = build_tmux_cc_command(session_name);
        tracing::info!(target = "muxterm::ssh", %cmd, "spawn remote tmux -CC");

        let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
        let (stderr_tx, mut stderr_rx) = mpsc::channel::<Vec<u8>>(16);
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(64);
        let (event_tx, event_rx) = event_channel();

        let client = self.client.clone();
        let cmd_owned = cmd.clone();

        // stderr → tracing
        tokio::spawn(async move {
            while let Some(chunk) = stderr_rx.recv().await {
                tracing::debug!(
                    target = "muxterm::ssh",
                    "tmux-cc stderr: {}",
                    String::from_utf8_lossy(&chunk)
                );
            }
        });

        // stdout → 行解析 → TmuxEvent
        let parse_tx = OutputBatcher::new(event_tx.clone());
        let parse_join = tokio::spawn(read_tmux_stdout(stdout_rx, parse_tx));

        // SSH exec（请求 pty，tmux -CC 需要）
        let exec_join = tokio::spawn(async move {
            let code = client
                .execute_io(
                    &cmd_owned,
                    stdout_tx,
                    Some(stderr_tx),
                    Some(stdin_rx),
                    true, // request_pty
                    Some(1),
                )
                .await;
            match code {
                Ok(c) => {
                    tracing::info!(target = "muxterm::ssh", code = c, "remote tmux -CC exited");
                    Ok(c)
                }
                Err(e) => {
                    tracing::error!(target = "muxterm::ssh", "remote tmux -CC error: {e}");
                    Err(anyhow!(SshError::Exec(e.to_string())))
                }
            }
        });

        let handle = RemoteTmuxClient {
            stdin_tx,
            parse_join: Some(parse_join),
            exec_join: Some(exec_join),
            session_name: session_name.to_string(),
        };
        Ok((handle, event_rx))
    }

    /// 关闭连接（drop client）。
    pub async fn disconnect(self) -> Result<()> {
        tracing::info!(
            target = "muxterm::ssh",
            host = %self.config.host,
            "disconnect SSH"
        );
        drop(self.client);
        Ok(())
    }
}

/// `exec` 的 stdout 流。
pub struct CommandStream {
    rx: mpsc::Receiver<Vec<u8>>,
    join: Option<JoinHandle<Result<u32>>>,
}

impl CommandStream {
    /// 读下一块 stdout；`None` 表示结束。
    pub async fn next_chunk(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }

    /// 收集全部 stdout，并返回退出码。
    pub async fn collect(mut self) -> Result<(String, u32)> {
        let mut out = Vec::new();
        while let Some(chunk) = self.next_chunk().await {
            out.extend_from_slice(&chunk);
        }
        let code = match self.join.take() {
            Some(j) => j.await.context("等待 SSH exec 结束")??,
            None => 0,
        };
        Ok((String::from_utf8_lossy(&out).into_owned(), code))
    }
}

/// 远程 `tmux -CC` 客户端（接口对齐 [`crate::core::runtime::tmux::TmuxClientHandle`]）。
pub struct RemoteTmuxClient {
    stdin_tx: mpsc::Sender<Vec<u8>>,
    parse_join: Option<JoinHandle<()>>,
    exec_join: Option<JoinHandle<Result<u32>>>,
    session_name: String,
}

impl RemoteTmuxClient {
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    /// 发送一条已构造好的命令。
    pub async fn send_command(&mut self, cmd: &TmuxCommand) -> Result<()> {
        self.send_raw(&cmd.to_line()).await
    }

    /// 发送原始文本（应自带末尾换行）。
    pub async fn send_raw(&mut self, raw: &str) -> Result<()> {
        tracing::debug!(target = "muxterm::ssh", "send: {:?}", raw);
        self.stdin_tx
            .send(raw.as_bytes().to_vec())
            .await
            .map_err(|_| anyhow!(SshError::ChannelClosed))?;
        Ok(())
    }

    /// 优雅 detach。
    pub async fn detach(&mut self) -> Result<()> {
        self.send_raw("detach-client\n").await
    }

    /// 强制结束：detach + 关闭 stdin（EOF）。
    pub async fn kill(mut self) -> Result<()> {
        let _ = self.detach().await;
        // 发送空缓冲表示 EOF（async-ssh2-tokio 约定）
        let _ = self.stdin_tx.send(Vec::new()).await;
        drop(self.stdin_tx);
        if let Some(j) = self.exec_join.take() {
            let _ = j.await;
        }
        if let Some(j) = self.parse_join.take() {
            let _ = j.await;
        }
        Ok(())
    }
}

fn to_auth_method(auth: &SshAuth) -> Result<AuthMethod> {
    match auth {
        SshAuth::Password(p) => Ok(AuthMethod::with_password(p)),
        SshAuth::Key { path, passphrase } => {
            Ok(AuthMethod::with_key_file(path, passphrase.as_deref()))
        }
        SshAuth::Agent => Ok(AuthMethod::with_agent()),
    }
}

/// 构造远端 tmux -CC 命令。
///
/// - 空名：`tmux -CC new-session`
/// - 有名：`tmux -CC new-session -A -s '<name>'`（存在则 attach）
pub fn build_tmux_cc_command(session_name: &str) -> String {
    let name = session_name.trim();
    if name.is_empty() {
        "tmux -CC new-session".into()
    } else {
        format!("tmux -CC new-session -A -s {}", shell_single_quote(name))
    }
}

/// POSIX 单引号转义。
pub fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// 解析 `user@host[:port][ /session]`（命令面板 SSH 连接用）。
///
/// 返回 `(SshConfig 基础字段所需的 user/host/port, session_name)`。
/// session 可省略（空字符串 = 远端 `new-session`）。
pub fn parse_ssh_connect_line(input: &str) -> Option<(String, String, u16, String)> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    // 空格或 `/` 分隔 session
    let (target, session) = if let Some((t, sess)) = s.split_once(' ') {
        (t.trim(), sess.trim().to_string())
    } else if let Some((t, sess)) = s.split_once('/') {
        // 避免把 IPv6 里的 / 误判：仅当左侧含 `@` 或看起来像 host:port 时
        if t.contains('@') || !t.contains(':') || t.matches(':').count() == 1 {
            (t.trim(), sess.trim().to_string())
        } else {
            (s, String::new())
        }
    } else {
        (s, String::new())
    };
    let (user, host, port) = parse_ssh_target(target)?;
    Some((user, host, port, session))
}

/// 解析 `user@host[:port]` 快捷写法（UI QuickPick 用）。
pub fn parse_ssh_target(input: &str) -> Option<(String, String, u16)> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let (user, hostport) = if let Some((u, rest)) = s.split_once('@') {
        if u.is_empty() || rest.is_empty() {
            return None;
        }
        (u.to_string(), rest)
    } else {
        let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
        (user, s)
    };
    let (host, port) = if let Some((h, p)) = hostport.rsplit_once(':') {
        // IPv6 `[::1]:22` 简化：若 host 含 `]` 则拆
        if h.starts_with('[') && h.ends_with(']') {
            let inner = &h[1..h.len() - 1];
            let port: u16 = p.parse().ok()?;
            (inner.to_string(), port)
        } else if h.contains(':') {
            // 裸 IPv6 无端口
            (hostport.to_string(), 22)
        } else {
            let port: u16 = p.parse().ok()?;
            (h.to_string(), port)
        }
    } else {
        (hostport.to_string(), 22)
    };
    if host.is_empty() {
        None
    } else {
        Some((user, host, port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tmux_cc_empty_session() {
        assert_eq!(build_tmux_cc_command(""), "tmux -CC new-session");
        assert_eq!(build_tmux_cc_command("  "), "tmux -CC new-session");
    }

    #[test]
    fn build_tmux_cc_named_session() {
        assert_eq!(
            build_tmux_cc_command("dev"),
            "tmux -CC new-session -A -s 'dev'"
        );
    }

    #[test]
    fn shell_quote_escapes_single_quote() {
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn parse_ssh_target_user_host_port() {
        let (u, h, p) = parse_ssh_target("alice@example.com:2222").unwrap();
        assert_eq!(u, "alice");
        assert_eq!(h, "example.com");
        assert_eq!(p, 2222);
    }

    #[test]
    fn parse_ssh_target_host_only() {
        let (u, h, p) = parse_ssh_target("192.168.1.10").unwrap();
        assert!(!u.is_empty());
        assert_eq!(h, "192.168.1.10");
        assert_eq!(p, 22);
    }

    #[test]
    fn parse_ssh_target_rejects_empty() {
        assert!(parse_ssh_target("").is_none());
        assert!(parse_ssh_target("   ").is_none());
        assert!(parse_ssh_target("@host").is_none());
    }

    #[test]
    fn parse_ssh_connect_line_with_session() {
        let (u, h, p, s) = parse_ssh_connect_line("alice@box:2222/dev").unwrap();
        assert_eq!(u, "alice");
        assert_eq!(h, "box");
        assert_eq!(p, 2222);
        assert_eq!(s, "dev");
        let (_, _, _, s2) = parse_ssh_connect_line("alice@box main").unwrap();
        assert_eq!(s2, "main");
    }

    #[test]
    fn parse_ssh_connect_line_no_session() {
        let (u, h, p, s) = parse_ssh_connect_line("bob@host").unwrap();
        assert_eq!(u, "bob");
        assert_eq!(h, "host");
        assert_eq!(p, 22);
        assert!(s.is_empty());
    }

    #[test]
    fn ssh_config_from_file_fields_key() {
        let c = SshConfig::from_file_fields("h", 22, "u", "/home/u/.ssh/id_ed25519");
        assert_eq!(c.host, "h");
        assert_eq!(c.user, "u");
        match c.auth {
            SshAuth::Key { path, passphrase } => {
                assert!(path.ends_with("id_ed25519"));
                assert!(passphrase.is_none());
            }
            _ => panic!("expected key auth"),
        }
    }

    #[test]
    fn ssh_config_from_file_fields_agent_when_no_key() {
        let c = SshConfig::from_file_fields("h", 0, "u", "");
        assert_eq!(c.port, 22);
        assert!(matches!(c.auth, SshAuth::Agent));
    }

    #[test]
    fn ssh_config_tmux_cc_command_delegates() {
        assert_eq!(SshConfig::tmux_cc_command("s"), build_tmux_cc_command("s"));
    }

    #[tokio::test]
    async fn connect_empty_host_errors() {
        let err = SshSession::connect(SshConfig::default()).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn ssh_parser_uses_shared_byte_framer_and_preserves_output_bytes() {
        let wire =
            b"\x1bP1000p%begin 10 9 0\r\nresponse\xff\r\n%output %7 \x94\x80\r\n%end 10 9 0\n";
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut buf = Vec::new();
        let mut response = None;

        for chunk in wire.chunks(1) {
            for line in feed_bytes_to_lines(&mut buf, chunk) {
                process_line(&line, &tx, &mut response).await;
            }
        }
        assert!(buf.is_empty());
        assert!(response.is_none());

        let mut saw_output = false;
        let mut saw_response = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                TmuxEvent::Message(crate::core::runtime::tmux::protocol::Message::Output {
                    content,
                    ..
                }) => {
                    assert_eq!(content, b"\x94\x80");
                    saw_output = true;
                }
                TmuxEvent::ResponseBlock {
                    number: 9, lines, ..
                } => {
                    assert!(lines.iter().any(|line| line.contains('\u{fffd}')));
                    saw_response = true;
                }
                _ => {}
            }
        }
        assert!(saw_output, "SSH byte path must emit raw Output content");
        assert!(
            saw_response,
            "ordinary response text may use lossy String conversion"
        );
    }

    #[tokio::test]
    async fn ssh_transport_chunks_do_not_overflow_output_lane_or_split_sgr() {
        let (event_tx, mut event_rx) = event_channel();
        let (stdout_tx, stdout_rx) = mpsc::channel(256);
        let chunks = 128usize;
        let mut expected = Vec::new();

        for i in 0..chunks {
            let payload = format!("\x1b[38;2;108;108;118mframe-{i}\x1b[0m");
            expected.extend_from_slice(payload.as_bytes());
            stdout_tx
                .send(
                    format!("%output %7 \\033[38;2;108;108;118mframe-{i}\\033[0m\r\n").into_bytes(),
                )
                .await
                .unwrap();
        }
        drop(stdout_tx);

        read_tmux_stdout(stdout_rx, OutputBatcher::new(event_tx)).await;

        let mut actual = Vec::new();
        let mut output_events = 0usize;
        let mut saw_gap = false;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                TmuxEvent::Message(crate::core::runtime::tmux::protocol::Message::Output {
                    pane,
                    content,
                    ..
                }) => {
                    assert_eq!(pane, crate::core::types::PaneId(7));
                    output_events += 1;
                    actual.extend_from_slice(&content);
                }
                TmuxEvent::OutputGap { pane } => {
                    assert_eq!(pane, crate::core::types::PaneId(7));
                    saw_gap = true;
                }
                TmuxEvent::Exit { .. } => {}
                other => panic!("unexpected event: {other:?}"),
            }
        }

        assert!(
            !saw_gap,
            "SSH transport chunks must not fill the output lane"
        );
        assert_eq!(actual, expected, "SGR bytes must remain exact and ordered");
        assert!(
            output_events <= 16,
            "adjacent SSH chunks should coalesce before the lane; got {output_events}"
        );
    }

    #[tokio::test]
    async fn ssh_interleaved_panes_do_not_make_each_other_gap() {
        let (event_tx, mut event_rx) = event_channel();
        let (stdout_tx, stdout_rx) = mpsc::channel(256);
        let mut expected = [Vec::new(), Vec::new()];

        for i in 0..128usize {
            let slot = i % 2;
            let pane = 7 + slot;
            let payload = format!("\x1b[38;2;36;41;46mpane-{pane}-frame-{i}\x1b[0m");
            expected[slot].extend_from_slice(payload.as_bytes());
            stdout_tx
                .send(
                    format!(
                        "%output %{pane} \\033[38;2;36;41;46mpane-{pane}-frame-{i}\\033[0m\r\n"
                    )
                    .into_bytes(),
                )
                .await
                .unwrap();
        }
        drop(stdout_tx);

        read_tmux_stdout(stdout_rx, OutputBatcher::new(event_tx)).await;

        let mut actual = [Vec::new(), Vec::new()];
        let mut gaps = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            match event {
                TmuxEvent::Message(crate::core::runtime::tmux::protocol::Message::Output {
                    pane,
                    content,
                    ..
                }) if pane.0 == 7 || pane.0 == 8 => {
                    actual[(pane.0 - 7) as usize].extend_from_slice(&content);
                }
                TmuxEvent::OutputGap { pane } => gaps.push(pane),
                TmuxEvent::Exit { .. } => {}
                other => panic!("unexpected event: {other:?}"),
            }
        }

        assert!(
            gaps.is_empty(),
            "one chatty pane must not gap peers: {gaps:?}"
        );
        assert_eq!(
            actual, expected,
            "each pane must retain its exact byte stream"
        );
    }
}
