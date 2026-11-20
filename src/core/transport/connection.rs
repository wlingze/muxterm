//! Reusable target connection identity and bounded command adapter.

#[cfg(unix)]
use std::collections::HashMap;
use std::io;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
#[cfg(any(unix, windows))]
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::transport::local::LocalProcessTransport;
use crate::transport::ssh::{build_ssh_command, SshProcessTransport};
use crate::transport::{
    ByteChannel, ChannelRequest, CommandOutput, ProcessTransport, TargetConnection, TcpPortForward,
    TransportResult,
};

/// A reusable target connection for local or SSH target context.
pub struct Connect {
    transport_id: String,
    target: String,
    #[cfg(unix)]
    unix_forwards: Mutex<HashMap<PathBuf, Arc<SshUnixSocketForward>>>,
}

impl Connect {
    /// Create a connection identity; the registry owns the cached `Arc`.
    pub fn new(transport_id: impl Into<String>, target: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            transport_id: transport_id.into(),
            target: target.into(),
            #[cfg(unix)]
            unix_forwards: Mutex::new(HashMap::new()),
        })
    }

    pub fn transport_id(&self) -> &str {
        &self.transport_id
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

impl TargetConnection for Connect {
    fn store_temporary_file(&self, bytes: &[u8], extension: &str) -> TransportResult<String> {
        super::temporary_file::store(self.transport_id(), self.target(), bytes, extension)
            .map_err(Into::into)
    }
    fn transport_id(&self) -> &str {
        self.transport_id()
    }

    fn target(&self) -> &str {
        self.target()
    }

    fn open_tcp_forward(&self, remote_port: u16) -> TransportResult<Box<dyn TcpPortForward>> {
        self.open_tcp_forward_with_access(remote_port, false)
    }

    fn open_tcp_forward_with_access(
        &self,
        remote_port: u16,
        allow_lan: bool,
    ) -> TransportResult<Box<dyn TcpPortForward>> {
        if self.transport_id != "ssh" {
            return Err(anyhow::anyhow!("TCP port forwarding requires SSH transport").into());
        }
        SshTcpPortForward::start(&self.target, remote_port, allow_lan)
            .map(|forward| Box::new(forward) as Box<dyn TcpPortForward>)
            .map_err(Into::into)
    }

    fn tcp_browser_host(&self) -> TransportResult<String> {
        if self.transport_id != "ssh" {
            return Err(anyhow::anyhow!("SSH browser address requires SSH transport").into());
        }
        let config = std::env::var_os("MUXTERM_SSH_CONFIG_PATH").map(PathBuf::from);
        super::ssh::address::resolve_browser_host(&self.target, config.as_deref())
    }

    fn list_tcp_listener_ports(&self) -> TransportResult<Vec<u16>> {
        if self.transport_id != "ssh" {
            return Err(anyhow::anyhow!("TCP port listing requires SSH transport").into());
        }

        let ss = self.exec_command(ChannelRequest::Exec {
            argv: vec!["ss".into(), "-H".into(), "-ltn".into()],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })?;
        if ss.status == 0 {
            return Ok(parse_ss_listener_ports(&ss.stdout));
        }

        // macOS and a few minimal Linux images do not ship iproute2's `ss`.
        let lsof = self.exec_command(ChannelRequest::Exec {
            argv: vec![
                "lsof".into(),
                "-nP".into(),
                "-iTCP".into(),
                "-sTCP:LISTEN".into(),
            ],
            cwd: None,
            env: Vec::new(),
            pty: None,
        })?;
        if lsof.status == 0 {
            return Ok(parse_lsof_listener_ports(&lsof.stdout));
        }

        let ss_error = String::from_utf8_lossy(&ss.stderr);
        let lsof_error = String::from_utf8_lossy(&lsof.stderr);
        Err(anyhow::anyhow!(
            "could not list remote TCP ports (ss: {}; lsof: {})",
            ss_error.trim(),
            lsof_error.trim()
        )
        .into())
    }

    fn open_channel(&self, request: ChannelRequest) -> TransportResult<Box<dyn ByteChannel>> {
        match request {
            ChannelRequest::Exec {
                argv,
                cwd,
                env,
                pty,
            } => Ok(self.open_exec_channel(argv, cwd, env, pty)?),
            ChannelRequest::UnixSocket { path } => Ok(self.open_unix_socket_channel(path)?),
        }
    }

    fn exec_command(&self, request: ChannelRequest) -> TransportResult<CommandOutput> {
        let ChannelRequest::Exec {
            argv,
            cwd,
            env,
            pty: _,
        } = request
        else {
            return Err(anyhow::anyhow!("bounded commands cannot open a Unix socket").into());
        };
        let Some(program) = argv.first() else {
            return Err(anyhow::anyhow!("bounded command argv 不能为空").into());
        };

        let mut command = if self.transport_id == "ssh" {
            if cwd.is_some() {
                return Err(
                    anyhow::anyhow!("SSH bounded command must encode cwd in its argv").into(),
                );
            }
            let remote_command = build_remote_exec_command(&argv, None, &env)?;
            let mut command = std::process::Command::new("ssh");
            command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=3"]);
            if let Some(config) = std::env::var_os("MUXTERM_SSH_CONFIG_PATH") {
                command.arg("-F").arg(config);
            }
            command.arg(&self.target).arg("--").arg(remote_command);
            command
        } else {
            let mut command = std::process::Command::new(program);
            command.args(&argv[1..]);
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            command.envs(env);
            command
        };
        let output = command
            .output()
            .map_err(|error| anyhow::anyhow!("执行 target command 失败: {error}"))?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn probe(&self) -> TransportResult<()> {
        Ok(())
    }
}

fn parse_ss_listener_ports(output: &[u8]) -> Vec<u16> {
    let mut ports = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(output).lines() {
        if let Some(address) = line.split_whitespace().nth(3) {
            if let Some(port) = port_from_listener_address(address) {
                ports.insert(port);
            }
        }
    }
    ports.into_iter().collect()
}

fn parse_lsof_listener_ports(output: &[u8]) -> Vec<u16> {
    let mut ports = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(output).lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(listen_index) = fields.iter().position(|field| *field == "(LISTEN)") else {
            continue;
        };
        let Some(address) = listen_index
            .checked_sub(1)
            .and_then(|index| fields.get(index))
        else {
            continue;
        };
        if let Some(port) = port_from_listener_address(address) {
            ports.insert(port);
        }
    }
    ports.into_iter().collect()
}

fn port_from_listener_address(address: &str) -> Option<u16> {
    let (_, port) = address.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

/// OpenSSH local forward. It never writes to the user's ssh_config and is
/// stopped when its owner drops this guard.
struct SshTcpPortForward {
    child: std::process::Child,
    local_port: u16,
    lan_access_enabled: bool,
}

impl SshTcpPortForward {
    fn start(alias: &str, remote_port: u16, allow_lan: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !alias.trim().is_empty(),
            "SSH port forwarding requires a host alias"
        );
        anyhow::ensure!(remote_port != 0, "remote port must be between 1 and 65535");

        let mut last_error = None;
        let bind_address = if allow_lan { "0.0.0.0" } else { "127.0.0.1" };
        for _ in 0..10 {
            let listener = std::net::TcpListener::bind((bind_address, 0))?;
            let local_port = listener.local_addr()?.port();
            drop(listener);

            let mut command = std::process::Command::new("ssh");
            command.args([
                "-nNT",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ConnectTimeout=3",
            ]);
            if let Some(config) = std::env::var_os("MUXTERM_SSH_CONFIG_PATH") {
                command.arg("-F").arg(config);
            }
            command
                .arg("-L")
                .arg(tcp_forward_spec(bind_address, local_port, remote_port))
                .arg(alias)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped());

            let mut child = command
                .spawn()
                .map_err(|error| anyhow::anyhow!("spawn SSH TCP forwarding failed: {error}"))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait()? {
                    let mut diagnostic = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut stderr, &mut diagnostic);
                    }
                    if diagnostic
                        .to_ascii_lowercase()
                        .contains("address already in use")
                        || diagnostic
                            .to_ascii_lowercase()
                            .contains("cannot listen to port")
                    {
                        last_error = Some(format!("{status}: {}", diagnostic.trim()));
                        break;
                    }
                    anyhow::bail!(
                        "SSH port forwarding failed (alias={alias}, remote={remote_port}): {}{}",
                        status,
                        if diagnostic.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", diagnostic.trim())
                        }
                    );
                }
                if std::net::TcpListener::bind((bind_address, local_port)).is_err() {
                    return Ok(Self {
                        child,
                        local_port,
                        lan_access_enabled: allow_lan,
                    });
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "timed out starting SSH port forward (alias={alias}, remote={remote_port})"
                    );
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }

        anyhow::bail!(
            "could not allocate a local loopback port for SSH forward to {remote_port}: {}",
            last_error.unwrap_or_else(|| "local port allocation failed".to_string())
        )
    }
}

