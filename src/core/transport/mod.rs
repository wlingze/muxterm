//! Transport 层：纯粹的字节流通道，不理解任何终端语义或复用协议。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §5。
//!
//! 扩展规则：新增 Transport 不修改 Runtime、不修改 Core Protocol。
//! - [`LocalProcessTransport`]：本地 portable-pty spawn
//! - [`SshProcessTransport`]：spawn 系统 `ssh <alias>` 进程
//!
//! Runtime 不关心 Transport 是 local 还是 SSH；Transport 不理解 shell/tmux 语义。

pub mod local;
pub mod ssh;

pub use local::LocalProcessTransport;
pub use ssh::SshProcessTransport;

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
}

/// Transport trait：在本地或远程执行一个长驻命令，提供双向字节流。
///
/// 一个 Transport 实例 = 一次进程生命周期（spawn → read/write → exit）。
/// 不理解 pane/session/tmux，只管字节流 + PTY 控制。
///
/// 同步接口（内部可 spawn 后台线程做 async→sync 桥接），
/// 与 `Backend::execute` 同步签名一致。
pub trait Transport: Send {
    /// 在远端（或本地）以 PTY 模式启动一个长驻命令。
    ///
    /// `program` 在 local 为 shell/tmux 路径，在 ssh 为经 SSH 执行的命令。
    /// `pty_size` 初始字符格尺寸。
    fn spawn_exec(&mut self, program: &str, args: &[&str], pty_size: PtySize)
        -> anyhow::Result<()>;

    /// 非阻塞读取 stdout/pty master 的下一块字节。None 表示 EOF / 进程退出。
    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>>;

    /// 写入 stdin/pty master。返回写入字节数。
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize>;

    /// 调整 PTY 字符格尺寸（SIGWINCH / pty resize）。
    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()>;

    /// 发送信号给子进程。
    fn kill(&mut self, signal: TransportSignal) -> anyhow::Result<()>;

    /// 非阻塞探测是否已退出。Some(code) 表示已退出；None 表示仍运行。
    fn try_wait(&mut self) -> std::io::Result<Option<u32>>;

    /// 优雅关闭：关闭写端，等待退出，回收资源。
    fn shutdown(&mut self) -> anyhow::Result<()>;

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
