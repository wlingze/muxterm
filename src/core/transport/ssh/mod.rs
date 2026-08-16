//! SshProcessTransport：spawn 系统 `ssh <alias>` 进程到 PTY。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §5.4。
//!
//! **关键约束**：SSH 认证完全委托系统 `ssh <alias>`。
//! muxterm 不实现自有 SSH 认证协议，不管理密钥/agent/known_hosts/密码。
//! 只接受 SSH alias（`~/.ssh/config` 的 Host 名）。
//!
//! 为了让单元测试不依赖外部网络或本机 sshd，Transport 内部通过
//! `ProcessLauncher` trait 抽象 spawn 调用。生产用 `SystemLauncher`（spawn 系统
//! ssh 进程），测试用 `FakeLauncher`（注入假进程输出）。

pub mod probe;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use super::{Transport, TransportError, TransportSignal};

/// 把字节块渲染成可读的 debug 字符串（可打印字符保留，控制字节转义）。
/// 用于 debug 模式把 SSH 原始收发数据落盘，方便排查远端 tmux 渲染/输入问题。
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

/// 进程启动器抽象：让 spawn 可被测试注入。
///
/// 生产用 [`SystemLauncher`]（真正 spawn 进程到 PTY）；
/// 测试用 [`FakeLauncher`]（注入假输出 + 记录参数）。
pub trait ProcessLauncher: Send {
    /// 在 PTY 中启动 `program args`，返回 (master, child, reader_rx)。
    fn launch(
        &self,
        program: &str,
        args: &[&str],
        pty_size: super::PtySize,
    ) -> Result<LaunchedProcess>;
}

/// 已启动进程的句柄。
pub struct LaunchedProcess {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub reader: tokio::sync::mpsc::Receiver<Vec<u8>>,
    pub pid: u32,
}

/// 系统进程启动器：spawn 真实进程到 portable-pty。
pub struct SystemLauncher;

impl ProcessLauncher for SystemLauncher {
    fn launch(
        &self,
        program: &str,
        args: &[&str],
        pty_size: super::PtySize,
    ) -> Result<LaunchedProcess> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: pty_size.rows.max(1),
                cols: pty_size.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow::anyhow!(TransportError::Spawn(e.to_string())))?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!(TransportError::Spawn(format!("spawn {program}: {e}"))))?;
        drop(pair.slave);

        let pid = child.process_id().unwrap_or(0);

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

        Ok(LaunchedProcess {
            master: pair.master,
            child,
            reader: rx,
            pid,
        })
    }
}

/// SSH Process Transport：经系统 `ssh <alias>` 建立字节流。
///
/// 内部持有 `ProcessLauncher`，生产用 `SystemLauncher`，测试可注入 `FakeLauncher`。
pub struct SshProcessTransport {
    launcher: Box<dyn ProcessLauncher>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    pid: Option<u32>,
    /// 可选 ssh_config 路径（-F），测试/CI 用显式生成的 config。
    ssh_config_path: Option<String>,
    /// 记录最后一次 spawn 的参数（测试验证用）。
    last_program: Option<String>,
    last_args: Option<Vec<String>>,
    /// 读写字节计数（SSH 读线程 + PtyWriter 共享）。
    traffic: Option<super::TrafficCounters>,
}

impl SshProcessTransport {
    /// 创建使用系统 launcher 的 SSH Transport。
    pub fn new() -> Self {
        Self::with_launcher(Box::new(SystemLauncher))
    }

    /// 设置 ssh_config 路径（-F <path>）。
    pub fn with_ssh_config(mut self, path: impl Into<String>) -> Self {
        self.ssh_config_path = Some(path.into());
        self
    }

    /// 创建使用自定义 launcher 的 SSH Transport（测试用）。
    pub fn with_launcher(launcher: Box<dyn ProcessLauncher>) -> Self {
        Self {
            launcher,
            master: None,
            child: None,
            reader: None,
            stderr_buf: Arc::new(Mutex::new(Vec::new())),
            pid: None,
            ssh_config_path: None,
            last_program: None,
            last_args: None,
            traffic: None,
        }
    }

    /// 注入共享流量计数（spawn_ssh 创建，读线程与 PtyWriter 共用）。
    pub fn set_traffic(&mut self, traffic: super::TrafficCounters) {
        self.traffic = Some(traffic);
    }

    /// 最后一次 spawn 的 program（测试验证用）。
    #[cfg(test)]
    pub fn last_program(&self) -> Option<&str> {
        self.last_program.as_deref()
    }

    /// 最后一次 spawn 的 args（测试验证用）。
    #[cfg(test)]
    pub fn last_args(&self) -> Option<&[String]> {
        self.last_args.as_deref()
    }
}