impl TcpPortForward for SshTcpPortForward {
    fn local_port(&self) -> u16 {
        self.local_port
    }

    fn lan_access_enabled(&self) -> bool {
        self.lan_access_enabled
    }
}

fn tcp_forward_spec(bind_address: &str, local_port: u16, remote_port: u16) -> String {
    format!("{bind_address}:{local_port}:127.0.0.1:{remote_port}")
}

impl Drop for SshTcpPortForward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Connect {
    #[cfg(unix)]
    fn ssh_unix_socket_forward(
        &self,
        remote_path: &Path,
    ) -> anyhow::Result<Arc<SshUnixSocketForward>> {
        let mut forwards = self
            .unix_forwards
            .lock()
            .map_err(|_| anyhow::anyhow!("SSH UnixSocket forward registry poisoned"))?;
        if let Some(forward) = forwards.get(remote_path) {
            return Ok(Arc::clone(forward));
        }

        let forward = Arc::new(SshUnixSocketForward::start(&self.target, remote_path)?);
        forwards.insert(remote_path.to_path_buf(), Arc::clone(&forward));
        Ok(forward)
    }

    #[cfg(unix)]
    fn discard_ssh_unix_socket_forward(
        &self,
        remote_path: &Path,
        failed: &Arc<SshUnixSocketForward>,
    ) -> anyhow::Result<()> {
        let mut forwards = self
            .unix_forwards
            .lock()
            .map_err(|_| anyhow::anyhow!("SSH UnixSocket forward registry poisoned"))?;
        if forwards
            .get(remote_path)
            .is_some_and(|current| Arc::ptr_eq(current, failed))
        {
            forwards.remove(remote_path);
        }
        Ok(())
    }

