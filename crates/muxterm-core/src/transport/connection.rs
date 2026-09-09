//! Reusable target-level connection identity.

use std::sync::Arc;

use super::{ByteChannel, ChannelRequest, CommandOutput, TargetConnection};

/// A reusable target connection (local no-op / SSH control context / test mock).
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

    fn open_channel(&self, _request: ChannelRequest) -> anyhow::Result<Box<dyn ByteChannel>> {
        Err(anyhow::anyhow!(
            "target connection '{}' has no channel adapter yet",
            self.target
        ))
    }

    fn exec_command(&self, request: ChannelRequest) -> anyhow::Result<CommandOutput> {
        let super::ChannelRequest::Exec {
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
