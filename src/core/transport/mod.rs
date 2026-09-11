//! Transport 层：纯粹的字节流通道，不理解任何终端语义或复用协议。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §5。
//!
//! 扩展规则：新增 Transport 不修改 Runtime、不修改 Core Protocol。
//! Runtime 不关心 Transport 是 local 还是 SSH；Transport 不理解 shell/tmux 语义。

pub mod connection;
pub mod connection_registry;
#[cfg(feature = "test-harness")]
pub mod local;
#[cfg(not(feature = "test-harness"))]
mod local;
pub mod provider;
pub mod registry;
#[cfg(feature = "test-harness")]
pub mod ssh;
#[cfg(not(feature = "test-harness"))]
mod ssh;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub use connection::Connect;
pub use connection_registry::ConnectionRegistry;
pub use provider::{TargetInfo, TransportInfo, TransportProvider};
pub use registry::{with_builtins, TransportRegistry};
pub use ssh::config::{list_ssh_hosts, parse_ssh_config, SshHostEntry};

/// Channel kinds are the only transport capability a Runtime provider needs
/// to resolve a Runtime × Transport combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    /// A command running over a byte-oriented PTY or pipe.
    Exec,
    /// A connection to an existing Unix socket.
    UnixSocket,
}

/// Request for a Runtime-owned byte channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRequest {
    Exec {
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
        pty: Option<PtySize>,
    },
    UnixSocket {
        path: PathBuf,
    },
}

/// Result of a short-lived target command used by Core services such as
/// Projects. Runtime-owned interactive channels remain `ByteChannel`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ChannelRequest {
    pub fn kind(&self) -> ChannelKind {
        match self {
            Self::Exec { .. } => ChannelKind::Exec,
            Self::UnixSocket { .. } => ChannelKind::UnixSocket,
        }
    }
}

/// Runtime-facing byte channel. It contains no terminal or pane semantics.
pub trait ByteChannel: Send {
    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>>;
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize>;
    fn resize(&mut self, cols: u16, rows: u16) -> TransportResult<()>;
    fn shutdown(&mut self) -> TransportResult<()>;
}

/// Reusable target-level connection owned by the transport registry.
pub trait TargetConnection: Send + Sync {
    fn transport_id(&self) -> &str;
    fn target(&self) -> &str;
    fn open_channel(&self, request: ChannelRequest) -> TransportResult<Box<dyn ByteChannel>>;
    /// Execute a bounded, non-interactive command on this target.
    ///
    /// The default keeps existing test connections source-compatible; real
    /// providers may implement it when Core services need target-side work.
    fn exec_command(&self, _request: ChannelRequest) -> TransportResult<CommandOutput> {
        Err(TransportError::message(
            "target connection does not support bounded commands",
        ))
    }
    fn probe(&self) -> TransportResult<()>;
}

/// PTY 字符格尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl PtySize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// 传输读写字节计数（SSH 读线程与 PtyWriter 各自累加，前端轮询快照）。
#[derive(Debug, Clone, Default)]
pub struct TrafficCounters {
    down: Arc<AtomicU64>,
    up: Arc<AtomicU64>,
}

impl TrafficCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录下行（远端 → 本地）字节。
    pub fn add_down(&self, n: u64) {
        self.down.fetch_add(n, Ordering::Relaxed);
    }

    /// 记录上行（本地 → 远端）字节。
    pub fn add_up(&self, n: u64) {
        self.up.fetch_add(n, Ordering::Relaxed);
    }

    /// 当前 (down, up) 字节快照。
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.down.load(Ordering::Relaxed),
            self.up.load(Ordering::Relaxed),
        )
    }
}

/// 传输信号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSignal {
    /// SIGHUP — 关闭终端会话。
    Hangup,
    /// SIGTERM — 优雅终止。
    Term,
    /// SIGKILL — 强制终止。
    Kill,
}

/// Transport 错误。
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("spawn 失败: {0}")]
    Spawn(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("进程已退出")]
    Exited,
    #[error("Transport 未启动")]
    NotStarted,
    #[error("Transport 操作失败: {0}")]
    Message(String),
}

impl TransportError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl From<anyhow::Error> for TransportError {
    fn from(error: anyhow::Error) -> Self {
        Self::message(format!("{error:#}"))
    }
}

/// Result alias for the transport library boundary.
pub type TransportResult<T> = std::result::Result<T, TransportError>;

/// Spawned process lifetime used internally to build a [`ByteChannel`].
///
/// A process transport owns one local or remote process (spawn → read/write →
/// exit). It does not understand pane, session, or tmux semantics. Runtime
/// code receives the narrower [`ByteChannel`] through [`TargetConnection`].
///
/// The synchronous methods are used by the transport adapters and their
/// background reader threads.
pub trait ProcessTransport: Send {
    /// 在远端（或本地）以 PTY 模式启动一个长驻命令。
    ///
    /// `program` 在 local 为 shell/tmux 路径，在 ssh 为经 SSH 执行的命令。
    /// `pty_size` 初始字符格尺寸。
    fn spawn_exec(
        &mut self,
        program: &str,
        args: &[&str],
        pty_size: PtySize,
    ) -> TransportResult<()>;

    /// Spawn a PTY process with target-side working directory and environment.
    ///
    /// Existing callers that only need the basic process contract can keep
    /// using [`ProcessTransport::spawn_exec`]. Providers that expose an
    /// `Exec` [`ChannelRequest`] should override this method when the
    /// underlying process supports these options.
    fn spawn_exec_with_options(
        &mut self,
        program: &str,
        args: &[&str],
        pty_size: PtySize,
        cwd: Option<&Path>,
        env: &[(String, String)],
    ) -> TransportResult<()> {
        if cwd.is_some() || !env.is_empty() {
            return Err(TransportError::message(
                "transport does not support process cwd/env options",
            ));
        }
        self.spawn_exec(program, args, pty_size)
    }

    /// 非阻塞读取 stdout/pty master 的下一块字节。None 表示 EOF / 进程退出。
    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>>;

    /// 写入 stdin/pty master。返回写入字节数。
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize>;

    /// 调整 PTY 字符格尺寸（SIGWINCH / pty resize）。
    fn resize(&mut self, cols: u16, rows: u16) -> TransportResult<()>;

    /// 发送信号给子进程。
    fn kill(&mut self, signal: TransportSignal) -> TransportResult<()>;

    /// 非阻塞探测是否已退出。Some(code) 表示已退出；None 表示仍运行。
    fn try_wait(&mut self) -> std::io::Result<Option<u32>>;

    /// 优雅关闭：关闭写端，等待退出，回收资源。
    fn shutdown(&mut self) -> TransportResult<()>;

    /// stderr 累积（调试用；有界 64KB）。
    fn stderr(&self) -> Vec<u8>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_size_default_80x24() {
        let s = PtySize::default();
        assert_eq!(s.cols, 80);
        assert_eq!(s.rows, 24);
    }

    #[test]
    fn pty_size_new_custom() {
        let s = PtySize::new(120, 40);
        assert_eq!(s.cols, 120);
        assert_eq!(s.rows, 40);
    }

    #[test]
    fn transport_signal_variants() {
        assert_ne!(TransportSignal::Hangup, TransportSignal::Term);
        assert_ne!(TransportSignal::Term, TransportSignal::Kill);
    }
}