    #[cfg(unix)]
    fn connect_ssh_unix_socket_forward(
        &self,
        remote_path: &Path,
    ) -> anyhow::Result<(UnixStream, Arc<SshUnixSocketForward>)> {
        for attempt in 0..2 {
            let forward = self.ssh_unix_socket_forward(remote_path)?;
            match UnixStream::connect(&forward.local_path) {
                Ok(stream) => return Ok((stream, forward)),
                Err(error)
                    if attempt == 0
                        && matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                        ) =>
                {
                    // ssh 子进程退出后，本地 socket 文件可能仍在，但已无人监听。
                    // 只移除仍指向本次失败 forward 的缓存项，避免并发新连接被误删。
                    self.discard_ssh_unix_socket_forward(remote_path, &forward)?;
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "连接 SSH UnixSocket forwarding 失败（{}）：{error}",
                        forward.local_path.display()
                    ));
                }
            }
        }
        unreachable!("UnixSocket forward connect retries are bounded to two attempts")
    }

    fn open_exec_channel(
        &self,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
        pty: Option<crate::transport::PtySize>,
    ) -> anyhow::Result<Box<dyn ByteChannel>> {
        let Some(program) = argv.first() else {
            return Err(anyhow::anyhow!("Exec channel argv 不能为空"));
        };
        let args = argv[1..].iter().map(String::as_str).collect::<Vec<_>>();
        let pty_size = pty.unwrap_or_default();

        match self.transport_id.as_str() {
            "local" => {
                let mut transport = LocalProcessTransport::new();
                transport.spawn_exec_with_options(
                    program,
                    &args,
                    pty_size,
                    cwd.as_deref(),
                    &env,
                )?;
                Ok(Box::new(ProcessByteChannel {
                    transport: Box::new(transport),
                }))
            }
            "ssh" => {
                if self.target.is_empty() {
                    return Err(anyhow::anyhow!("SSH Exec channel 缺少 target alias"));
                }
                let remote_command = build_remote_exec_command(&argv, cwd.as_deref(), &env)?;
                let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
                let (ssh_program, ssh_args) =
                    build_ssh_command(&self.target, &remote_command, ssh_config.as_deref());
                let ssh_arg_refs = ssh_args.iter().map(String::as_str).collect::<Vec<_>>();
                let mut transport = SshProcessTransport::new();
                transport.spawn_exec(&ssh_program, &ssh_arg_refs, pty_size)?;
                Ok(Box::new(ProcessByteChannel {
                    transport: Box::new(transport),
                }))
            }
            transport => Err(anyhow::anyhow!(
                "unsupported target transport '{transport}'"
            )),
        }
    }

    /// 打开一条到「目标机器上某个本地 socket」的通道。
    ///
    /// 端点形态由平台决定：Unix 用 socket 路径；Windows 用命名管道（本机
    /// Herdr/shell）或 loopback TCP（`ssh -L` 的本地端，因为 Win32-OpenSSH
    /// 不支持本地 Unix socket 转发）。远端永远还是 Linux 的 Unix socket。
    fn open_unix_socket_channel(&self, path: PathBuf) -> anyhow::Result<Box<dyn ByteChannel>> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            return Err(anyhow::anyhow!(
                "target socket channels are not supported on this platform"
            ));
        }

        #[cfg(unix)]
        {
            let (stream, forward) = match self.transport_id.as_str() {
                "local" => (UnixStream::connect(&path)?, None),
                "ssh" => {
                    if self.target.is_empty() {
                        return Err(anyhow::anyhow!("SSH UnixSocket channel 缺少 target alias"));
                    }
                    let (stream, forward) = self.connect_ssh_unix_socket_forward(&path)?;
                    (stream, Some(forward))
                }
                transport => {
                    return Err(anyhow::anyhow!(
                        "unsupported target transport '{transport}'"
                    ));
                }
            };
            stream.set_nonblocking(true)?;
            Ok(Box::new(SocketByteChannel {
                stream: crate::ipc::LocalStream::Unix(stream),
                forward,
            }))
        }

        #[cfg(windows)]
        {
            match self.transport_id.as_str() {
                "local" => {
                    let endpoint = crate::ipc::endpoint_from_raw(&path.to_string_lossy())?;
                    let stream = crate::ipc::LocalStream::connect(&endpoint)?;
                    stream.set_nonblocking(true)?;
                    Ok(Box::new(SocketByteChannel {
                        stream,
                        forward: None,
                    }))
                }
                "ssh" => {
                    if self.target.is_empty() {
                        return Err(anyhow::anyhow!("SSH socket channel 缺少 target alias"));
                    }
                    let (stream, forward) = SshTcpSocketForward::start(&self.target, &path)?;
                    stream.set_nonblocking(true)?;
                    Ok(Box::new(SocketByteChannel {
                        stream,
                        forward: Some(forward),
                    }))
                }
                transport => Err(anyhow::anyhow!(
                    "unsupported target transport '{transport}'"
                )),
            }
        }
    }
}