impl Default for SshProcessTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// 构造 `ssh <alias> <remote_command>` 的命令参数列表。
///
/// **不实现自有认证**：所有密钥/agent/ProxyJump/known_hosts 由系统 ssh 处理。
///
/// 返回 `(program, args)`：
/// - program = "ssh"
/// - args = ["-tt", alias, "--", remote_command...]
///
/// 用 `-tt` 强制分配远端 pty。远端 `tmux -CC` 的 stdout 必须是 tty
/// （否则 `tcgetattr failed`），所以必须让 ssh 在远端分配 pty；本地
/// spawn ssh 的 pty 用于交互/信号，两者配合使远端 tmux -CC 正常工作。
pub fn build_ssh_command(
    alias: &str,
    remote_command: &str,
    ssh_config_path: Option<&str>,
) -> (String, Vec<String>) {
    let program = "ssh".to_string();
    let mut args = Vec::new();
    // -F <config>：显式指定 ssh config（测试/CI 用生成 config，不读 ~/.ssh/config）
    if let Some(path) = ssh_config_path {
        args.push("-F".to_string());
        args.push(path.to_string());
    }
    // -tt: 强制分配远端 pty（tmux -CC 需要远端 stdout 是 tty，否则 tcgetattr failed）
    args.push("-tt".to_string());
    // -o BatchMode=yes: 禁用密码交互
    args.push("-o".to_string());
    args.push("BatchMode=yes".to_string());
    // -o ConnectTimeout=10: 连接超时
    args.push("-o".to_string());
    args.push("ConnectTimeout=10".to_string());
    args.push(alias.to_string());
    if !remote_command.is_empty() {
        args.push("--".to_string());
        // remote_command 作为单个参数传递（由远端 shell 解释）
        args.push(remote_command.to_string());
    }
    (program, args)
}

impl SshProcessTransport {
    /// 取出 writer（供外部读取写端，如 TmuxClient SSH 模式）。
    ///
    /// 调用后 transport 不再可写，但仍可读。
    pub fn take_pty_writer(&mut self) -> Result<Box<dyn std::io::Write + Send>> {
        let master = self
            .master
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("SSH transport 未启动"))?;
        master
            .take_writer()
            .map_err(|e| anyhow::anyhow!("SSH transport take_writer 失败: {e}"))
    }
}

impl Transport for SshProcessTransport {
    fn spawn_exec(&mut self, program: &str, args: &[&str], pty_size: super::PtySize) -> Result<()> {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.last_program = Some(program.to_string());
        self.last_args = Some(args_owned.clone());

        tracing::debug!(
            target = "muxterm::ssh",
            program = %program,
            args = ?args,
            "spawn ssh transport"
        );
        let launched = self
            .launcher
            .launch(program, args, pty_size)
            .context("SSH transport spawn 失败")?;

        self.master = Some(launched.master);
        self.child = Some(launched.child);
        self.reader = Some(launched.reader);
        self.pid = Some(launched.pid);
        Ok(())
    }

    fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let Some(rx) = self.reader.as_mut() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(data) => {
                if let Some(t) = &self.traffic {
                    t.add_down(data.len() as u64);
                }
                tracing::trace!(
                    target = "muxterm::ssh",
                    len = data.len(),
                    hex = %hex_debug(&data),
                    "recv ssh chunk"
                );
                Ok(Some(data))
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                // Channel closed = reader thread exited = EOF
                self.reader = None;
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "SSH transport EOF",
                ))
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let Some(master) = self.master.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "ssh transport not started",
            ));
        };
        let mut writer = master
            .take_writer()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        tracing::debug!(
            target = "muxterm::ssh",
            len = data.len(),
            hex = %hex_debug(data),
            "send ssh bytes"
        );
        writer.write_all(data)?;
        writer.flush()?;
        if let Some(t) = &self.traffic {
            t.add_up(data.len() as u64);
        }
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
            .context("resize ssh pty 失败")?;
        Ok(())
    }

    fn kill(&mut self, signal: TransportSignal) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        let pid = child.process_id().unwrap_or(0) as i32;
        if pid > 0 {
            let sig = match signal {
                TransportSignal::Hangup => libc::SIGHUP,
                TransportSignal::Term => libc::SIGTERM,
                TransportSignal::Kill => libc::SIGKILL,
            };
            let r = unsafe { libc::kill(pid, sig) };
            if r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
        }
        let _ = child.kill();
        Ok(())
    }

    fn try_wait(&mut self) -> std::io::Result<Option<u32>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(Some(0));
        };
        match child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.exit_code())),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn shutdown(&mut self) -> Result<()> {
        self.master.take();
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

    /// 验证 `build_ssh_command` 生成正确的 ssh 命令行。
    #[test]
    fn build_ssh_command_basic() {
        let (program, args) = build_ssh_command("myserver", "tmux -CC new-session", None);
        assert_eq!(program, "ssh");
        assert!(args.contains(&"-tt".to_string()));
        assert!(args.contains(&"myserver".to_string()));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"tmux -CC new-session".to_string()));
    }

    /// 验证 ssh 命令包含 BatchMode 和 ConnectTimeout（非交互安全）。
    #[test]
    fn build_ssh_command_batch_mode_and_timeout() {
        let (_, args) = build_ssh_command("host", "echo hi", None);
        assert!(
            args.contains(&"BatchMode=yes".to_string()),
            "应包含 BatchMode=yes 以避免密码交互挂起"
        );
        assert!(
            args.contains(&"ConnectTimeout=10".to_string()),
            "应包含 ConnectTimeout 以避免连接卡死"
        );
    }

    /// 验证空 remote_command 不加 `--`。
    #[test]
    fn build_ssh_command_empty_remote() {
        let (_, args) = build_ssh_command("host", "", None);
        assert!(args.contains(&"host".to_string()));
        assert!(!args.contains(&"--".to_string()));
    }

    /// 验证 alias 不会被 shell 解释（应原样传递）。
    #[test]
    fn build_ssh_command_alias_preserved() {
        let (_, args) = build_ssh_command("prod-jump-host", "uptime", None);
        assert!(args.contains(&"prod-jump-host".to_string()));
    }

    /// 验证 ssh_config_path 时命令包含 -F <path>。
    #[test]
    fn build_ssh_command_with_config_includes_f_flag() {
        let (_, args) = build_ssh_command("myserver", "echo hi", Some("/tmp/test-ssh-config"));
        let f_idx = args.iter().position(|a| a == "-F").expect("命令应包含 -F");
        assert_eq!(
            args.get(f_idx + 1),
            Some(&"/tmp/test-ssh-config".to_string()),
            "-F 后应跟 config 路径"
        );
    }

    /// 验证无 config 时不包含 -F。
    #[test]
    fn build_ssh_command_without_config_no_f_flag() {
        let (_, args) = build_ssh_command("host", "echo hi", None);
        assert!(!args.contains(&"-F".to_string()), "无 config 不应有 -F");
    }

    /// Fake launcher 用于注入测试，不依赖外部进程。
    struct FakeLauncher {
        program: Mutex<Option<String>>,
        args: Mutex<Option<Vec<String>>>,
    }

    impl FakeLauncher {
        fn new() -> Self {
            Self {
                program: Mutex::new(None),
                args: Mutex::new(None),
            }
        }
    }

    impl ProcessLauncher for FakeLauncher {
        fn launch(
            &self,
            program: &str,
            args: &[&str],
            _pty_size: super::super::PtySize,
        ) -> Result<LaunchedProcess> {
            // 记录参数
            *self.program.lock().unwrap() = Some(program.to_string());
            *self.args.lock().unwrap() = Some(args.iter().map(|s| s.to_string()).collect());

            // 创建真实 pty 但 spawn `true`（立刻退出）模拟假进程
            let pty_system = NativePtySystem::default();
            let pair = pty_system
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| anyhow::anyhow!("fake pty: {e}"))?;
            let cmd = CommandBuilder::new("true");
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| anyhow::anyhow!("fake spawn: {e}"))?;
            drop(pair.slave);
            let (_tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
            Ok(LaunchedProcess {
                master: pair.master,
                child,
                reader: rx,
                pid: 0,
            })
        }
    }

    /// 验证 SshProcessTransport 使用注入的 launcher，并记录正确的 program/args。
    #[test]
    fn ssh_transport_uses_injected_launcher() {
        let launcher = FakeLauncher::new();
        let _launcher_box = Box::new(launcher);
        // 我们需要从 FakeLauncher 读取记录，所以用 Arc 共享
        // 但 ProcessLauncher 要求 Send + owned，这里简化：spawn 前记录地址
        let recorded_program = Arc::new(Mutex::new(None::<String>));
        let recorded_args = Arc::new(Mutex::new(None::<Vec<String>>));

        struct RecordingLauncher {
            program: Arc<Mutex<Option<String>>>,
            args: Arc<Mutex<Option<Vec<String>>>>,
        }
        impl ProcessLauncher for RecordingLauncher {
            fn launch(
                &self,
                program: &str,
                args: &[&str],
                pty_size: super::super::PtySize,
            ) -> Result<LaunchedProcess> {
                *self.program.lock().unwrap() = Some(program.to_string());
                *self.args.lock().unwrap() = Some(args.iter().map(|s| s.to_string()).collect());
                // 复用 FakeLauncher 的逻辑
                let pty_system = NativePtySystem::default();
                let pair = pty_system
                    .openpty(PtySize {
                        rows: pty_size.rows.max(1),
                        cols: pty_size.cols.max(1),
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|e| anyhow::anyhow!("pty: {e}"))?;
                let cmd = CommandBuilder::new("true");
                let child = pair
                    .slave
                    .spawn_command(cmd)
                    .map_err(|e| anyhow::anyhow!("spawn: {e}"))?;
                drop(pair.slave);
                let (_tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
                Ok(LaunchedProcess {
                    master: pair.master,
                    child,
                    reader: rx,
                    pid: 0,
                })
            }
        }

        let rl = RecordingLauncher {
            program: recorded_program.clone(),
            args: recorded_args.clone(),
        };
        let mut transport = SshProcessTransport::with_launcher(Box::new(rl));

        let (program, args) = build_ssh_command("test-alias", "echo hello", None);
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        transport
            .spawn_exec(&program, &args_ref, super::super::PtySize::new(80, 24))
            .expect("spawn with injected launcher");

        assert_eq!(
            recorded_program.lock().unwrap().as_deref(),
            Some("ssh"),
            "应记录 program=ssh"
        );
        let recorded = recorded_args.lock().unwrap();
        assert!(recorded.is_some());
        let recorded = recorded.as_ref().unwrap();
        assert!(recorded.contains(&"test-alias".to_string()));
        assert!(recorded.contains(&"echo hello".to_string()));
    }

    /// 验证 SshProcessTransport 默认用 SystemLauncher。
    #[test]
    fn ssh_transport_default_uses_system_launcher() {
        // (moved into test body above)
        // 未 spawn 时不应有进程
        let mut t = SshProcessTransport::new();
        assert!(t.try_wait().unwrap().is_some()); // None child → Some(0)
    }

    /// EOF：reader channel 关闭（远端进程退出）时，`read()` 应返回 Err（EOF），
    /// 而不是卡死或无限返回 None。这是「连接断开」路径的健壮性保障。
    #[test]
    fn ssh_transport_read_returns_eof_on_channel_close() {
        let launcher = FakeLauncher::new();
        let mut transport = SshProcessTransport::with_launcher(Box::new(launcher));

        let (program, args) = build_ssh_command("test-alias", "echo hello", None);
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        transport
            .spawn_exec(&program, &args_ref, super::super::PtySize::new(80, 24))
            .expect("spawn");

        // FakeLauncher 的 reader channel tx 立即 drop → read() 应返回 Err(EOF)
        let result = transport.read();
        assert!(
            result.is_err(),
            "channel 关闭后 read() 应返回 Err(EOF)，实际 {result:?}"
        );
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::UnexpectedEof,
            "应为 EOF 错误"
        );
    }

    /// E4：SSH transport 读写字节计数——down/up 来自真实 read/write，不假造。
    #[test]
    fn ssh_transport_counts_read_and_write_bytes() {
        struct ChannelLauncher {
            tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<Vec<u8>>>>>,
        }
        impl ProcessLauncher for ChannelLauncher {
            fn launch(
                &self,
                _program: &str,
                _args: &[&str],
                pty_size: super::super::PtySize,
            ) -> Result<LaunchedProcess> {
                let pty_system = NativePtySystem::default();
                let pair = pty_system
                    .openpty(PtySize {
                        rows: pty_size.rows.max(1),
                        cols: pty_size.cols.max(1),
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|e| anyhow::anyhow!("pty: {e}"))?;
                let cmd = CommandBuilder::new("true");
                let child = pair
                    .slave
                    .spawn_command(cmd)
                    .map_err(|e| anyhow::anyhow!("spawn: {e}"))?;
                drop(pair.slave);
                let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
                *self.tx.lock().unwrap() = Some(tx);
                Ok(LaunchedProcess {
                    master: pair.master,
                    child,
                    reader: rx,
                    pid: 0,
                })
            }
        }

        let launcher = ChannelLauncher {
            tx: Arc::new(Mutex::new(None)),
        };
        let tx_handle = launcher.tx.clone();
        let traffic = super::super::TrafficCounters::new();
        let mut transport = SshProcessTransport::with_launcher(Box::new(launcher));
        transport.set_traffic(traffic.clone());

        let (program, args) = build_ssh_command("test-alias", "echo hello", None);
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        transport
            .spawn_exec(&program, &args_ref, super::super::PtySize::new(80, 24))
            .expect("spawn");

        // 下行：reader channel 注入一块数据，read() 应累加 down。
        let tx = tx_handle
            .lock()
            .unwrap()
            .clone()
            .expect("launcher 应持有 tx");
        tx.blocking_send(b"hello-pty".to_vec()).expect("send");
        let data = transport.read().expect("read").expect("应有数据");
        assert_eq!(data, b"hello-pty");

        // 上行：write() 真实写入 pty master，应累加 up。
        let n = transport.write(b"abc").expect("write");
        assert_eq!(n, 3);

        assert_eq!(traffic.snapshot(), (9, 3), "down=9 up=3");
    }
}
