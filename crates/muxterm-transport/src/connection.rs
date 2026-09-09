//! Reusable target connection identity and bounded command adapter.

use std::io;
use std::path::Path;
use std::sync::Arc;

use crate::local::LocalProcessTransport;
use crate::ssh::{build_ssh_command, SshProcessTransport};
use crate::{ByteChannel, ChannelRequest, CommandOutput, TargetConnection, Transport};

/// A reusable target connection for local or SSH target context.
#[derive(Debug)]
pub struct Connect {
    transport_id: String,
    target: String,
}

impl Connect {
    /// Create a connection identity; the registry owns the cached `Arc`.
    pub fn new(transport_id: impl Into<String>, target: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            transport_id: transport_id.into(),
            target: target.into(),
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

    fn open_channel(&self, request: ChannelRequest) -> anyhow::Result<Box<dyn ByteChannel>> {
        let ChannelRequest::Exec {
            argv,
            cwd,
            env,
            pty,
        } = request
        else {
            return Err(anyhow::anyhow!(
                "UnixSocket channels are not implemented by this connection adapter"
            ));
        };
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

    fn exec_command(&self, request: ChannelRequest) -> anyhow::Result<CommandOutput> {
        let ChannelRequest::Exec {
            argv,
            cwd,
            env,
            pty: _,
        } = request
        else {
            return Err(anyhow::anyhow!(
                "bounded commands cannot open a Unix socket"
            ));
        };
        let Some(program) = argv.first() else {
            return Err(anyhow::anyhow!("bounded command argv 不能为空"));
        };

        let mut command = if self.transport_id == "ssh" {
            if cwd.is_some() {
                return Err(anyhow::anyhow!(
                    "SSH bounded command must encode cwd in its argv"
                ));
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

    fn probe(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Adapt one transport-owned process lifetime to the Runtime-facing channel.
struct ProcessByteChannel {
    transport: Box<dyn Transport>,
}

impl ByteChannel for ProcessByteChannel {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.transport.read()
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.transport.write(data)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.transport.resize(cols, rows)
    }

    fn shutdown(&mut self) -> anyhow::Result<()> {
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