/// Adapt one transport-owned process lifetime to the Runtime-facing channel.
struct ProcessByteChannel {
    transport: Box<dyn ProcessTransport>,
}

impl ByteChannel for ProcessByteChannel {
    fn process_observer(&self) -> Option<crate::transport::ProcessObserver> {
        self.transport.process_observer()
    }

    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.transport.read()
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.transport.write(data)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> TransportResult<()> {
        self.transport.resize(cols, rows)
    }

    fn shutdown(&mut self) -> TransportResult<()> {
        self.transport.shutdown()
    }
}

impl Drop for ProcessByteChannel {
    fn drop(&mut self) {
        let _ = self.transport.shutdown();
    }
}

fn build_remote_exec_command(
    argv: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> anyhow::Result<String> {
    let Some(_) = argv.first() else {
        return Err(anyhow::anyhow!("SSH Exec channel argv 不能为空"));
    };

    let mut command = String::new();
    if let Some(cwd) = cwd {
        let cwd = cwd
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("SSH Exec channel cwd 不是 UTF-8"))?;
        command.push_str("cd ");
        command.push_str(&shell_quote(cwd));
        command.push_str(" && ");
    }
    command.push_str("export PATH=");
    command.push_str(crate::executable::REMOTE_PATH_VALUE);
    command.push_str("; ");
    for (key, value) in env {
        if !is_valid_env_key(key) {
            return Err(anyhow::anyhow!("SSH Exec channel env key 无效: {key}"));
        }
        command.push_str("export ");
        command.push_str(key);
        command.push('=');
        command.push_str(&shell_quote(value));
        command.push_str("; ");
    }
    command.push_str("exec ");
    command.push_str(
        &argv
            .iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    Ok(command)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// 本地/转发 socket 通道：跨平台的 `LocalStream` + 平台自己的转发守卫。
struct SocketByteChannel {
    stream: crate::ipc::LocalStream,
    #[cfg(unix)]
    forward: Option<Arc<SshUnixSocketForward>>,
    #[cfg(windows)]
    forward: Option<SshTcpSocketForward>,
}

impl ByteChannel for SocketByteChannel {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self.stream.read_chunk()? {
            crate::ipc::Chunk::Data(bytes) => Ok(Some(bytes)),
            crate::ipc::Chunk::Pending => Ok(None),
            crate::ipc::Chunk::Closed => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "socket channel EOF",
            )),
        }
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.stream.write_all(data)?;
        Ok(data.len())
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> TransportResult<()> {
        Err(anyhow::anyhow!("socket channels do not support resize").into())
    }

    fn shutdown(&mut self) -> TransportResult<()> {
        let _ = self.stream.shutdown();
        #[cfg(unix)]
        {
            // 只释放本通道对共享 forward 的引用；缓存里还有别的使用者。
            self.forward.take();
        }
        #[cfg(windows)]
        {
            self.forward.take();
        }
        Ok(())
    }
}

