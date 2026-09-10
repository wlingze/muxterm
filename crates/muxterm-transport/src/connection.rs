//! Reusable target connection identity and bounded command adapter.

#[cfg(unix)]
use std::collections::HashMap;
use std::io;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::net::Shutdown;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;
#[cfg(unix)]
use std::time::{Duration, Instant};

use crate::local::LocalProcessTransport;
use crate::ssh::{build_ssh_command, SshProcessTransport};
use crate::{
    ByteChannel, ChannelRequest, CommandOutput, ProcessTransport, TargetConnection, TransportResult,
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
    fn transport_id(&self) -> &str {
        self.transport_id()
    }

    fn target(&self) -> &str {
        self.target()
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
            let mut command = std::process::Command::new("ssh");
            command.arg(&self.target).arg("--").arg(program);
            command.args(&argv[1..]);
            command
        } else {
            let mut command = std::process::Command::new(program);
            command.args(&argv[1..]);
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            command
        };
        command.envs(env);
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

    fn open_exec_channel(
        &self,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
        pty: Option<crate::PtySize>,
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

    fn open_unix_socket_channel(&self, path: PathBuf) -> anyhow::Result<Box<dyn ByteChannel>> {
        #[cfg(not(unix))]
        {
            let _ = path;
            return Err(anyhow::anyhow!(
                "UnixSocket channels are not supported on this platform"
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
                    let forward = self.ssh_unix_socket_forward(&path)?;
                    let stream = UnixStream::connect(&forward.local_path).map_err(|error| {
                        anyhow::anyhow!(
                            "连接 SSH UnixSocket forwarding 失败（{}）：{error}",
                            forward.local_path.display()
                        )
                    })?;
                    (stream, Some(forward))
                }
                transport => {
                    return Err(anyhow::anyhow!(
                        "unsupported target transport '{transport}'"
                    ));
                }
            };
            stream.set_nonblocking(true)?;
            Ok(Box::new(UnixSocketByteChannel { stream, forward }))
        }
    }
}

/// Adapt one transport-owned process lifetime to the Runtime-facing channel.
struct ProcessByteChannel {
    transport: Box<dyn ProcessTransport>,
}

impl ByteChannel for ProcessByteChannel {
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

#[cfg(unix)]
struct UnixSocketByteChannel {
    stream: UnixStream,
    forward: Option<Arc<SshUnixSocketForward>>,
}

#[cfg(unix)]
impl ByteChannel for UnixSocketByteChannel {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buffer = [0u8; 8192];
        match self.stream.read(&mut buffer) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Unix socket channel EOF",
            )),
            Ok(size) => Ok(Some(buffer[..size].to_vec())),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.stream.write(data)
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> TransportResult<()> {
        Err(anyhow::anyhow!("UnixSocket channels do not support resize").into())
    }

    fn shutdown(&mut self) -> TransportResult<()> {
        let _ = self.stream.shutdown(Shutdown::Both);
        self.forward.take();
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for UnixSocketByteChannel {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct SshUnixSocketForward {
    child: Child,
    local_path: PathBuf,
}

#[cfg(unix)]
impl SshUnixSocketForward {
    fn start(alias: &str, remote_path: &Path) -> anyhow::Result<Self> {
        let remote_path = remote_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("SSH UnixSocket path 不是 UTF-8"))?;
        let local_path = std::env::temp_dir().join(format!(
            "muxterm-transport-fwd-{}-{}-{}.sock",
            alias.replace(|character: char| !character.is_ascii_alphanumeric(), "-"),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_file(&local_path);

        let mut command = Command::new("ssh");
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
            .arg(format!("{}:{}", local_path.display(), remote_path))
            .arg(alias)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| anyhow::anyhow!("spawn SSH UnixSocket forwarding 失败: {error}"))?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if local_path.exists() {
                return Ok(Self { child, local_path });
            }
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&local_path);
        Err(anyhow::anyhow!(
            "SSH UnixSocket forwarding 未就绪（alias={alias}, remote={remote_path})"
        ))
    }
}

#[cfg(unix)]
impl Drop for SshUnixSocketForward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.local_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_preserves_transport_and_target_identity() {
        let connection = Connect::new("ssh", "dev");
        assert_eq!(connection.transport_id(), "ssh");
        assert_eq!(connection.target(), "dev");
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
                pty: Some(crate::PtySize::new(80, 24)),
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
            "cd '/tmp/with space' && export TEST_VALUE='x'\\''y'; exec 'printf' 'a b' 'quote'\\''value'"
        );
    }
}