impl Drop for SocketByteChannel {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct SshForwardDirectory(PathBuf);

#[cfg(unix)]
impl SshForwardDirectory {
    fn create() -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;

        // macOS 的 TMPDIR 很长；socket 路径不能再拼 alias 和长时间戳。
        // 使用短路径下的独占私有目录，创建失败时不触碰已有目录。
        let mut random = [0u8; 8];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let token = u64::from_ne_bytes(random);
        let path = PathBuf::from(format!("/tmp/mt-fwd-{}-{token:016x}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }

    fn socket_path(&self) -> PathBuf {
        self.0.join("s")
    }
}

#[cfg(unix)]
impl Drop for SshForwardDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct SshUnixSocketForward {
    child: Child,
    local_path: PathBuf,
    _directory: SshForwardDirectory,
}

#[cfg(unix)]
impl SshUnixSocketForward {
    fn start(alias: &str, remote_path: &Path) -> anyhow::Result<Self> {
        Self::start_command(alias, remote_path, Command::new("ssh"))
    }

    fn start_command(
        alias: &str,
        remote_path: &Path,
        mut command: Command,
    ) -> anyhow::Result<Self> {
        let remote_path = remote_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("SSH UnixSocket path 不是 UTF-8"))?;
        let directory = SshForwardDirectory::create()?;
        let local_path = directory.socket_path();
        // 文件位于 0700 私有目录；避免管道写满让 ssh 卡住，失败时仅读取有限诊断。
        let stderr_path = directory.0.join("stderr");
        let stderr = std::fs::File::create(&stderr_path)?;
        command.args([
            "-nNT",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=2",
            // Unix socket 转发是长连接；不要复用用户的 control master，
            // 避免 master 切换 / 失效时第一次 ping 直接失败。
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
        ]);
        if let Some(config) = std::env::var_os("MUXTERM_SSH_CONFIG_PATH") {
            command.arg("-F").arg(config);
        }
        command
            .arg("-L")
            .arg(format!("{}:{}", local_path.display(), remote_path))
            .arg(alias)
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        let mut child = command
            .spawn()
            .map_err(|error| anyhow::anyhow!("spawn SSH UnixSocket forwarding 失败: {error}"))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut exit_status = None;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                exit_status = Some(status);
                break;
            }
            if local_path.exists() {
                return Ok(Self {
                    child,
                    local_path,
                    _directory: directory,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = child.kill();
        let _ = child.wait();
        let mut diagnostic = Vec::new();
        if let Ok(file) = std::fs::File::open(&stderr_path) {
            let _ = file.take(4096).read_to_end(&mut diagnostic);
        }
        let reason =
            exit_status.map_or_else(|| "等待超时".to_string(), |status| status.to_string());
        Err(anyhow::anyhow!(
            "SSH UnixSocket forwarding 未就绪（alias={alias}, remote={remote_path}, {reason}）: {}",
            String::from_utf8_lossy(&diagnostic).trim()
        ))
    }
}

#[cfg(unix)]
impl Drop for SshUnixSocketForward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // 字段随后 drop：先停止 ssh，再移除该转发独占的临时目录。
    }
}

/// Windows 上的 SSH socket 转发：本地 loopback TCP → 远端 Unix socket。
///
/// Win32-OpenSSH 不支持把本地 Unix socket 作为 `-L` 的本地端，但 `-L
/// 127.0.0.1:<port>:<remote unix socket>`（streamlocal 的 TCP 本地端）是支持的。
/// 端口由本进程先绑定再释放，撞车时换一个重试。
#[cfg(windows)]
#[derive(Debug)]
struct SshTcpSocketForward {
    child: Child,
    port: u16,
}

#[cfg(windows)]
impl SshTcpSocketForward {
    fn start(alias: &str, remote_path: &Path) -> anyhow::Result<(crate::ipc::LocalStream, Self)> {
        let remote = remote_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("SSH socket path 不是 UTF-8"))?;
        let mut last_error: Option<anyhow::Error> = None;
        for _ in 0..3 {
            let port = crate::ipc::free_loopback_port()?;
            match Self::try_start(alias, remote, port) {
                // 探测连接直接当通道用：再连一次会多出一条被 ssh 立刻关闭的
                // 转发连接，Herdr 那边会看到一次多余的 accept。
                Ok((stream, forward)) => {
                    return Ok((crate::ipc::LocalStream::Tcp(stream), forward));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("SSH socket forwarding 失败（alias={alias}）")))
    }

    fn try_start(
        alias: &str,
        remote_path: &str,
        port: u16,
    ) -> anyhow::Result<(std::net::TcpStream, Self)> {
        let mut command = Command::new("ssh");
        // Windows 没有 control master，别传 ControlMaster/ControlPath。
        command.args([
            "-nNT",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=2",
        ]);
        if let Some(config) = std::env::var_os("MUXTERM_SSH_CONFIG_PATH") {
            command.arg("-F").arg(config);
        }
        command
            .arg("-L")
            .arg(format!("127.0.0.1:{port}:{remote_path}"))
            .arg(alias)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| anyhow::anyhow!("spawn SSH socket forwarding 失败: {error}"))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                // ssh 已退出：stderr 里是「-L 语法不支持 / 远端 socket 不存在」
                // 之类的原因，读出来比只说 exit code 有用得多。
                let diagnostic = child
                    .stderr
                    .take()
                    .map(|stderr| {
                        use std::io::Read as _;
                        let mut text = String::new();
                        let _ = stderr.take(4096).read_to_string(&mut text);
                        text
                    })
                    .unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "SSH socket forwarding 退出（alias={alias}, remote={remote_path}, {status}）: {}",
                    diagnostic.trim()
                ));
            }
            if let Ok(stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                stream.set_nodelay(true)?;
                return Ok((stream, Self { child, port }));
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = child.kill();
        let _ = child.wait();
        Err(anyhow::anyhow!(
            "SSH socket forwarding 未就绪（alias={alias}, remote={remote_path}, 127.0.0.1:{port} 等待超时）"
        ))
    }
}

#[cfg(windows)]
impl Drop for SshTcpSocketForward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[cfg(unix)]
    #[test]
    fn ssh_forward_socket_binds_in_private_directory_and_is_cleaned_up() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        let directory = SshForwardDirectory::create().unwrap();
        let second = SshForwardDirectory::create().unwrap();
        let path = directory.socket_path();
        assert_ne!(path, second.socket_path());
        assert_eq!(
            std::fs::metadata(&directory.0)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let listener = UnixListener::bind(&path).expect("SSH local socket path must be bindable");
        let mut client = UnixStream::connect(&path).unwrap();
        client.write_all(b"ping").unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let mut data = [0; 4];
        server.read_exact(&mut data).unwrap();
        assert_eq!(&data, b"ping");
        drop(listener);
        drop(directory);
        assert!(!path.parent().unwrap().exists());
        assert!(second.0.exists(), "cleanup must not remove another forward");
    }

    #[cfg(unix)]
    #[test]
    fn ssh_forward_failure_preserves_stderr_and_exit_status() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'forward bind failed' >&2; exit 42"]);
        let error = SshUnixSocketForward::start_command(
            "test-alias",
            Path::new("/remote/test.sock"),
            command,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("forward bind failed"), "{error}");
        assert!(error.contains("42"), "{error}");
        assert!(error.contains("test-alias"), "{error}");
    }

    #[test]
    fn connection_preserves_transport_and_target_identity() {
        let connection = Connect::new("ssh", "dev");
        assert_eq!(connection.transport_id(), "ssh");
        assert_eq!(connection.target(), "dev");
    }

    #[test]
    fn parses_ss_tcp_listener_addresses_and_deduplicates_ports() {
        let output = b"State Recv-Q Send-Q Local Address:Port Peer Address:Port\n\
            LISTEN 0 128 127.0.0.1:3000 0.0.0.0:*\n\
            LISTEN 0 4096 [::]:443 [::]:*\n\
            LISTEN 0 128 0.0.0.0:3000 0.0.0.0:*\n\
            LISTEN 0 128 0.0.0.0:0 0.0.0.0:*\n\
            malformed listener row\n";
        assert_eq!(parse_ss_listener_ports(output), vec![443, 3000]);
    }

    #[test]
    fn parses_lsof_tcp_listener_addresses_and_ignores_headers() {
        let output = b"COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n\
            node 42 alice 19u IPv4 0x1 0t0 TCP *:5173 (LISTEN)\n\
            node 42 alice 20u IPv6 0x2 0t0 TCP [::1]:5173 (LISTEN)\n\
            sshd 1 root 3u IPv4 0x3 0t0 TCP *:22 (ESTABLISHED)\n\
            test 2 root 4u IPv4 0x4 0t0 TCP *:0 (LISTEN)\n";
        assert_eq!(parse_lsof_listener_ports(output), vec![5173]);
    }

    #[test]
    fn ssh_forward_specs_bind_loopback_or_all_interfaces() {
        assert_eq!(
            tcp_forward_spec("127.0.0.1", 41001, 3000),
            "127.0.0.1:41001:127.0.0.1:3000"
        );
        assert_eq!(
            tcp_forward_spec("0.0.0.0", 41001, 3000),
            "0.0.0.0:41001:127.0.0.1:3000"
        );
    }

    #[test]
    fn unix_socket_command_is_rejected_by_bounded_exec() {
        let connection = Connect::new("local", "");
        let result = connection.exec_command(ChannelRequest::UnixSocket {
            path: "/tmp/muxterm-test.sock".into(),
        });
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_unix_socket_channel_forwards_bytes() {
        use std::os::unix::net::UnixListener;

        let path = std::env::temp_dir().join(format!(
            "muxterm-transport-test-{}-{}.sock",
            std::process::id(),
            1
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind Unix socket");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept Unix socket");
            stream
                .write_all(b"socket-ready")
                .expect("write Unix socket");
        });

        let connection = Connect::new("local", "");
        let mut channel = connection
            .open_channel(ChannelRequest::UnixSocket { path: path.clone() })
            .expect("open local Unix socket channel");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut output = Vec::new();
        while std::time::Instant::now() < deadline {
            match channel.read().expect("read Unix socket channel") {
                Some(data) => output.extend(data),
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
            if output
                .windows(b"socket-ready".len())
                .any(|window| window == b"socket-ready")
            {
                break;
            }
        }
        assert!(
            output
                .windows(b"socket-ready".len())
                .any(|window| window == b"socket-ready"),
            "Unix socket output did not arrive: {output:?}"
        );
        channel.shutdown().expect("shutdown Unix socket channel");
        server.join().expect("join Unix socket server");
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn local_unix_socket_channel_reports_eof() {
        use std::os::unix::net::UnixListener;

        let path = std::env::temp_dir().join(format!(
            "muxterm-transport-eof-{}-{}.sock",
            std::process::id(),
            1
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind Unix socket");
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("accept Unix socket");
        });

        let connection = Connect::new("local", "");
        let mut channel = connection
            .open_channel(ChannelRequest::UnixSocket { path: path.clone() })
            .expect("open local Unix socket channel");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut eof = None;
        while std::time::Instant::now() < deadline {
            match channel.read() {
                Err(error) => {
                    eof = Some(error);
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        let error = eof.expect("Unix socket EOF should be reported");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        channel.shutdown().expect("shutdown Unix socket channel");
        server.join().expect("join Unix socket server");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_exec_channel_forwards_cwd_env_and_output() {
        let connection = Connect::new("local", "");
        let mut channel = connection
            .open_channel(ChannelRequest::Exec {
                argv: vec![
                    "sh".into(),
                    "-c".into(),
                    "printf '%s' \"$MUXTERM_CHANNEL_TEST\"".into(),
                ],
                cwd: Some("/tmp".into()),
                env: vec![("MUXTERM_CHANNEL_TEST".into(), "ready value".into())],
                pty: Some(crate::transport::PtySize::new(80, 24)),
            })
            .expect("open local exec channel");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut output = Vec::new();
        while std::time::Instant::now() < deadline {
            match channel.read().expect("read local exec channel") {
                Some(data) => output.extend(data),
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
            if output
                .windows(b"ready value".len())
                .any(|window| window == b"ready value")
            {
                break;
            }
        }
        assert!(
            output
                .windows(b"ready value".len())
                .any(|window| window == b"ready value"),
            "local exec output did not contain the forwarded environment: {output:?}"
        );
        channel.shutdown().expect("shutdown local exec channel");
    }

    #[test]
    fn ssh_exec_command_quotes_cwd_environment_and_argv() {
        let argv = vec!["printf".into(), "a b".into(), "quote'value".into()];
        let command = build_remote_exec_command(
            &argv,
            Some(Path::new("/tmp/with space")),
            &[("TEST_VALUE".into(), "x'y".into())],
        )
        .expect("build remote command");
        assert_eq!(
            command,
            "cd '/tmp/with space' && export PATH=\"$HOME/.local/bin:$HOME/.nix-profile/bin:/opt/homebrew/bin:/usr/local/bin:/opt/local/bin:/run/current-system/sw/bin:$PATH\"; export TEST_VALUE='x'\\''y'; exec 'printf' 'a b' 'quote'\\''value'"
        );
    }
}
